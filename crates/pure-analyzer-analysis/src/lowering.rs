//! Conservative lowering of the minimal resolved M3 query subset.

use pure_analyzer_diagnostics::{FileId, ReasonCode};
use pure_analyzer_model::{Multiplicity, Name, QName, TypeRef};
use pure_analyzer_resolve::{
    LocalValue, LocalValueKind, NavigationResolution, NavigationResolver, NavigationStep,
    NavigationTarget, Resolution,
};
use pure_analyzer_syntax::{GreenElement, GreenNode, SyntaxKind, TextRange};

use crate::{
    AnalysisInput, Column, ColumnId, IrOrigin, Knowledge, ModelOrigin, Nullability, OpaqueOutcome,
    Projection, RelationExpression, RelationFacts, RelationOperator, RelationSchema,
    RelationSource, RelationalOutcome, RelationalQuery, ResolvedNavigation, ScalarExpression,
    ScalarLiteral, ScalarOperator, SourceSpan, Totality,
    relational::compose_navigation_multiplicity,
};

const BOOLEAN_TYPE: &str = "Boolean";
const INTEGER_TYPE: &str = "Integer";
const MAP_VALUE_NAME: &str = "value";
const ONE: u32 = 1;
const STRING_TYPE: &str = "String";

/// Lower one parsed M3 query into the proven relational core or a typed opaque outcome.
///
/// The input must contain exactly one top-level query expression and a model graph. The
/// supported subset is deliberately limited to `Class.all()`, to-one resolved navigation,
/// `->filter` with an equality predicate, and one-lambda `->map`. Other valid syntax stays
/// explicit as an opaque outcome rather than being approximated.
#[must_use]
pub fn lower_m3_query(input: AnalysisInput<'_, '_>) -> RelationalOutcome {
    let fallback_origin = origin(input.file(), input.tree().text_range(), Vec::new());
    if !input.parse_diagnostics().is_empty() || contains_error_node(input.tree()) {
        return opaque(ReasonCode::IndUnparseable, fallback_origin);
    }

    let queries = top_level_queries(input.tree());
    let [query] = queries.as_slice() else {
        return opaque(ReasonCode::IndUnmodeledOp, fallback_origin);
    };
    let Some(model) = input.model() else {
        return opaque(
            ReasonCode::ModelIncomplete,
            origin(input.file(), query.text_range(), Vec::new()),
        );
    };

    let mut lowerer = QueryLowerer::new(input.file(), model, query.text_range());
    match lowerer.lower_query(query) {
        Ok(expression) => RelationalOutcome::supported(RelationalQuery::new(expression)),
        Err(reason) => opaque(reason, lowerer.failure_origin()),
    }
}

#[derive(Debug, Clone)]
struct BoundColumn {
    column: Column,
    local: LocalValue,
}

#[derive(Debug, Clone)]
struct RelationState {
    expression: RelationExpression,
    element: BoundColumn,
}

#[derive(Debug, Clone)]
struct LoweredScalar {
    expression: ScalarExpression,
    local: LocalValue,
}

#[derive(Debug, Clone)]
struct NavigationValue {
    name: Name,
    scalar: LoweredScalar,
}

#[derive(Debug)]
struct QueryLowerer<'model> {
    file: FileId,
    navigation: NavigationResolver<'model>,
    next_column: u32,
    failure_origin: IrOrigin,
}

impl<'model> QueryLowerer<'model> {
    fn new(
        file: FileId,
        model: &'model pure_analyzer_model::ModelGraph,
        fallback_range: TextRange,
    ) -> Self {
        Self {
            file,
            navigation: NavigationResolver::new(model),
            next_column: 0,
            failure_origin: origin(file, fallback_range, Vec::new()),
        }
    }

    fn failure_origin(&self) -> IrOrigin {
        self.failure_origin.clone()
    }

    fn mark_failure(&mut self, node: &GreenNode) {
        self.failure_origin = origin(self.file, node.text_range(), Vec::new());
    }

    fn mark_failure_with_origins(
        &mut self,
        node: &GreenNode,
        origins: &[&IrOrigin],
        extra_model_origins: &[ModelOrigin],
    ) {
        self.failure_origin =
            merged_origin_with_models(self.file, node.text_range(), origins, extra_model_origins);
    }

    fn lower_query(&mut self, query: &GreenNode) -> Result<RelationExpression, ReasonCode> {
        self.mark_failure(query);
        self.lower_relation_nodes(&direct_nodes(query))
            .map(|state| state.expression)
    }

    fn lower_relation_nodes(&mut self, nodes: &[GreenNode]) -> Result<RelationState, ReasonCode> {
        let mut state = None;
        for node in nodes {
            self.mark_failure(node);
            match node.kind() {
                SyntaxKind::ALL_EXPR => {
                    if state.is_some() {
                        return Err(ReasonCode::IndUnmodeledOp);
                    }
                    state = Some(self.lower_all(node)?);
                }
                SyntaxKind::PROPERTY_NAV => {
                    state = Some(self.project_navigation(require_relation(state)?, node)?);
                }
                SyntaxKind::ARROW_CALL => {
                    state = Some(self.lower_arrow(require_relation(state)?, node)?);
                }
                _ => return Err(ReasonCode::IndUnmodeledOp),
            }
        }
        require_relation(state)
    }

    fn lower_all(&mut self, node: &GreenNode) -> Result<RelationState, ReasonCode> {
        self.mark_failure(node);
        let class_path = all_class_path(node).ok_or(ReasonCode::IndUnmodeledOp)?;
        let class = match self.navigation.resolver().resolve_class(&class_path) {
            Resolution::Found(class) => class,
            Resolution::Missing => return Err(ReasonCode::IndUnresolvedSchema),
            Resolution::UnderResolved(_) | Resolution::Ambiguous(_) | Resolution::Cycle(_) => {
                return Err(ReasonCode::ModelIncomplete);
            }
        };
        let multiplicity = exactly_one()?;
        let column_origin = origin(
            self.file,
            node.text_range(),
            vec![ModelOrigin::from_class(&class)],
        );
        let column = Column::new(
            self.next_column()?,
            Name::new(class.path().simple_name()).map_err(|_| ReasonCode::IndUnmodeledOp)?,
            TypeRef::new(class.path().clone(), Vec::new()),
            multiplicity,
            Nullability::Unknown,
            column_origin.clone(),
        );
        let schema =
            RelationSchema::new(vec![column.clone()]).map_err(|_| ReasonCode::IndUnmodeledOp)?;
        let expression = RelationExpression::new(
            RelationOperator::Scan(RelationSource::Class(class.clone())),
            schema,
            RelationFacts::unknown(),
            column_origin,
        )
        .map_err(|_| ReasonCode::IndUnmodeledOp)?;
        Ok(RelationState {
            expression,
            element: BoundColumn {
                column,
                local: LocalValue::class(class, multiplicity),
            },
        })
    }

    fn lower_arrow(
        &mut self,
        state: RelationState,
        node: &GreenNode,
    ) -> Result<RelationState, ReasonCode> {
        self.mark_failure_with_origins(node, &[state.expression.origin()], &[]);
        let name = arrow_name(node).ok_or(ReasonCode::IndUnmodeledOp)?;
        let lambda = arrow_lambda(node).ok_or(ReasonCode::IndUnmodeledOp)?;
        match name.as_str() {
            "filter" => self.lower_filter(state, node, lambda),
            "map" => self.lower_map(state, node, lambda),
            _ => Err(ReasonCode::IndUnmodeledOp),
        }
    }

    fn lower_filter(
        &mut self,
        state: RelationState,
        node: &GreenNode,
        lambda: LambdaBody,
    ) -> Result<RelationState, ReasonCode> {
        self.mark_failure_with_origins(node, &[state.expression.origin()], &[]);
        let predicate = self
            .lower_scalar_query(&lambda.body, &state.element, &lambda.parameter)
            .map_err(predicate_reason)?;
        let schema = state.expression.schema().clone();
        let facts = state.expression.facts().clone();
        let operator_origin = merged_origin(
            self.file,
            node.text_range(),
            &[state.expression.origin(), predicate.expression.origin()],
        );
        let expression = RelationExpression::new(
            RelationOperator::Filter {
                input: Box::new(state.expression),
                predicate: predicate.expression,
            },
            schema,
            facts,
            operator_origin,
        )
        .map_err(|_| ReasonCode::IndOpaquePredicate)?;
        Ok(RelationState {
            expression,
            element: state.element,
        })
    }

    fn lower_map(
        &mut self,
        state: RelationState,
        node: &GreenNode,
        lambda: LambdaBody,
    ) -> Result<RelationState, ReasonCode> {
        self.mark_failure_with_origins(node, &[state.expression.origin()], &[]);
        let scalar = self
            .lower_scalar_query(&lambda.body, &state.element, &lambda.parameter)
            .map_err(operator_reason)?;
        let name = Name::new(MAP_VALUE_NAME).map_err(|_| ReasonCode::IndUnmodeledOp)?;
        self.project(state, node, name, scalar)
    }

    fn project_navigation(
        &mut self,
        state: RelationState,
        node: &GreenNode,
    ) -> Result<RelationState, ReasonCode> {
        self.mark_failure_with_origins(node, &[state.expression.origin()], &[]);
        let input = column_scalar(&state.element);
        let navigation = self.lower_navigation(node, input)?;
        self.project(state, node, navigation.name, navigation.scalar)
    }

    fn project(
        &mut self,
        state: RelationState,
        node: &GreenNode,
        name: Name,
        scalar: LoweredScalar,
    ) -> Result<RelationState, ReasonCode> {
        self.mark_failure_with_origins(
            node,
            &[state.expression.origin(), scalar.expression.origin()],
            &[],
        );
        let column = Column::new(
            self.next_column()?,
            name,
            scalar.expression.type_ref().clone(),
            scalar.expression.multiplicity(),
            scalar.expression.nullability(),
            scalar.expression.origin().clone(),
        );
        let schema =
            RelationSchema::new(vec![column.clone()]).map_err(|_| ReasonCode::IndUnmodeledOp)?;
        let operator_origin = merged_origin(
            self.file,
            node.text_range(),
            &[state.expression.origin(), scalar.expression.origin()],
        );
        let expression = RelationExpression::new(
            RelationOperator::Project {
                input: Box::new(state.expression),
                projections: vec![Projection::new(column.id(), scalar.expression)],
            },
            schema,
            RelationFacts::unknown(),
            operator_origin,
        )
        .map_err(|_| ReasonCode::IndUnmodeledOp)?;
        Ok(RelationState {
            expression,
            element: BoundColumn {
                column,
                local: scalar.local,
            },
        })
    }

    fn lower_scalar_query(
        &mut self,
        query: &GreenNode,
        binding: &BoundColumn,
        parameter: &Name,
    ) -> Result<LoweredScalar, ReasonCode> {
        self.mark_failure(query);
        if query.kind() != SyntaxKind::QUERY_EXPR {
            return Err(ReasonCode::IndOpaquePredicate);
        }
        self.lower_scalar_nodes(&direct_nodes(query), binding, parameter)
    }

    fn lower_scalar_nodes(
        &mut self,
        nodes: &[GreenNode],
        binding: &BoundColumn,
        parameter: &Name,
    ) -> Result<LoweredScalar, ReasonCode> {
        let mut value = None;
        for node in nodes {
            self.mark_failure(node);
            match node.kind() {
                SyntaxKind::PROPERTY_NAV => {
                    let input = value.take().ok_or(ReasonCode::IndOpaquePredicate)?;
                    value = Some(self.lower_navigation(node, input)?.scalar);
                }
                SyntaxKind::VARIABLE_EXPR => {
                    if value.is_some() {
                        return Err(ReasonCode::IndOpaquePredicate);
                    }
                    value = Some(self.lower_variable(node, binding, parameter)?);
                }
                SyntaxKind::LITERAL_EXPR => {
                    if value.is_some() {
                        return Err(ReasonCode::IndOpaquePredicate);
                    }
                    value = Some(self.lower_literal(node)?);
                }
                SyntaxKind::BINARY_EXPR => {
                    if value.is_some() {
                        return Err(ReasonCode::IndOpaquePredicate);
                    }
                    value = Some(self.lower_binary(node, binding, parameter)?);
                }
                SyntaxKind::PAREN_EXPR => {
                    if value.is_some() {
                        return Err(ReasonCode::IndOpaquePredicate);
                    }
                    value = Some(self.lower_parenthesized(node, binding, parameter)?);
                }
                _ => return Err(ReasonCode::IndOpaquePredicate),
            }
        }
        value.ok_or(ReasonCode::IndOpaquePredicate)
    }

    fn lower_parenthesized(
        &mut self,
        node: &GreenNode,
        binding: &BoundColumn,
        parameter: &Name,
    ) -> Result<LoweredScalar, ReasonCode> {
        self.mark_failure(node);
        self.lower_scalar_nodes(&direct_nodes(node), binding, parameter)
    }

    fn lower_variable(
        &mut self,
        node: &GreenNode,
        binding: &BoundColumn,
        parameter: &Name,
    ) -> Result<LoweredScalar, ReasonCode> {
        self.mark_failure_with_origins(node, &[binding.column.origin()], &[]);
        let name = variable_name(node).ok_or(ReasonCode::IndOpaquePredicate)?;
        if &name != parameter {
            return Err(ReasonCode::IndUnresolvedSchema);
        }
        let expression = ScalarExpression::new(
            ScalarOperator::Column(binding.column.id()),
            binding.column.type_ref().clone(),
            binding.column.multiplicity(),
            binding.column.nullability(),
            Knowledge::<Totality>::unknown(),
            merged_origin(self.file, node.text_range(), &[binding.column.origin()]),
        );
        Ok(LoweredScalar {
            expression,
            local: binding.local.clone(),
        })
    }

    fn lower_navigation(
        &mut self,
        node: &GreenNode,
        input: LoweredScalar,
    ) -> Result<NavigationValue, ReasonCode> {
        self.mark_failure_with_origins(node, &[input.expression.origin()], &[]);
        let name = property_name(node).ok_or(ReasonCode::IndUnmodeledOp)?;
        let step = NavigationStep::property(name.clone());
        let receiver = local_with_multiplicity(&input.local, input.expression.multiplicity());
        let outcome = self
            .navigation
            .resolve(&receiver, std::slice::from_ref(&step));
        let NavigationResolution::Found(chain) = outcome else {
            return Err(navigation_reason(outcome));
        };
        if let Some(NavigationTarget::Member(member)) = chain.hops().first().map(|hop| hop.target())
        {
            self.mark_failure_with_origins(
                node,
                &[input.expression.origin()],
                &[ModelOrigin::from_member(member)],
            );
        }
        let navigation =
            ResolvedNavigation::from_chain(&chain).ok_or(ReasonCode::IndUnmodeledOp)?;
        let target = navigation.member().target().clone();
        let member_multiplicity = navigation.member().multiplicity();
        let member_origin = ModelOrigin::from_member(navigation.member());
        let mut navigation_origins = vec![member_origin];
        if let LocalValueKind::Class(class) = chain.value().kind() {
            navigation_origins.push(ModelOrigin::from_class(class));
        }
        let multiplicity =
            compose_navigation_multiplicity(input.expression.multiplicity(), member_multiplicity)
                .ok_or(ReasonCode::IndUnmodeledOp)?;
        let expression_origin = merged_origin_with_models(
            self.file,
            node.text_range(),
            &[input.expression.origin()],
            &navigation_origins,
        );
        let expression = ScalarExpression::new(
            ScalarOperator::Navigation {
                input: Box::new(input.expression),
                navigation: Box::new(navigation),
            },
            target,
            multiplicity,
            Nullability::Unknown,
            Knowledge::<Totality>::unknown(),
            expression_origin,
        );
        Ok(NavigationValue {
            name,
            scalar: LoweredScalar {
                expression,
                local: local_with_multiplicity(chain.value(), multiplicity),
            },
        })
    }

    fn lower_binary(
        &mut self,
        node: &GreenNode,
        binding: &BoundColumn,
        parameter: &Name,
    ) -> Result<LoweredScalar, ReasonCode> {
        self.mark_failure(node);
        let (operator, left_nodes, right_nodes) = binary_parts(node)?;
        let left = self.lower_scalar_nodes(&left_nodes, binding, parameter)?;
        let right = self.lower_scalar_nodes(&right_nodes, binding, parameter)?;
        if left.expression.type_ref() != right.expression.type_ref()
            || !is_exactly_one(left.expression.multiplicity())
            || !is_exactly_one(right.expression.multiplicity())
        {
            self.mark_failure_with_origins(
                node,
                &[left.expression.origin(), right.expression.origin()],
                &[],
            );
            return Err(ReasonCode::IndOpaquePredicate);
        }
        let type_ref = primitive_type(BOOLEAN_TYPE)?;
        let multiplicity = exactly_one()?;
        let expression_origin = merged_origin(
            self.file,
            node.text_range(),
            &[left.expression.origin(), right.expression.origin()],
        );
        let equal = ScalarExpression::new(
            ScalarOperator::Equal {
                left: Box::new(left.expression),
                right: Box::new(right.expression),
            },
            type_ref.clone(),
            multiplicity,
            Nullability::NonNullable,
            Knowledge::<Totality>::unknown(),
            expression_origin.clone(),
        );
        let expression = if operator == SyntaxKind::EQ {
            equal
        } else {
            ScalarExpression::new(
                ScalarOperator::Not {
                    input: Box::new(equal),
                },
                type_ref.clone(),
                multiplicity,
                Nullability::NonNullable,
                Knowledge::<Totality>::unknown(),
                expression_origin,
            )
        };
        Ok(LoweredScalar {
            expression,
            local: LocalValue::scalar(type_ref, multiplicity),
        })
    }

    fn lower_literal(&mut self, node: &GreenNode) -> Result<LoweredScalar, ReasonCode> {
        self.mark_failure(node);
        let tokens = significant_tokens(node);
        let [token] = tokens.as_slice() else {
            return Err(ReasonCode::IndOpaquePredicate);
        };
        let (literal, type_ref) = match token.kind() {
            SyntaxKind::BOOLEAN => {
                let value = match token.text() {
                    "true" => true,
                    "false" => false,
                    _ => return Err(ReasonCode::IndOpaquePredicate),
                };
                (ScalarLiteral::Boolean(value), primitive_type(BOOLEAN_TYPE)?)
            }
            SyntaxKind::INTEGER => {
                let value = token
                    .text()
                    .parse::<i64>()
                    .map_err(|_| ReasonCode::IndOpaquePredicate)?;
                (ScalarLiteral::Integer(value), primitive_type(INTEGER_TYPE)?)
            }
            SyntaxKind::STRING => (
                ScalarLiteral::String(
                    pure_string(token.text()).ok_or(ReasonCode::IndOpaquePredicate)?,
                ),
                primitive_type(STRING_TYPE)?,
            ),
            _ => return Err(ReasonCode::IndOpaquePredicate),
        };
        let multiplicity = exactly_one()?;
        let expression = ScalarExpression::new(
            ScalarOperator::Literal(literal),
            type_ref.clone(),
            multiplicity,
            Nullability::NonNullable,
            Knowledge::<Totality>::unknown(),
            origin(self.file, node.text_range(), Vec::new()),
        );
        Ok(LoweredScalar {
            expression,
            local: LocalValue::scalar(type_ref, multiplicity),
        })
    }

    fn next_column(&mut self) -> Result<ColumnId, ReasonCode> {
        let id = ColumnId::new(self.next_column);
        self.next_column = self
            .next_column
            .checked_add(1)
            .ok_or(ReasonCode::IndUnmodeledOp)?;
        Ok(id)
    }
}

#[derive(Debug, Clone)]
struct LambdaBody {
    parameter: Name,
    body: GreenNode,
}

fn top_level_queries(tree: &GreenNode) -> Vec<GreenNode> {
    if tree.kind() == SyntaxKind::QUERY_EXPR {
        return vec![tree.clone()];
    }
    direct_nodes(tree)
        .into_iter()
        .filter(|node| node.kind() == SyntaxKind::QUERY_EXPR)
        .collect()
}

fn contains_error_node(node: &GreenNode) -> bool {
    node.kind() == SyntaxKind::ERROR_NODE
        || node.tokens().any(|token| token.kind() == SyntaxKind::ERROR)
        || direct_nodes(node).iter().any(contains_error_node)
}

fn require_relation(state: Option<RelationState>) -> Result<RelationState, ReasonCode> {
    state.ok_or(ReasonCode::IndUnmodeledOp)
}

fn all_class_path(node: &GreenNode) -> Option<QName> {
    let children = direct_nodes(node);
    let paths = children
        .iter()
        .filter(|child| child.kind() == SyntaxKind::QUALIFIED_NAME)
        .collect::<Vec<_>>();
    let calls = children
        .iter()
        .filter(|child| child.kind() == SyntaxKind::CALL_ARGS)
        .collect::<Vec<_>>();
    let [path] = paths.as_slice() else {
        return None;
    };
    let [call] = calls.as_slice() else {
        return None;
    };
    let functions = node
        .children()
        .iter()
        .filter_map(GreenElement::as_token)
        .filter(|token| {
            matches!(
                token.kind(),
                SyntaxKind::ALL_KW
                    | SyntaxKind::ALL_VERSIONS_KW
                    | SyntaxKind::ALL_VERSIONS_IN_RANGE_KW
            )
        })
        .collect::<Vec<_>>();
    let [function] = functions.as_slice() else {
        return None;
    };
    if function.kind() != SyntaxKind::ALL_KW || !empty_call_arguments(call) {
        return None;
    }
    QName::new(compact_text(path)).ok()
}

fn arrow_name(node: &GreenNode) -> Option<Name> {
    let children = direct_nodes(node);
    let paths = children
        .iter()
        .filter(|child| child.kind() == SyntaxKind::QUALIFIED_NAME)
        .collect::<Vec<_>>();
    let [path] = paths.as_slice() else {
        return None;
    };
    let path = QName::new(compact_text(path)).ok()?;
    path.package()
        .is_none()
        .then(|| Name::new(path.simple_name()).ok())
        .flatten()
}

fn arrow_lambda(node: &GreenNode) -> Option<LambdaBody> {
    let children = direct_nodes(node);
    let calls = children
        .iter()
        .filter(|child| child.kind() == SyntaxKind::CALL_ARGS)
        .collect::<Vec<_>>();
    let [call] = calls.as_slice() else {
        return None;
    };
    let arguments = direct_nodes(call);
    let [lambda] = arguments.as_slice() else {
        return None;
    };
    lambda_body(lambda)
}

fn lambda_body(node: &GreenNode) -> Option<LambdaBody> {
    if node.kind() != SyntaxKind::LAMBDA_EXPR {
        return None;
    }
    let children = direct_nodes(node);
    let parameters = children
        .iter()
        .filter(|child| child.kind() == SyntaxKind::LAMBDA_PARAMS)
        .collect::<Vec<_>>();
    let blocks = children
        .iter()
        .filter(|child| child.kind() == SyntaxKind::CODE_BLOCK)
        .collect::<Vec<_>>();
    let [parameters] = parameters.as_slice() else {
        return None;
    };
    let [block] = blocks.as_slice() else {
        return None;
    };
    let parameter = lambda_parameter(parameters)?;
    let body_nodes = direct_nodes(block);
    let [body] = body_nodes.as_slice() else {
        return None;
    };
    (body.kind() == SyntaxKind::QUERY_EXPR).then(|| LambdaBody {
        parameter,
        body: body.clone(),
    })
}

fn lambda_parameter(node: &GreenNode) -> Option<Name> {
    if node.tokens().any(|token| token.kind() == SyntaxKind::COLON)
        || !direct_nodes(node).is_empty()
    {
        return None;
    }
    let names = node
        .tokens()
        .filter(|token| token.kind() == SyntaxKind::IDENT)
        .collect::<Vec<_>>();
    let [name] = names.as_slice() else {
        return None;
    };
    Name::new(name.text()).ok()
}

fn property_name(node: &GreenNode) -> Option<Name> {
    if node.kind() != SyntaxKind::PROPERTY_NAV || !direct_nodes(node).is_empty() {
        return None;
    }
    let names = node
        .tokens()
        .filter(|token| token.kind() == SyntaxKind::IDENT)
        .collect::<Vec<_>>();
    let [name] = names.as_slice() else {
        return None;
    };
    Name::new(name.text()).ok()
}

fn variable_name(node: &GreenNode) -> Option<Name> {
    let names = node
        .tokens()
        .filter(|token| token.kind() == SyntaxKind::IDENT)
        .collect::<Vec<_>>();
    let [name] = names.as_slice() else {
        return None;
    };
    Name::new(name.text()).ok()
}

fn empty_call_arguments(node: &GreenNode) -> bool {
    direct_nodes(node).is_empty()
        && significant_tokens(node)
            .iter()
            .map(|token| token.kind())
            .eq([SyntaxKind::PAREN_OPEN, SyntaxKind::PAREN_CLOSE])
}

fn binary_parts(
    node: &GreenNode,
) -> Result<(SyntaxKind, Vec<GreenNode>, Vec<GreenNode>), ReasonCode> {
    let mut operator = None;
    let mut after_operator = false;
    let mut left = Vec::new();
    let mut right = Vec::new();
    for element in node.children() {
        match element {
            GreenElement::Token(token)
                if matches!(token.kind(), SyntaxKind::EQ | SyntaxKind::NEQ) =>
            {
                if operator.replace(token.kind()).is_some() {
                    return Err(ReasonCode::IndOpaquePredicate);
                }
                after_operator = true;
            }
            GreenElement::Node(child) if after_operator => right.push(child.clone()),
            GreenElement::Node(child) => left.push(child.clone()),
            GreenElement::Token(_) => {}
        }
    }
    let Some(operator) = operator else {
        return Err(ReasonCode::IndOpaquePredicate);
    };
    if left.is_empty() || right.is_empty() {
        return Err(ReasonCode::IndOpaquePredicate);
    }
    Ok((operator, left, right))
}

fn navigation_reason(outcome: NavigationResolution) -> ReasonCode {
    match outcome {
        NavigationResolution::Missing(_) | NavigationResolution::WrongArity(_) => {
            ReasonCode::IndUnresolvedSchema
        }
        NavigationResolution::UnderResolved(_)
        | NavigationResolution::Ambiguous(_)
        | NavigationResolution::Cycle(_) => ReasonCode::ModelIncomplete,
        NavigationResolution::Found(_) => ReasonCode::IndUnresolvedSchema,
    }
}

fn predicate_reason(reason: ReasonCode) -> ReasonCode {
    if reason == ReasonCode::IndUnmodeledOp {
        ReasonCode::IndOpaquePredicate
    } else {
        reason
    }
}

fn operator_reason(reason: ReasonCode) -> ReasonCode {
    if reason == ReasonCode::IndOpaquePredicate {
        ReasonCode::IndUnmodeledOp
    } else {
        reason
    }
}

fn column_scalar(binding: &BoundColumn) -> LoweredScalar {
    LoweredScalar {
        expression: ScalarExpression::new(
            ScalarOperator::Column(binding.column.id()),
            binding.column.type_ref().clone(),
            binding.column.multiplicity(),
            binding.column.nullability(),
            Knowledge::<Totality>::unknown(),
            binding.column.origin().clone(),
        ),
        local: binding.local.clone(),
    }
}

fn local_with_multiplicity(value: &LocalValue, multiplicity: Multiplicity) -> LocalValue {
    match value.kind() {
        LocalValueKind::Class(class) => LocalValue::class(class.clone(), multiplicity),
        LocalValueKind::Scalar(scalar) => LocalValue::scalar(scalar.clone(), multiplicity),
        LocalValueKind::RelationRow(row) => LocalValue::relation_row(row.clone(), multiplicity),
        LocalValueKind::Unknown(unknown) => LocalValue::unknown(unknown.clone(), multiplicity),
    }
}

fn exactly_one() -> Result<Multiplicity, ReasonCode> {
    Multiplicity::new(ONE, Some(ONE)).map_err(|_| ReasonCode::IndUnmodeledOp)
}

fn is_exactly_one(multiplicity: Multiplicity) -> bool {
    multiplicity.lower() == ONE && multiplicity.upper() == Some(ONE)
}

fn primitive_type(name: &str) -> Result<TypeRef, ReasonCode> {
    QName::new(name)
        .map(|path| TypeRef::new(path, Vec::new()))
        .map_err(|_| ReasonCode::IndUnmodeledOp)
}

fn pure_string(text: &str) -> Option<String> {
    text.strip_prefix('\'')
        .and_then(|value| value.strip_suffix('\''))
        .map(|value| value.replace("''", "'"))
}

fn origin(file: FileId, range: TextRange, model_origins: Vec<ModelOrigin>) -> IrOrigin {
    IrOrigin::new(SourceSpan::new(file, range), model_origins)
}

fn merged_origin(file: FileId, range: TextRange, origins: &[&IrOrigin]) -> IrOrigin {
    merged_origin_with_models(file, range, origins, &[])
}

fn merged_origin_with_models(
    file: FileId,
    range: TextRange,
    origins: &[&IrOrigin],
    extra_model_origins: &[ModelOrigin],
) -> IrOrigin {
    let mut model_origins = Vec::new();
    for source in origins {
        for model_origin in source.model_origins() {
            if !model_origins.contains(model_origin) {
                model_origins.push(model_origin.clone());
            }
        }
    }
    for model_origin in extra_model_origins {
        if !model_origins.contains(model_origin) {
            model_origins.push(model_origin.clone());
        }
    }
    origin(file, range, model_origins)
}

fn opaque(reason: ReasonCode, origin: IrOrigin) -> RelationalOutcome {
    RelationalOutcome::opaque(OpaqueOutcome::new(reason, origin))
}

fn direct_nodes(node: &GreenNode) -> Vec<GreenNode> {
    node.children()
        .iter()
        .filter_map(GreenElement::as_node)
        .cloned()
        .collect()
}

fn significant_tokens(node: &GreenNode) -> Vec<&pure_analyzer_syntax::GreenToken> {
    node.tokens()
        .filter(|token| !is_trivia(token.kind()))
        .collect()
}

fn compact_text(node: &GreenNode) -> String {
    node.tokens()
        .filter(|token| !is_trivia(token.kind()))
        .map(|token| token.text())
        .collect()
}

fn is_trivia(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        SyntaxKind::WHITESPACE | SyntaxKind::LINE_COMMENT | SyntaxKind::BLOCK_COMMENT
    )
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use pure_analyzer_lexer::lex;
    use pure_analyzer_model::ModelGraph;
    use pure_analyzer_syntax::GreenNodeBuilder;

    use super::*;

    #[test]
    fn empty_call_arguments_rejects_an_invisible_child_node() {
        let source = "()";
        let tokens = lex(source);
        let mut builder = GreenNodeBuilder::new(source, &tokens);
        builder.open(SyntaxKind::ROOT);
        builder.open(SyntaxKind::CALL_ARGS);
        builder.advance();
        builder.open(SyntaxKind::ERROR_NODE);
        builder.close();
        builder.advance();
        builder.close();
        builder.close();
        let root = builder.finish().expect("fixture tree must build");
        let call_arguments = direct_nodes(&root)
            .into_iter()
            .next()
            .expect("fixture must contain call arguments");

        assert_eq!(call_arguments.kind(), SyntaxKind::CALL_ARGS);
        assert!(!empty_call_arguments(&call_arguments));
    }

    #[test]
    fn lambda_parameter_rejects_an_invisible_child_node() {
        let source = "x";
        let tokens = lex(source);
        let mut builder = GreenNodeBuilder::new(source, &tokens);
        builder.open(SyntaxKind::ROOT);
        builder.open(SyntaxKind::LAMBDA_PARAMS);
        builder.advance();
        builder.open(SyntaxKind::ERROR_NODE);
        builder.close();
        builder.close();
        builder.close();
        let root = builder.finish().expect("fixture tree must build");
        let parameters = direct_nodes(&root)
            .into_iter()
            .next()
            .expect("fixture must contain lambda parameters");

        assert_eq!(parameters.kind(), SyntaxKind::LAMBDA_PARAMS);
        assert_eq!(lambda_parameter(&parameters), None);
    }

    #[test]
    fn binary_parts_rejects_a_missing_left_operand() {
        let source = "==true";
        let tokens = lex(source);
        let mut builder = GreenNodeBuilder::new(source, &tokens);
        builder.open(SyntaxKind::ROOT);
        builder.open(SyntaxKind::BINARY_EXPR);
        builder.advance();
        builder.open(SyntaxKind::LITERAL_EXPR);
        builder.advance();
        builder.close();
        builder.close();
        builder.close();
        let root = builder.finish().expect("fixture tree must build");
        let binary = direct_nodes(&root)
            .into_iter()
            .next()
            .expect("fixture must contain a binary expression");

        assert_eq!(binary.kind(), SyntaxKind::BINARY_EXPR);
        assert_eq!(binary_parts(&binary), Err(ReasonCode::IndOpaquePredicate));
    }

    #[test]
    fn malformed_tree_is_rejected_even_without_parser_diagnostics() {
        let source = "";
        let tokens = lex(source);
        let mut builder = GreenNodeBuilder::new(source, &tokens);
        builder.open(SyntaxKind::ROOT);
        builder.open(SyntaxKind::ERROR_NODE);
        builder.close();
        builder.close();
        let root = builder.finish().expect("fixture tree must build");
        let error = direct_nodes(&root)
            .into_iter()
            .next()
            .expect("fixture must contain an error node");

        assert!(contains_error_node(&error));
        assert!(contains_error_node(&root));
        assert!(matches!(
            lower_m3_query(AnalysisInput::new(FileId::new(91), source, &root, &[], None)),
            RelationalOutcome::Opaque(value) if value.reason() == ReasonCode::IndUnparseable
        ));
    }

    #[test]
    fn mark_failure_updates_the_failure_span() {
        let source = "x";
        let tokens = lex(source);
        let mut builder = GreenNodeBuilder::new(source, &tokens);
        builder.open(SyntaxKind::ROOT);
        builder.open(SyntaxKind::LITERAL_EXPR);
        builder.advance();
        builder.close();
        builder.close();
        let root = builder.finish().expect("fixture tree must build");
        let literal = direct_nodes(&root)
            .into_iter()
            .next()
            .expect("fixture must contain a literal node");
        let model = ModelGraph::default();
        let mut lowerer = QueryLowerer::new(FileId::new(91), &model, TextRange::default());

        lowerer.mark_failure(&literal);

        assert_eq!(
            lowerer.failure_origin().source().range(),
            literal.text_range()
        );
    }

    #[test]
    fn property_name_requires_a_bare_property_navigation_node() {
        let source = "x";
        let tokens = lex(source);
        let mut builder = GreenNodeBuilder::new(source, &tokens);
        builder.open(SyntaxKind::ROOT);
        builder.open(SyntaxKind::LITERAL_EXPR);
        builder.advance();
        builder.close();
        builder.close();
        let root = builder.finish().expect("fixture tree must build");
        let literal = direct_nodes(&root)
            .into_iter()
            .next()
            .expect("fixture must contain a literal node");

        assert_eq!(property_name(&literal), None);
    }

    #[test]
    fn all_class_path_requires_the_all_keyword() {
        let source = "x()";
        let tokens = lex(source);
        let mut builder = GreenNodeBuilder::new(source, &tokens);
        builder.open(SyntaxKind::ROOT);
        builder.open(SyntaxKind::ALL_EXPR);
        builder.open(SyntaxKind::QUALIFIED_NAME);
        builder.advance();
        builder.close();
        builder.open(SyntaxKind::CALL_ARGS);
        builder.advance();
        builder.advance();
        builder.close();
        builder.close();
        builder.close();
        let root = builder.finish().expect("fixture tree must build");
        let all = direct_nodes(&root)
            .into_iter()
            .next()
            .expect("fixture must contain an all expression");

        assert_eq!(all_class_path(&all), None);
    }

    #[test]
    fn all_class_path_rejects_mixed_all_function_tokens() {
        let source = "x all() allVersions";
        let tokens = lex(source);
        let mut builder = GreenNodeBuilder::new(source, &tokens);
        builder.open(SyntaxKind::ROOT);
        builder.open(SyntaxKind::ALL_EXPR);
        builder.open(SyntaxKind::QUALIFIED_NAME);
        builder.advance();
        builder.close();
        builder.advance();
        builder.advance();
        builder.open(SyntaxKind::CALL_ARGS);
        builder.advance();
        builder.advance();
        builder.close();
        builder.advance();
        builder.advance();
        builder.close();
        builder.close();
        let root = builder.finish().expect("fixture tree must build");
        let all = direct_nodes(&root)
            .into_iter()
            .next()
            .expect("fixture must contain an all expression");

        assert_eq!(all_class_path(&all), None);
    }

    #[test]
    fn all_class_path_rejects_non_empty_call_arguments() {
        let source = "x all(1)";
        let tokens = lex(source);
        let mut builder = GreenNodeBuilder::new(source, &tokens);
        builder.open(SyntaxKind::ROOT);
        builder.open(SyntaxKind::ALL_EXPR);
        builder.open(SyntaxKind::QUALIFIED_NAME);
        builder.advance();
        builder.close();
        builder.advance();
        builder.advance();
        builder.open(SyntaxKind::CALL_ARGS);
        builder.advance();
        builder.open(SyntaxKind::LITERAL_EXPR);
        builder.advance();
        builder.close();
        builder.advance();
        builder.close();
        builder.close();
        builder.close();
        let root = builder.finish().expect("fixture tree must build");
        let all = direct_nodes(&root)
            .into_iter()
            .next()
            .expect("fixture must contain an all expression");

        assert_eq!(all_class_path(&all), None);
    }

    #[test]
    fn exactly_one_rejects_an_unbounded_value() {
        let one_or_more = Multiplicity::new(ONE, None).expect("fixture multiplicity must be valid");

        assert!(!is_exactly_one(one_or_more));
    }

    #[test]
    fn trivia_classification_retains_whitespace() {
        assert!(is_trivia(SyntaxKind::WHITESPACE));
        assert!(!is_trivia(SyntaxKind::IDENT));
    }
}
