//! Local navigation analysis over the supported M3 concrete syntax tree.

use std::collections::BTreeMap;

use pure_analyzer_model::{ModelGraph, Multiplicity, Name, QName, TypeRef};
use pure_analyzer_resolve::{
    LocalValue, NavigationResolution, NavigationResolver, NavigationStep,
    NavigationUnderResolution, RelationRow, Resolution, TypeEnvironment, TypeScope, UnknownValue,
};
use pure_analyzer_syntax::{GreenElement, GreenNode, SyntaxKind, TextRange};

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
    outcome: LocalResolution,
}

impl LocalResolutionSite {
    fn new(span: TextRange, outcome: LocalResolution) -> Self {
        Self { span, outcome }
    }

    /// Return the exact syntax span that produced this result.
    #[must_use]
    pub const fn span(&self) -> TextRange {
        self.span
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
    let _ = analyzer.evaluate_node(tree, &mut environment);
    LocalNavigationAnalysis {
        sites: analyzer.sites,
    }
}

#[derive(Debug)]
struct LocalAnalyzer<'model> {
    resolver: NavigationResolver<'model>,
    sites: Vec<LocalResolutionSite>,
}

trait LocalBindings {
    fn bind(&mut self, name: Name, value: LocalValue);
    fn lookup(&self, name: &Name) -> Option<&LocalValue>;
    fn scope(&mut self) -> impl LocalBindings + '_;
}

impl LocalBindings for TypeEnvironment {
    fn bind(&mut self, name: Name, value: LocalValue) {
        let _ = TypeEnvironment::bind(self, name, value);
    }

    fn lookup(&self, name: &Name) -> Option<&LocalValue> {
        TypeEnvironment::lookup(self, name)
    }

    fn scope(&mut self) -> impl LocalBindings + '_ {
        TypeEnvironment::scope(self)
    }
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
            SyntaxKind::CODE_BLOCK => self.evaluate_code_block(node, environment),
            SyntaxKind::LAMBDA_EXPR => {
                self.evaluate_lambda(node, Self::unknown_value(), environment)
            }
            SyntaxKind::LET_STMT => self.evaluate_let(node, environment),
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
                SyntaxKind::LET_STMT => {
                    value = self.evaluate_let(node, environment);
                    variable_source = None;
                }
                SyntaxKind::ERROR_NODE | SyntaxKind::ISLAND | SyntaxKind::OPAQUE_ISLAND => {
                    value = Self::unknown_value();
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
        let Some(path) = direct_nodes(node)
            .into_iter()
            .find(|child| child.kind() == SyntaxKind::QUALIFIED_NAME)
            .and_then(qualified_name)
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
        let Some(step) = navigation_step(node) else {
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
            let relation_row = relation_row_value(&parameters, incoming.multiplicity());
            for (index, name) in lambda_parameter_names(&parameters).into_iter().enumerate() {
                let value = if index == 0 {
                    relation_row.clone().unwrap_or_else(|| incoming.clone())
                } else {
                    Self::unknown_value()
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
                SyntaxKind::QUERY_EXPR => self.evaluate_nodes(&direct_nodes(&child), environment),
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

fn direct_nodes(node: &GreenNode) -> Vec<GreenNode> {
    node.children()
        .iter()
        .filter_map(GreenElement::as_node)
        .cloned()
        .collect()
}

fn qualified_name(node: GreenNode) -> Option<QName> {
    QName::new(node.text()).ok()
}

fn variable_name(node: &GreenNode) -> Option<Name> {
    direct_name_after(node, SyntaxKind::DOLLAR)
}

fn let_binding_name(node: &GreenNode) -> Option<Name> {
    direct_name_after(node, SyntaxKind::LET_KW)
}

fn direct_name_after(node: &GreenNode, marker: SyntaxKind) -> Option<Name> {
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
            return Name::new(token.text()).ok();
        }
    }
    None
}

fn navigation_step(node: &GreenNode) -> Option<NavigationStep> {
    let name = direct_name_after(node, SyntaxKind::DOT)?;
    let arguments = direct_nodes(node)
        .into_iter()
        .find(|child| child.kind() == SyntaxKind::CALL_ARGS)
        .map_or(0, |arguments| call_argument_count(&arguments));
    Some(if arguments == 0 {
        NavigationStep::property(name)
    } else {
        NavigationStep::call(name, arguments)
    })
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

fn relation_row_value(node: &GreenNode, multiplicity: Multiplicity) -> Option<LocalValue> {
    let relation = find_descendant(node, SyntaxKind::RELATION_TYPE)?;
    let columns = direct_nodes(&relation)
        .into_iter()
        .filter(|column| column.kind() == SyntaxKind::COLUMN_INFO)
        .filter_map(relation_column)
        .collect::<BTreeMap<_, _>>();
    Some(LocalValue::relation_row(
        RelationRow::new(columns),
        multiplicity,
    ))
}

fn relation_column(node: GreenNode) -> Option<(Name, LocalValue)> {
    let name = direct_name(&node)?;
    let type_reference = find_descendant(&node, SyntaxKind::TYPE_REF)?;
    let path =
        find_descendant(&type_reference, SyntaxKind::QUALIFIED_NAME).and_then(qualified_name)?;
    let value = LocalValue::unknown(
        UnknownValue::UnmodeledType(TypeRef::new(path, Vec::new())),
        Multiplicity::zero_or_more(),
    );
    Some((name, value))
}

fn direct_name(node: &GreenNode) -> Option<Name> {
    node.children()
        .iter()
        .filter_map(GreenElement::as_token)
        .find(|token| NAME_KINDS.contains(&token.kind()))
        .and_then(|token| Name::new(token.text()).ok())
}

fn find_descendant(node: &GreenNode, kind: SyntaxKind) -> Option<GreenNode> {
    direct_nodes(node).into_iter().find_map(|child| {
        (child.kind() == kind)
            .then_some(child.clone())
            .or_else(|| find_descendant(&child, kind))
    })
}
