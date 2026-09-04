//! Local navigation analysis over the supported M3 concrete syntax tree.

use std::collections::BTreeSet;

use pure_analyzer_model::{ModelGraph, Multiplicity, Name, QName, TypeRef};
use pure_analyzer_resolve::{
    LocalValue, NavigationResolution, NavigationResolver, NavigationStep,
    NavigationUnderResolution, RelationColumn, RelationColumnId, RelationRow, Resolution,
    TypeEnvironment, TypeScope, UnknownValue,
};
use pure_analyzer_syntax::{GreenElement, GreenNode, SyntaxKind, TextRange};

use crate::cst_util::{contains_error_node, direct_nodes, is_trivia};

const NAME_KINDS: [SyntaxKind; 6] = [
    SyntaxKind::IDENT,
    SyntaxKind::ALL_KW,
    SyntaxKind::LET_KW,
    SyntaxKind::ALL_VERSIONS_KW,
    SyntaxKind::ALL_VERSIONS_IN_RANGE_KW,
    SyntaxKind::TO_BYTES_KW,
];

/// One local-resolution result attached to a precise source span.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalResolutionSite {
    span: TextRange,
    reference_span: TextRange,
    outcome: LocalResolution,
}

impl LocalResolutionSite {
    fn new(span: TextRange, reference_span: TextRange, outcome: LocalResolution) -> Self {
        Self {
            span,
            reference_span,
            outcome,
        }
    }

    /// Return the exact syntax span that produced this result.
    #[must_use]
    pub const fn span(&self) -> TextRange {
        self.span
    }

    /// Return the exact identifier span that this resolution site resolves.
    #[must_use]
    pub const fn reference_span(&self) -> TextRange {
        self.reference_span
    }

    /// Return the resolution outcome at [`Self::span`].
    #[must_use]
    pub const fn outcome(&self) -> &LocalResolution {
        &self.outcome
    }
}

/// A locally resolved M3 expression form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalResolution {
    /// A `Class.all()`-style source expression.
    ClassAll(Resolution<LocalValue>),
    /// A property-style navigation step over a local source.
    Navigation(NavigationResolution),
}

/// Ordered local-resolution results for one M3 concrete syntax tree.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LocalNavigationAnalysis {
    sites: Vec<LocalResolutionSite>,
}

impl LocalNavigationAnalysis {
    /// Return all resolution sites in source order.
    #[must_use]
    pub fn sites(&self) -> &[LocalResolutionSite] {
        &self.sites
    }
}

/// Analyze locally resolvable M3 class sources and property navigations.
///
/// The pass deliberately limits itself to facts that syntax and model can
/// establish without full Pure inference. Unsupported calls, opaque islands,
/// and recovery nodes produce an unknown flow, so they cannot turn into a
/// false closed-world missing-member result.
#[must_use]
pub fn analyze_m3_locals(tree: &GreenNode, graph: &ModelGraph) -> LocalNavigationAnalysis {
    let mut analyzer = LocalAnalyzer {
        resolver: NavigationResolver::new(graph),
        sites: Vec::new(),
    };
    let mut environment = TypeEnvironment::new();
    let mut root_scope = environment.scope();
    let _ = analyzer.evaluate_node(tree, &mut root_scope);
    LocalNavigationAnalysis {
        sites: analyzer.sites,
    }
}

#[derive(Debug)]
struct LocalAnalyzer<'model> {
    resolver: NavigationResolver<'model>,
    sites: Vec<LocalResolutionSite>,
}

#[derive(Debug, Clone)]
enum RelationParameterBinding {
    NotRelation,
    Row(LocalValue),
    Invalid,
}

trait LocalBindings {
    fn bind(&mut self, name: Name, value: LocalValue);
    fn lookup(&self, name: &Name) -> Option<&LocalValue>;
    fn scope(&mut self) -> impl LocalBindings + '_;
}

impl LocalBindings for TypeScope<'_> {
    fn bind(&mut self, name: Name, value: LocalValue) {
        let _ = TypeScope::bind(self, name, value);
    }

    fn lookup(&self, name: &Name) -> Option<&LocalValue> {
        TypeScope::lookup(self, name)
    }

    fn scope(&mut self) -> impl LocalBindings + '_ {
        TypeScope::scope(self)
    }
}

impl LocalAnalyzer<'_> {
    fn unknown_value() -> LocalValue {
        LocalValue::unknown(UnknownValue::HigherOrder, Multiplicity::zero_or_more())
    }

    fn evaluate_node(
        &mut self,
        node: &GreenNode,
        environment: &mut impl LocalBindings,
    ) -> LocalValue {
        match node.kind() {
            SyntaxKind::LAMBDA_EXPR => {
                self.evaluate_lambda(node, Self::unknown_value(), environment)
            }
            _ => self.evaluate_nodes(&direct_nodes(node), environment),
        }
    }

    fn evaluate_nodes(
        &mut self,
        nodes: &[GreenNode],
        environment: &mut impl LocalBindings,
    ) -> LocalValue {
        let mut value = Self::unknown_value();
        let mut variable_source = None;

        for node in nodes {
            match node.kind() {
                SyntaxKind::QUERY_EXPR
                | SyntaxKind::BINARY_EXPR
                | SyntaxKind::PAREN_EXPR
                | SyntaxKind::UNARY_EXPR => {
                    value = self.evaluate_nodes(&direct_nodes(node), environment);
                    variable_source = None;
                }
                SyntaxKind::ALL_EXPR => {
                    value = self.evaluate_class_all(node);
                    variable_source = None;
                }
                SyntaxKind::VARIABLE_EXPR => {
                    variable_source = variable_name(node);
                    value = variable_source
                        .as_ref()
                        .and_then(|name| environment.lookup(name))
                        .cloned()
                        .unwrap_or_else(Self::unknown_value);
                }
                SyntaxKind::PROPERTY_NAV => {
                    value = self.evaluate_property_navigation(
                        node,
                        value,
                        variable_source.take(),
                        environment,
                    );
                }
                SyntaxKind::ARROW_CALL => {
                    value = self.evaluate_arrow_call(node, value, environment);
                    variable_source = None;
                }
                SyntaxKind::FUNCTION_CALL => {
                    self.evaluate_function_call(node, environment);
                    value = Self::unknown_value();
                    variable_source = None;
                }
                SyntaxKind::LAMBDA_EXPR => {
                    value = self.evaluate_lambda(node, Self::unknown_value(), environment);
                    variable_source = None;
                }
                _ => {
                    let _ = self.evaluate_node(node, environment);
                    value = Self::unknown_value();
                    variable_source = None;
                }
            }
        }

        value
    }

    fn evaluate_class_all(&mut self, node: &GreenNode) -> LocalValue {
        let Some((path, reference_span)) = direct_nodes(node)
            .into_iter()
            .find(|child| child.kind() == SyntaxKind::QUALIFIED_NAME)
            .and_then(|child| {
                let reference_span = child.text_range();
                qualified_name(child).map(|path| (path, reference_span))
            })
        else {
            return Self::unknown_value();
        };

        let outcome = self.resolver.class_all(&path);
        let value = match &outcome {
            Resolution::Found(value) => value.clone(),
            Resolution::Missing
            | Resolution::UnderResolved(_)
            | Resolution::Ambiguous(_)
            | Resolution::Cycle(_) => Self::unknown_value(),
        };
        self.sites.push(LocalResolutionSite::new(
            node.text_range(),
            reference_span,
            LocalResolution::ClassAll(outcome),
        ));
        value
    }

    fn evaluate_property_navigation(
        &mut self,
        node: &GreenNode,
        value: LocalValue,
        variable_source: Option<Name>,
        environment: &mut impl LocalBindings,
    ) -> LocalValue {
        self.evaluate_call_arguments(node, environment);
        let Some((step, reference_span)) = navigation_step(node) else {
            return Self::unknown_value();
        };

        let outcome = match variable_source {
            Some(name) => match environment.lookup(&name) {
                Some(source) => self.resolver.resolve(source, std::slice::from_ref(&step)),
                None => NavigationResolution::UnderResolved(
                    NavigationUnderResolution::UnboundVariable { name },
                ),
            },
            None => self.resolver.resolve(&value, std::slice::from_ref(&step)),
        };
        let value = navigation_value(&outcome).unwrap_or_else(Self::unknown_value);
        self.sites.push(LocalResolutionSite::new(
            node.text_range(),
            reference_span,
            LocalResolution::Navigation(outcome),
        ));
        value
    }

    fn evaluate_arrow_call(
        &mut self,
        node: &GreenNode,
        incoming: LocalValue,
        environment: &mut impl LocalBindings,
    ) -> LocalValue {
        let name = direct_nodes(node)
            .into_iter()
            .find(|child| child.kind() == SyntaxKind::QUALIFIED_NAME)
            .and_then(qualified_name)
            .map(|path| path.simple_name().to_owned());
        let call_arguments = direct_nodes(node)
            .into_iter()
            .find(|child| child.kind() == SyntaxKind::CALL_ARGS);
        let Some(call_arguments) = call_arguments else {
            return Self::unknown_value();
        };

        let arguments = direct_nodes(&call_arguments);
        let lambda_nodes = arguments
            .iter()
            .filter(|child| child.kind() == SyntaxKind::LAMBDA_EXPR)
            .cloned()
            .collect::<Vec<_>>();
        let non_lambda_nodes = arguments
            .into_iter()
            .filter(|child| child.kind() != SyntaxKind::LAMBDA_EXPR)
            .collect::<Vec<_>>();
        let _ = self.evaluate_nodes(&non_lambda_nodes, environment);

        let mut lambda_result = Self::unknown_value();
        for lambda in lambda_nodes {
            lambda_result = self.evaluate_lambda(&lambda, incoming.clone(), environment);
        }

        match name.as_deref() {
            Some("filter") => incoming,
            Some("map") => lambda_result,
            _ => Self::unknown_value(),
        }
    }

    fn evaluate_function_call(&mut self, node: &GreenNode, environment: &mut impl LocalBindings) {
        for child in direct_nodes(node) {
            if child.kind() == SyntaxKind::CALL_ARGS {
                self.evaluate_call_arguments(&child, environment);
            } else if child.kind() != SyntaxKind::QUALIFIED_NAME {
                let _ = self.evaluate_node(&child, environment);
            }
        }
    }

    fn evaluate_call_arguments(&mut self, node: &GreenNode, environment: &mut impl LocalBindings) {
        let nodes = direct_nodes(node)
            .into_iter()
            .filter(|child| child.kind() != SyntaxKind::LAMBDA_EXPR)
            .collect::<Vec<_>>();
        let _ = self.evaluate_nodes(&nodes, environment);
    }

    fn evaluate_lambda(
        &mut self,
        node: &GreenNode,
        incoming: LocalValue,
        environment: &mut impl LocalBindings,
    ) -> LocalValue {
        let mut scope = environment.scope();
        if let Some(parameters) = direct_nodes(node)
            .into_iter()
            .find(|child| child.kind() == SyntaxKind::LAMBDA_PARAMS)
        {
            let relation_rows = typed_relation_rows(&parameters, incoming.multiplicity());
            let parameter_names = lambda_parameter_names(&parameters);
            let mut seen_names = BTreeSet::new();
            let mut duplicate_names = BTreeSet::new();
            for name in &parameter_names {
                if !seen_names.insert(name.clone()) {
                    let _ = duplicate_names.insert(name.clone());
                }
            }
            for (index, name) in parameter_names.into_iter().enumerate() {
                let value = if duplicate_names.contains(&name) {
                    Self::unknown_value()
                } else {
                    match relation_rows.get(index) {
                        Some(RelationParameterBinding::Row(row)) => row.clone(),
                        Some(RelationParameterBinding::Invalid) => Self::unknown_value(),
                        Some(RelationParameterBinding::NotRelation) | None if index == 0 => {
                            incoming.clone()
                        }
                        Some(RelationParameterBinding::NotRelation) | None => Self::unknown_value(),
                    }
                };
                scope.bind(name, value);
            }
        }

        let Some(block) = direct_nodes(node)
            .into_iter()
            .find(|child| child.kind() == SyntaxKind::CODE_BLOCK)
        else {
            return Self::unknown_value();
        };
        self.evaluate_code_block(&block, &mut scope)
    }

    fn evaluate_code_block(
        &mut self,
        node: &GreenNode,
        environment: &mut impl LocalBindings,
    ) -> LocalValue {
        let mut value = Self::unknown_value();
        for child in direct_nodes(node) {
            value = match child.kind() {
                SyntaxKind::LET_STMT => self.evaluate_let(&child, environment),
                _ => self.evaluate_node(&child, environment),
            };
        }
        value
    }

    fn evaluate_let(
        &mut self,
        node: &GreenNode,
        environment: &mut impl LocalBindings,
    ) -> LocalValue {
        let value = self.evaluate_nodes(&direct_nodes(node), environment);
        if let Some(name) = let_binding_name(node) {
            environment.bind(name, value.clone());
        }
        value
    }
}

fn qualified_name(node: GreenNode) -> Option<QName> {
    QName::new(compact_text(&node)).ok()
}

fn variable_name(node: &GreenNode) -> Option<Name> {
    direct_name_after(node, SyntaxKind::DOLLAR)
}

fn let_binding_name(node: &GreenNode) -> Option<Name> {
    direct_name_after(node, SyntaxKind::LET_KW)
}

fn direct_name_after(node: &GreenNode, marker: SyntaxKind) -> Option<Name> {
    direct_name_after_with_span(node, marker).map(|(name, _)| name)
}

fn direct_name_after_with_span(node: &GreenNode, marker: SyntaxKind) -> Option<(Name, TextRange)> {
    let mut found_marker = false;
    for element in node.children() {
        let Some(token) = element.as_token() else {
            continue;
        };
        if token.kind() == marker {
            found_marker = true;
            continue;
        }
        if found_marker && NAME_KINDS.contains(&token.kind()) {
            return Name::new(token.text())
                .ok()
                .map(|name| (name, token.text_range()));
        }
    }
    None
}

fn navigation_step(node: &GreenNode) -> Option<(NavigationStep, TextRange)> {
    let (name, reference_span) = direct_name_after_with_span(node, SyntaxKind::DOT)?;
    let arguments = direct_nodes(node)
        .into_iter()
        .find(|child| child.kind() == SyntaxKind::CALL_ARGS)
        .map_or(0, |arguments| call_argument_count(&arguments));
    Some((
        if arguments == 0 {
            NavigationStep::property(name)
        } else {
            NavigationStep::call(name, arguments)
        },
        reference_span,
    ))
}

fn call_argument_count(node: &GreenNode) -> usize {
    let has_expression = node
        .children()
        .iter()
        .any(|element| element.as_node().is_some());
    if !has_expression {
        return 0;
    }
    let commas = node
        .children()
        .iter()
        .filter_map(GreenElement::as_token)
        .filter(|token| token.kind() == SyntaxKind::COMMA)
        .count();
    commas.saturating_add(1)
}

fn navigation_value(outcome: &NavigationResolution) -> Option<LocalValue> {
    match outcome {
        NavigationResolution::Found(chain) => Some(chain.value().clone()),
        NavigationResolution::Missing(_)
        | NavigationResolution::UnderResolved(_)
        | NavigationResolution::Ambiguous(_)
        | NavigationResolution::Cycle(_)
        | NavigationResolution::WrongArity(_) => None,
    }
}

fn lambda_parameter_names(node: &GreenNode) -> Vec<Name> {
    node.children()
        .iter()
        .filter_map(GreenElement::as_token)
        .filter(|token| NAME_KINDS.contains(&token.kind()))
        .filter_map(|token| Name::new(token.text()).ok())
        .collect()
}

fn typed_relation_rows(
    node: &GreenNode,
    multiplicity: Multiplicity,
) -> Vec<RelationParameterBinding> {
    let mut has_parameter = false;
    let mut binding = RelationParameterBinding::NotRelation;
    let mut rows = Vec::new();
    for element in node.children() {
        match element {
            GreenElement::Token(token) if token.kind() == SyntaxKind::COMMA => {
                if has_parameter {
                    rows.push(binding);
                    has_parameter = false;
                    binding = RelationParameterBinding::NotRelation;
                }
            }
            GreenElement::Token(token) if NAME_KINDS.contains(&token.kind()) => {
                has_parameter = Name::new(token.text()).is_ok();
            }
            GreenElement::Node(type_reference) if type_reference.kind() == SyntaxKind::TYPE_REF => {
                binding = match (has_parameter, is_named_relation_type(type_reference)) {
                    (true, true) => is_relation_type(type_reference)
                        .then(|| relation_row_value(type_reference, multiplicity))
                        .flatten()
                        .map_or(
                            RelationParameterBinding::Invalid,
                            RelationParameterBinding::Row,
                        ),
                    _ => binding,
                };
            }
            _ => {}
        }
    }
    if has_parameter {
        rows.push(binding);
    }
    rows
}

fn is_named_relation_type(node: &GreenNode) -> bool {
    let paths = direct_nodes(node)
        .into_iter()
        .filter(|child| child.kind() == SyntaxKind::QUALIFIED_NAME)
        .filter_map(qualified_name)
        .collect::<Vec<_>>();
    let [path] = paths.as_slice() else {
        return false;
    };
    path.as_str() == "Relation"
}

fn is_relation_type(node: &GreenNode) -> bool {
    !contains_error_node(node)
        && is_named_relation_type(node)
        && direct_nodes(node)
            .iter()
            .filter(|child| child.kind() == SyntaxKind::RELATION_TYPE)
            .count()
            == 1
}

fn relation_row_value(node: &GreenNode, multiplicity: Multiplicity) -> Option<LocalValue> {
    let relations = direct_nodes(node)
        .into_iter()
        .filter(|child| child.kind() == SyntaxKind::RELATION_TYPE)
        .collect::<Vec<_>>();
    let [relation] = relations.as_slice() else {
        return None;
    };
    if contains_error_node(relation) {
        return None;
    }
    let columns = direct_nodes(relation)
        .into_iter()
        .filter(|column| column.kind() == SyntaxKind::COLUMN_INFO)
        .enumerate()
        .map(|(index, column)| {
            relation_column(column, RelationColumnId::new(u32::try_from(index).ok()?))
        })
        .collect::<Option<Vec<_>>>()?;
    Some(LocalValue::relation_row(
        RelationRow::new(columns).ok()?,
        multiplicity,
    ))
}

fn relation_column(node: GreenNode, id: RelationColumnId) -> Option<RelationColumn> {
    if contains_error_node(&node) {
        return None;
    }
    let names = node
        .children()
        .iter()
        .filter_map(GreenElement::as_token)
        .filter(|token| NAME_KINDS.contains(&token.kind()))
        .filter_map(|token| Name::new(token.text()).ok())
        .collect::<Vec<_>>();
    let [name] = names.as_slice() else {
        return None;
    };
    let type_references = direct_nodes(&node)
        .into_iter()
        .filter(|child| child.kind() == SyntaxKind::TYPE_REF)
        .collect::<Vec<_>>();
    let [type_reference] = type_references.as_slice() else {
        return None;
    };
    let multiplicities = direct_nodes(&node)
        .into_iter()
        .filter(|child| child.kind() == SyntaxKind::MULTIPLICITY)
        .collect::<Vec<_>>();
    let [multiplicity] = multiplicities.as_slice() else {
        return None;
    };
    Some(RelationColumn::new(
        id,
        name.clone(),
        type_ref(type_reference)?,
        multiplicity_from_node(multiplicity)?,
        declaration_span(&node)?,
    ))
}

fn type_ref(node: &GreenNode) -> Option<TypeRef> {
    if node.kind() != SyntaxKind::TYPE_REF || contains_error_node(node) {
        return None;
    }
    let paths = direct_nodes(node)
        .into_iter()
        .filter(|child| child.kind() == SyntaxKind::QUALIFIED_NAME)
        .filter_map(qualified_name)
        .collect::<Vec<_>>();
    let [path] = paths.as_slice() else {
        return None;
    };
    let arguments = direct_nodes(node)
        .into_iter()
        .filter(|child| child.kind() == SyntaxKind::TYPE_REF)
        .map(|argument| type_ref(&argument))
        .collect::<Option<Vec<_>>>()?;
    Some(TypeRef::new(path.clone(), arguments))
}

fn multiplicity_from_node(node: &GreenNode) -> Option<Multiplicity> {
    if node.kind() != SyntaxKind::MULTIPLICITY || contains_error_node(node) {
        return None;
    }
    let text = compact_text(node);
    let body = text.strip_prefix('[')?.strip_suffix(']')?;
    if body == "*" {
        return Multiplicity::new(0, None).ok();
    }
    if let Some((lower, upper)) = body.split_once("..") {
        if upper.contains("..") {
            return None;
        }
        let lower = parse_multiplicity_bound(lower)?;
        let upper = if upper == "*" {
            None
        } else {
            Some(parse_multiplicity_bound(upper)?)
        };
        return Multiplicity::new(lower, upper).ok();
    }
    let bound = parse_multiplicity_bound(body)?;
    Multiplicity::new(bound, Some(bound)).ok()
}

fn parse_multiplicity_bound(text: &str) -> Option<u32> {
    text.bytes()
        .all(|byte| byte.is_ascii_digit())
        .then(|| text.parse().ok())
        .flatten()
}

fn compact_text(node: &GreenNode) -> String {
    node.tokens()
        .filter(|token| !is_trivia(token.kind()))
        .map(|token| token.text())
        .collect()
}

fn declaration_span(node: &GreenNode) -> Option<TextRange> {
    let mut tokens = node.tokens().filter(|token| !is_trivia(token.kind()));
    let first = tokens.next()?;
    let start = first.text_range().start();
    let end = tokens.last().map_or_else(
        || first.text_range().end(),
        |token| token.text_range().end(),
    );
    Some(TextRange::new(start, end))
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use pure_analyzer_lexer::lex;
    use pure_analyzer_syntax::GreenNodeBuilder;

    use super::*;

    #[test]
    fn relation_column_rejects_a_hidden_recovery_node() {
        let source = "name:String[1]";
        let tokens = lex(source);
        let mut builder = GreenNodeBuilder::new(source, &tokens);
        builder.open(SyntaxKind::ROOT);
        builder.open(SyntaxKind::COLUMN_INFO);
        builder.advance();
        builder.advance();
        builder.open(SyntaxKind::TYPE_REF);
        builder.open(SyntaxKind::QUALIFIED_NAME);
        builder.advance();
        builder.close();
        builder.close();
        builder.open(SyntaxKind::MULTIPLICITY);
        builder.advance();
        builder.advance();
        builder.advance();
        builder.close();
        builder.open(SyntaxKind::ERROR_NODE);
        builder.close();
        builder.close();
        builder.close();
        let root = builder.finish().expect("fixture tree must build");
        let column = direct_nodes(&root)
            .into_iter()
            .next()
            .expect("fixture must contain a column");

        assert_eq!(
            relation_column(column, RelationColumnId::new(0)),
            None,
            "a recovered column must not become a partial relation binding"
        );
    }

    #[test]
    fn multiplicity_rejects_a_hidden_recovery_node() {
        let source = "[1]";
        let tokens = lex(source);
        let mut builder = GreenNodeBuilder::new(source, &tokens);
        builder.open(SyntaxKind::ROOT);
        builder.open(SyntaxKind::MULTIPLICITY);
        builder.advance();
        builder.advance();
        builder.advance();
        builder.open(SyntaxKind::ERROR_NODE);
        builder.close();
        builder.close();
        builder.close();
        let root = builder.finish().expect("fixture tree must build");
        let multiplicity = direct_nodes(&root)
            .into_iter()
            .next()
            .expect("fixture must contain a multiplicity");

        assert_eq!(
            multiplicity_from_node(&multiplicity),
            None,
            "a recovered multiplicity must not become a partial relation binding"
        );
    }

    #[test]
    fn malformed_relation_type_is_an_invalid_parameter_binding() {
        let parameters = relation_parameters_fixture(SyntaxKind::TYPE_REF, true, true);

        assert!(matches!(
            typed_relation_rows(&parameters, Multiplicity::zero_or_more()).as_slice(),
            [RelationParameterBinding::Invalid]
        ));
    }

    #[test]
    fn generic_relation_type_is_not_a_relation_row_type() {
        let source = "Relation<String>";
        let tokens = lex(source);
        let mut builder = GreenNodeBuilder::new(source, &tokens);
        builder.open(SyntaxKind::ROOT);
        builder.open(SyntaxKind::TYPE_REF);
        builder.open(SyntaxKind::QUALIFIED_NAME);
        builder.advance();
        builder.close();
        builder.advance();
        builder.open(SyntaxKind::TYPE_REF);
        builder.open(SyntaxKind::QUALIFIED_NAME);
        builder.advance();
        builder.close();
        builder.close();
        builder.advance();
        builder.close();
        builder.close();
        let root = builder.finish().expect("fixture tree must build");
        let type_reference = direct_nodes(&root)
            .into_iter()
            .next()
            .expect("fixture must contain a type reference");

        assert!(!is_relation_type(&type_reference));
    }

    #[test]
    fn bare_relation_type_is_not_a_relation_row_type() {
        let source = "Relation";
        let tokens = lex(source);
        let mut builder = GreenNodeBuilder::new(source, &tokens);
        builder.open(SyntaxKind::ROOT);
        builder.open(SyntaxKind::TYPE_REF);
        builder.open(SyntaxKind::QUALIFIED_NAME);
        builder.advance();
        builder.close();
        builder.close();
        builder.close();
        let root = builder.finish().expect("fixture tree must build");
        let type_reference = direct_nodes(&root)
            .into_iter()
            .next()
            .expect("fixture must contain a type reference");

        assert!(!is_relation_type(&type_reference));
    }

    fn simple_type_ref_fixture(source: &str) -> GreenNode {
        let tokens = lex(source);
        let mut builder = GreenNodeBuilder::new(source, &tokens);
        builder.open(SyntaxKind::ROOT);
        builder.open(SyntaxKind::TYPE_REF);
        builder.open(SyntaxKind::QUALIFIED_NAME);
        builder.advance();
        builder.close();
        builder.close();
        builder.close();
        let root = builder.finish().expect("fixture tree must build");
        direct_nodes(&root)
            .into_iter()
            .next()
            .expect("fixture must contain a type reference")
    }

    /// Regression for a `is_named_relation_type -> true` mutant: the guard
    /// must reject any other qualified name, not treat every named type as
    /// `Relation`.
    #[test]
    fn is_named_relation_type_requires_the_literal_name_relation() {
        assert!(is_named_relation_type(&simple_type_ref_fixture("Relation")));
        assert!(
            !is_named_relation_type(&simple_type_ref_fixture("String")),
            "a type named anything other than Relation must not be treated as one"
        );
    }

    /// Regression for a `||` -> `&&` mutant on `type_ref`'s guard: a
    /// well-formed qualified name inside a hidden recovery node must still be
    /// rejected, exactly like `relation_column_rejects_a_hidden_recovery_node`
    /// pins for the sibling `relation_column` guard.
    #[test]
    fn type_ref_rejects_a_hidden_recovery_node() {
        let source = "String";
        let tokens = lex(source);
        let mut builder = GreenNodeBuilder::new(source, &tokens);
        builder.open(SyntaxKind::ROOT);
        builder.open(SyntaxKind::TYPE_REF);
        builder.open(SyntaxKind::QUALIFIED_NAME);
        builder.advance();
        builder.close();
        builder.open(SyntaxKind::ERROR_NODE);
        builder.close();
        builder.close();
        builder.close();
        let root = builder.finish().expect("fixture tree must build");
        let recovered_type = direct_nodes(&root)
            .into_iter()
            .next()
            .expect("fixture must contain a type reference");

        assert_eq!(
            type_ref(&recovered_type),
            None,
            "a type reference containing a hidden recovery node must not resolve, \
             even though its qualified name alone is well-formed"
        );
    }

    #[test]
    fn relation_shaped_non_type_parameter_is_not_bound() {
        let parameters = relation_parameters_fixture(SyntaxKind::PAREN_EXPR, false, true);

        assert!(matches!(
            typed_relation_rows(&parameters, Multiplicity::zero_or_more()).as_slice(),
            [RelationParameterBinding::NotRelation]
        ));
    }

    #[test]
    fn non_name_tokens_do_not_introduce_relation_parameters() {
        let parameters = relation_parameters_fixture(SyntaxKind::TYPE_REF, false, false);

        assert!(typed_relation_rows(&parameters, Multiplicity::zero_or_more()).is_empty());
    }

    fn relation_parameters_fixture(
        outer_kind: SyntaxKind,
        outer_error: bool,
        has_name_token: bool,
    ) -> GreenNode {
        let source = if has_name_token {
            "row:Relation<(name:String[1])>"
        } else {
            "(:Relation<(name:String[1])>"
        };
        let tokens = lex(source);
        let mut builder = GreenNodeBuilder::new(source, &tokens);
        builder.open(SyntaxKind::ROOT);
        builder.open(SyntaxKind::LAMBDA_PARAMS);
        builder.advance();
        builder.advance();
        builder.open(outer_kind);
        builder.open(SyntaxKind::QUALIFIED_NAME);
        builder.advance();
        builder.close();
        builder.advance();
        builder.open(SyntaxKind::RELATION_TYPE);
        builder.advance();
        builder.open(SyntaxKind::COLUMN_INFO);
        builder.advance();
        builder.advance();
        builder.open(SyntaxKind::TYPE_REF);
        builder.open(SyntaxKind::QUALIFIED_NAME);
        builder.advance();
        builder.close();
        builder.close();
        builder.open(SyntaxKind::MULTIPLICITY);
        builder.advance();
        builder.advance();
        builder.advance();
        builder.close();
        builder.close();
        builder.advance();
        builder.close();
        builder.advance();
        if outer_error {
            builder.open(SyntaxKind::ERROR_NODE);
            builder.close();
        }
        builder.close();
        builder.close();
        builder.close();
        let root = builder.finish().expect("fixture tree must build");

        direct_nodes(&root)
            .into_iter()
            .next()
            .expect("fixture must contain lambda parameters")
    }
}
