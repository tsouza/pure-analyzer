//! Conservative lowering of the minimal resolved M3 query subset.

use std::collections::BTreeSet;

use pure_analyzer_diagnostics::{FileId, ReasonCode};
use pure_analyzer_model::{Multiplicity, Name, QName, TypeRef};
use pure_analyzer_resolve::{
    LocalValue, LocalValueKind, NavigationResolution, NavigationResolver, NavigationStep,
    NavigationTarget, RelationColumn, RelationColumnId, RelationRow, Resolution,
};
use pure_analyzer_syntax::{GreenElement, GreenNode, SyntaxKind, TextRange};

use crate::{
    AnalysisInput, Column, ColumnId, ColumnSelectorOpaqueReason, ColumnSelectorOutcome, IrOrigin,
    JoinKind, Knowledge, ModelOrigin, Nullability, OpaqueOutcome, Projection, ProjectionKind,
    RelationExpression, RelationFacts, RelationOperator, RelationSchema, RelationSource,
    RelationalOutcome, RelationalQuery, ResolvedNavigation, RowSemantics, ScalarExpression,
    ScalarLiteral, ScalarOperator, SortDirection, SortKey, SourceSpan, Totality,
    cst_util::{contains_error_node, direct_nodes, element_is_trivia, is_trivia},
    relational::{
        BOOLEAN_TYPE, EXACTLY_ONE, INTEGER_TYPE, MAP_VALUE_COLUMN_NAME, STRING_TYPE,
        compose_navigation_multiplicity,
    },
    resolve_relation_column_selectors,
};

/// Lower one parsed M3 query into the proven relational core or a typed opaque outcome.
///
/// The input must contain exactly one top-level query expression and a model graph. The
/// supported subset is deliberately limited to `Class.all()`, to-one resolved navigation,
/// `->filter` with an equality predicate, one-lambda `->map`, constrained schema-aware
/// `->project(~[alias: row | expression, ...])`, bare and selected `->distinct()`, proven
/// ascending/descending resolved keys in `->sort`, and one terminal inner join with two resolved
/// row bindings. Other valid syntax stays explicit as an opaque outcome rather than being
/// approximated.
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
struct BoundRelationRow {
    local: LocalValue,
    columns: Vec<BoundColumn>,
}

#[derive(Debug, Clone)]
enum BoundElement {
    Column(BoundColumn),
    RelationRow(BoundRelationRow),
}

impl BoundElement {
    fn as_column(&self) -> Option<&BoundColumn> {
        match self {
            Self::Column(column) => Some(column),
            Self::RelationRow(_) => None,
        }
    }

    fn scalar_binding(&self) -> ScalarBinding {
        match self {
            Self::Column(column) => ScalarBinding::Column(column.clone()),
            Self::RelationRow(row) => ScalarBinding::RelationRow(row.clone()),
        }
    }
}

#[derive(Debug, Clone)]
enum ScalarBinding {
    Column(BoundColumn),
    RelationRow(BoundRelationRow),
}

#[derive(Debug, Clone)]
enum LoweredValue {
    Scalar(LoweredScalar),
    RelationRow(BoundRelationRow),
}

#[derive(Debug, Clone)]
struct RelationState {
    expression: RelationExpression,
    element: Option<BoundElement>,
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
                SyntaxKind::PAREN_EXPR => {
                    if state.is_some() {
                        return Err(ReasonCode::IndUnmodeledOp);
                    }
                    state = Some(self.lower_parenthesized_relation(node)?);
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

    fn lower_parenthesized_relation(
        &mut self,
        node: &GreenNode,
    ) -> Result<RelationState, ReasonCode> {
        self.mark_failure(node);
        self.lower_relation_nodes(&direct_nodes(node))
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
            element: Some(BoundElement::Column(BoundColumn {
                column,
                local: LocalValue::class(class, multiplicity),
            })),
        })
    }

    fn lower_arrow(
        &mut self,
        state: RelationState,
        node: &GreenNode,
    ) -> Result<RelationState, ReasonCode> {
        self.mark_failure_with_origins(node, &[state.expression.origin()], &[]);
        let name = arrow_name(node).ok_or(ReasonCode::IndUnmodeledOp)?;
        if !matches!(name.as_str(), "distinct" | "sort") && state.element.is_none() {
            return Err(ReasonCode::IndUnmodeledOp);
        }
        match name.as_str() {
            "filter" => self.lower_filter(
                state,
                node,
                arrow_lambda(node).ok_or(ReasonCode::IndUnmodeledOp)?,
            ),
            "map" => self.lower_map(
                state,
                node,
                arrow_lambda(node).ok_or(ReasonCode::IndUnmodeledOp)?,
            ),
            "project" => self.lower_relation_project(state, node),
            "distinct" => self.lower_distinct(state, node),
            "join" => self.lower_join(state, node),
            "sort" => self.lower_sort(state, node),
            _ => Err(ReasonCode::IndUnmodeledOp),
        }
    }

    fn lower_sort(
        &mut self,
        state: RelationState,
        node: &GreenNode,
    ) -> Result<RelationState, ReasonCode> {
        let input_origin = state.expression.origin().clone();
        self.mark_failure_with_origins(node, &[&input_origin], &[]);
        let call = arrow_call_arguments(node).ok_or(ReasonCode::IndUnmodeledOp)?;
        let arguments = sort_arguments(&call).ok_or(ReasonCode::IndUnmodeledOp)?;
        let schema = state.expression.schema().clone();
        let facts = state.expression.facts().clone();
        let mut keys = Vec::with_capacity(arguments.len());

        for argument in arguments {
            self.failure_origin = merged_origin(self.file, argument.range, &[&input_origin]);
            let (direction, selector) =
                sort_key_parts(&argument.nodes).ok_or(ReasonCode::IndUnmodeledOp)?;
            let column = self.resolve_sort_column(&selector, &schema, &input_origin)?;
            let column_origin = schema
                .column(column)
                .map(Column::origin)
                .ok_or(ReasonCode::IndUnresolvedSchema)?;
            let key_origin =
                merged_origin(self.file, argument.range, &[&input_origin, column_origin]);
            keys.push(SortKey::new(column, direction, key_origin));
        }

        let mut operator_origins = Vec::with_capacity(keys.len().saturating_add(1));
        operator_origins.push(&input_origin);
        operator_origins.extend(keys.iter().map(SortKey::origin));
        let operator_origin = merged_origin(self.file, node.text_range(), &operator_origins);
        let expression = RelationExpression::new(
            RelationOperator::Sort {
                input: Box::new(state.expression),
                keys,
            },
            schema,
            facts,
            operator_origin,
        )
        .map_err(|_| ReasonCode::IndUnmodeledOp)?;
        Ok(RelationState {
            expression,
            element: state.element,
        })
    }

    fn resolve_sort_column(
        &mut self,
        selector: &GreenNode,
        schema: &RelationSchema,
        input_origin: &IrOrigin,
    ) -> Result<ColumnId, ReasonCode> {
        match resolve_relation_column_selectors(self.file, selector, schema) {
            ColumnSelectorOutcome::Resolved(resolved) => {
                let [resolved] = resolved.selectors() else {
                    return Err(ReasonCode::IndUnmodeledOp);
                };
                Ok(resolved.column())
            }
            ColumnSelectorOutcome::Opaque(opaque) => {
                self.failure_origin = merged_source_origin(opaque.source(), &[input_origin]);
                match opaque.reason() {
                    ColumnSelectorOpaqueReason::Missing(_)
                    | ColumnSelectorOpaqueReason::DuplicateSchemaName(_) => {
                        Err(ReasonCode::IndUnresolvedSchema)
                    }
                    ColumnSelectorOpaqueReason::UnsupportedForm
                    | ColumnSelectorOpaqueReason::Malformed
                    | ColumnSelectorOpaqueReason::UnsupportedBody
                    | ColumnSelectorOpaqueReason::DuplicateSelector(_) => {
                        Err(ReasonCode::IndUnmodeledOp)
                    }
                }
            }
        }
    }

    fn lower_distinct(
        &mut self,
        state: RelationState,
        node: &GreenNode,
    ) -> Result<RelationState, ReasonCode> {
        self.mark_failure_with_origins(node, &[state.expression.origin()], &[]);
        let call = arrow_call_arguments(node).ok_or(ReasonCode::IndUnmodeledOp)?;
        if empty_call_arguments(&call) {
            if state.element.is_none() {
                return Err(ReasonCode::IndUnmodeledOp);
            }
            return self.lower_bare_distinct(state, node);
        }
        self.lower_selected_distinct(state, node, &call)
    }

    fn lower_bare_distinct(
        &mut self,
        state: RelationState,
        node: &GreenNode,
    ) -> Result<RelationState, ReasonCode> {
        let schema = state.expression.schema().clone();
        let operator_origin =
            merged_origin(self.file, node.text_range(), &[state.expression.origin()]);
        let facts = RelationFacts::new(
            Knowledge::unknown(),
            Knowledge::proven(RowSemantics::Set, operator_origin.clone()),
        );
        let expression = RelationExpression::new(
            RelationOperator::Distinct {
                input: Box::new(state.expression),
            },
            schema,
            facts,
            operator_origin,
        )
        .map_err(|_| ReasonCode::IndUnmodeledOp)?;
        Ok(RelationState {
            expression,
            element: state.element,
        })
    }

    fn lower_selected_distinct(
        &mut self,
        state: RelationState,
        node: &GreenNode,
        call: &GreenNode,
    ) -> Result<RelationState, ReasonCode> {
        let selectors = strict_distinct_selectors(call).ok_or(ReasonCode::IndUnmodeledOp)?;
        let resolved = match resolve_relation_column_selectors(
            self.file,
            &selectors,
            state.expression.schema(),
        ) {
            ColumnSelectorOutcome::Resolved(resolved) => resolved,
            ColumnSelectorOutcome::Opaque(opaque) => {
                self.failure_origin = merged_origin(
                    self.file,
                    opaque.source().range(),
                    &[state.expression.origin()],
                );
                return Err(selector_reason(opaque.reason()));
            }
        };
        let columns = resolved
            .selectors()
            .iter()
            .map(|selector| selector.column())
            .collect::<Vec<_>>();
        let schema = RelationSchema::new(
            columns
                .iter()
                .map(|column| {
                    state
                        .expression
                        .schema()
                        .column(*column)
                        .cloned()
                        .ok_or(ReasonCode::IndUnmodeledOp)
                })
                .collect::<Result<Vec<_>, _>>()?,
        )
        .map_err(|_| ReasonCode::IndUnmodeledOp)?;
        let RelationState {
            expression: input,
            element,
        } = state;
        let element = selected_bound_element(element, &columns)?;
        let operator_origin = merged_origin(self.file, node.text_range(), &[input.origin()]);
        let expression = RelationExpression::new(
            RelationOperator::DistinctOn {
                input: Box::new(input),
                columns,
            },
            schema,
            RelationFacts::unknown(),
            operator_origin,
        )
        .map_err(|_| ReasonCode::IndUnmodeledOp)?;
        Ok(RelationState {
            expression,
            element,
        })
    }

    fn lower_join(
        &mut self,
        state: RelationState,
        node: &GreenNode,
    ) -> Result<RelationState, ReasonCode> {
        self.mark_failure_with_origins(node, &[state.expression.origin()], &[]);
        let left_element = state
            .element
            .as_ref()
            .and_then(BoundElement::as_column)
            .ok_or(ReasonCode::IndUnmodeledOp)?;
        let arguments = join_arguments(node)?;
        let right_state = self.lower_relation_nodes(std::slice::from_ref(&arguments.right))?;
        let right_element = right_state
            .element
            .as_ref()
            .and_then(BoundElement::as_column)
            .ok_or(ReasonCode::IndUnmodeledOp)?;
        let predicate = self
            .lower_scalar_query(
                &arguments.predicate,
                &[
                    (
                        &arguments.left_parameter,
                        ScalarBinding::Column(left_element.clone()),
                    ),
                    (
                        &arguments.right_parameter,
                        ScalarBinding::Column(right_element.clone()),
                    ),
                ],
            )
            .map_err(predicate_reason)?;
        if !references_column(&predicate.expression, left_element.column.id())
            || !references_column(&predicate.expression, right_element.column.id())
        {
            self.mark_failure_with_origins(
                node,
                &[
                    state.expression.origin(),
                    right_state.expression.origin(),
                    predicate.expression.origin(),
                ],
                &[],
            );
            return Err(ReasonCode::IndOpaquePredicate);
        }
        let schema = RelationSchema::new(
            state
                .expression
                .schema()
                .columns()
                .iter()
                .chain(right_state.expression.schema().columns())
                .cloned()
                .collect(),
        )
        .map_err(|_| ReasonCode::IndUnmodeledOp)?;
        let operator_origin = merged_origin(
            self.file,
            node.text_range(),
            &[
                state.expression.origin(),
                right_state.expression.origin(),
                predicate.expression.origin(),
            ],
        );
        let expression = RelationExpression::new(
            RelationOperator::Join {
                kind: JoinKind::Inner,
                left: Box::new(state.expression),
                right: Box::new(right_state.expression),
                condition: predicate.expression,
            },
            schema,
            RelationFacts::unknown(),
            operator_origin,
        )
        .map_err(|_| ReasonCode::IndOpaquePredicate)?;
        Ok(RelationState {
            expression,
            element: None,
        })
    }

    fn lower_filter(
        &mut self,
        state: RelationState,
        node: &GreenNode,
        lambda: LambdaBody,
    ) -> Result<RelationState, ReasonCode> {
        self.mark_failure_with_origins(node, &[state.expression.origin()], &[]);
        let element = state
            .element
            .as_ref()
            .ok_or(ReasonCode::IndUnmodeledOp)?
            .scalar_binding();
        let predicate = self
            .lower_scalar_query(&lambda.body, &[(&lambda.parameter, element)])
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
        let element = state
            .element
            .as_ref()
            .ok_or(ReasonCode::IndUnmodeledOp)?
            .scalar_binding();
        let scalar = self
            .lower_scalar_query(&lambda.body, &[(&lambda.parameter, element)])
            .map_err(operator_reason)?;
        let name = Name::new(MAP_VALUE_COLUMN_NAME).map_err(|_| ReasonCode::IndUnmodeledOp)?;
        self.project(state, node, name, scalar)
    }

    fn lower_relation_project(
        &mut self,
        state: RelationState,
        node: &GreenNode,
    ) -> Result<RelationState, ReasonCode> {
        self.mark_failure_with_origins(node, &[state.expression.origin()], &[]);
        let element = state
            .element
            .as_ref()
            .and_then(BoundElement::as_column)
            .ok_or(ReasonCode::IndUnmodeledOp)?
            .clone();
        let specs = project_column_specs(node)?;
        let input_origin = state.expression.origin().clone();
        let mut columns = Vec::with_capacity(specs.len());
        let mut projections = Vec::with_capacity(specs.len());
        let mut projection_origins = Vec::with_capacity(specs.len());
        let mut row_columns = Vec::with_capacity(specs.len());

        for spec in specs {
            let scalar = self
                .lower_scalar_query(
                    &spec.lambda.body,
                    &[(
                        &spec.lambda.parameter,
                        ScalarBinding::Column(element.clone()),
                    )],
                )
                .map_err(operator_reason)?;
            let source_range = significant_range(&spec.source).ok_or(ReasonCode::IndUnmodeledOp)?;
            let column_origin =
                merged_origin(self.file, source_range, &[scalar.expression.origin()]);
            let column = Column::new(
                self.next_column()?,
                spec.alias,
                scalar.expression.type_ref().clone(),
                scalar.expression.multiplicity(),
                scalar.expression.nullability(),
                column_origin,
            );
            projection_origins.push(scalar.expression.origin().clone());
            projections.push(Projection::new(column.id(), scalar.expression));
            row_columns.push(BoundColumn {
                column: column.clone(),
                local: scalar.local,
            });
            columns.push(column);
        }

        let schema = RelationSchema::new(columns).map_err(|_| ReasonCode::IndUnmodeledOp)?;
        let relation_row = bound_relation_row(row_columns)?;
        let mut origins = Vec::with_capacity(projection_origins.len() + 1);
        origins.push(&input_origin);
        origins.extend(projection_origins.iter());
        let operator_origin = merged_origin(self.file, node.text_range(), &origins);
        let expression = RelationExpression::new(
            RelationOperator::Project {
                input: Box::new(state.expression),
                projections,
                kind: ProjectionKind::Relation,
            },
            schema,
            RelationFacts::unknown(),
            operator_origin,
        )
        .map_err(|_| ReasonCode::IndUnmodeledOp)?;
        Ok(RelationState {
            expression,
            element: Some(BoundElement::RelationRow(relation_row)),
        })
    }

    fn project_navigation(
        &mut self,
        state: RelationState,
        node: &GreenNode,
    ) -> Result<RelationState, ReasonCode> {
        self.mark_failure_with_origins(node, &[state.expression.origin()], &[]);
        let element = state
            .element
            .as_ref()
            .and_then(BoundElement::as_column)
            .ok_or(ReasonCode::IndUnmodeledOp)?;
        let input = column_scalar(element);
        let navigation = self.lower_navigation(node, input)?;
        self.project(state, node, navigation.name, navigation.scalar)
    }

    /// Shared by [`Self::lower_map`] and [`Self::project_navigation`]: both
    /// produce a single-column, scalar-collection result (`->map(f)` and
    /// `.property` navigation are equally scalar, never a `Relation<>`), so
    /// every call site here is [`ProjectionKind::Scalar`].
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
                kind: ProjectionKind::Scalar,
            },
            schema,
            RelationFacts::unknown(),
            operator_origin,
        )
        .map_err(|_| ReasonCode::IndUnmodeledOp)?;
        Ok(RelationState {
            expression,
            element: Some(BoundElement::Column(BoundColumn {
                column,
                local: scalar.local,
            })),
        })
    }

    fn lower_scalar_query(
        &mut self,
        query: &GreenNode,
        bindings: &[(&Name, ScalarBinding)],
    ) -> Result<LoweredScalar, ReasonCode> {
        self.mark_failure(query);
        if query.kind() != SyntaxKind::QUERY_EXPR {
            return Err(ReasonCode::IndOpaquePredicate);
        }
        self.lower_scalar_nodes(&direct_nodes(query), bindings)
    }

    fn lower_scalar_nodes(
        &mut self,
        nodes: &[GreenNode],
        bindings: &[(&Name, ScalarBinding)],
    ) -> Result<LoweredScalar, ReasonCode> {
        match self.lower_value_nodes(nodes, bindings)? {
            LoweredValue::Scalar(scalar) => Ok(scalar),
            LoweredValue::RelationRow(_) => Err(ReasonCode::IndOpaquePredicate),
        }
    }

    fn lower_value_nodes(
        &mut self,
        nodes: &[GreenNode],
        bindings: &[(&Name, ScalarBinding)],
    ) -> Result<LoweredValue, ReasonCode> {
        let mut value = None;
        for node in nodes {
            self.mark_failure(node);
            match node.kind() {
                SyntaxKind::PROPERTY_NAV => {
                    let input = value.take().ok_or(ReasonCode::IndOpaquePredicate)?;
                    let scalar = match input {
                        LoweredValue::Scalar(scalar) => self.lower_navigation(node, scalar)?.scalar,
                        LoweredValue::RelationRow(row) => self.lower_relation_column(node, &row)?,
                    };
                    value = Some(LoweredValue::Scalar(scalar));
                }
                SyntaxKind::VARIABLE_EXPR => {
                    if value.is_some() {
                        return Err(ReasonCode::IndOpaquePredicate);
                    }
                    value = Some(self.lower_variable(node, bindings)?);
                }
                SyntaxKind::LITERAL_EXPR => {
                    if value.is_some() {
                        return Err(ReasonCode::IndOpaquePredicate);
                    }
                    value = Some(LoweredValue::Scalar(self.lower_literal(node)?));
                }
                SyntaxKind::BINARY_EXPR => {
                    if value.is_some() {
                        return Err(ReasonCode::IndOpaquePredicate);
                    }
                    value = Some(LoweredValue::Scalar(self.lower_binary(node, bindings)?));
                }
                SyntaxKind::PAREN_EXPR => {
                    if value.is_some() {
                        return Err(ReasonCode::IndOpaquePredicate);
                    }
                    value = Some(LoweredValue::Scalar(
                        self.lower_parenthesized(node, bindings)?,
                    ));
                }
                _ => return Err(ReasonCode::IndOpaquePredicate),
            }
        }
        value.ok_or(ReasonCode::IndOpaquePredicate)
    }

    fn lower_parenthesized(
        &mut self,
        node: &GreenNode,
        bindings: &[(&Name, ScalarBinding)],
    ) -> Result<LoweredScalar, ReasonCode> {
        self.mark_failure(node);
        self.lower_scalar_nodes(&direct_nodes(node), bindings)
    }

    fn lower_variable(
        &mut self,
        node: &GreenNode,
        bindings: &[(&Name, ScalarBinding)],
    ) -> Result<LoweredValue, ReasonCode> {
        let name = variable_name(node).ok_or(ReasonCode::IndOpaquePredicate)?;
        let binding = bindings
            .iter()
            .find_map(|(parameter, binding)| (*parameter == &name).then_some(binding))
            .ok_or(ReasonCode::IndUnresolvedSchema)?;
        match binding {
            ScalarBinding::Column(binding) => {
                self.mark_failure_with_origins(node, &[binding.column.origin()], &[]);
                let expression = ScalarExpression::new(
                    ScalarOperator::Column(binding.column.id()),
                    binding.column.type_ref().clone(),
                    binding.column.multiplicity(),
                    binding.column.nullability(),
                    Knowledge::<Totality>::unknown(),
                    merged_origin(self.file, node.text_range(), &[binding.column.origin()]),
                );
                Ok(LoweredValue::Scalar(LoweredScalar {
                    expression,
                    local: binding.local.clone(),
                }))
            }
            ScalarBinding::RelationRow(row) => Ok(LoweredValue::RelationRow(row.clone())),
        }
    }

    fn lower_relation_column(
        &mut self,
        node: &GreenNode,
        row: &BoundRelationRow,
    ) -> Result<LoweredScalar, ReasonCode> {
        let name = property_name(node).ok_or(ReasonCode::IndOpaquePredicate)?;
        let step = NavigationStep::property(name.clone());
        let outcome = self
            .navigation
            .resolve(&row.local, std::slice::from_ref(&step));
        let NavigationResolution::Found(chain) = outcome else {
            return Err(navigation_reason(outcome));
        };
        let Some(NavigationTarget::RelationColumn(resolved)) =
            chain.hops().first().map(|hop| hop.target())
        else {
            return Err(ReasonCode::IndUnresolvedSchema);
        };
        let index =
            usize::try_from(resolved.id().index()).map_err(|_| ReasonCode::IndUnresolvedSchema)?;
        let binding = row
            .columns
            .get(index)
            .filter(|candidate| candidate.column.name() == &name)
            .ok_or(ReasonCode::IndUnresolvedSchema)?;
        self.mark_failure_with_origins(node, &[binding.column.origin()], &[]);
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
        bindings: &[(&Name, ScalarBinding)],
    ) -> Result<LoweredScalar, ReasonCode> {
        self.mark_failure(node);
        let (operator, left_nodes, right_nodes) = binary_parts(node)?;
        if !matches!(operator, SyntaxKind::EQ | SyntaxKind::NEQ) {
            return Err(ReasonCode::IndOpaquePredicate);
        }
        let left = self.lower_scalar_nodes(&left_nodes, bindings)?;
        let right = self.lower_scalar_nodes(&right_nodes, bindings)?;
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

#[derive(Debug, Clone)]
struct ProjectColumnSpec {
    alias: Name,
    source: GreenNode,
    lambda: LambdaBody,
}

fn bound_relation_row(columns: Vec<BoundColumn>) -> Result<BoundRelationRow, ReasonCode> {
    if columns.is_empty() {
        return Err(ReasonCode::IndUnmodeledOp);
    }
    let relation_columns = columns
        .iter()
        .enumerate()
        .map(|(index, column)| {
            let index = u32::try_from(index).map_err(|_| ReasonCode::IndUnmodeledOp)?;
            Ok(RelationColumn::new(
                RelationColumnId::new(index),
                column.column.name().clone(),
                column.column.type_ref().clone(),
                column.column.multiplicity(),
                column.column.origin().source().range(),
            ))
        })
        .collect::<Result<Vec<_>, ReasonCode>>()?;
    let row = RelationRow::new(relation_columns).map_err(|_| ReasonCode::IndUnresolvedSchema)?;
    Ok(BoundRelationRow {
        local: LocalValue::relation_row(row, exactly_one()?),
        columns,
    })
}

fn selected_bound_element(
    element: Option<BoundElement>,
    columns: &[ColumnId],
) -> Result<Option<BoundElement>, ReasonCode> {
    match element {
        None => Ok(None),
        Some(BoundElement::Column(binding)) => Ok(columns
            .contains(&binding.column.id())
            .then_some(BoundElement::Column(binding))),
        Some(BoundElement::RelationRow(row)) => {
            let selected = columns
                .iter()
                .map(|id| {
                    row.columns
                        .iter()
                        .find(|column| column.column.id() == *id)
                        .cloned()
                        .ok_or(ReasonCode::IndUnmodeledOp)
                })
                .collect::<Result<Vec<_>, _>>()?;
            bound_relation_row(selected)
                .map(BoundElement::RelationRow)
                .map(Some)
        }
    }
}

fn project_column_specs(node: &GreenNode) -> Result<Vec<ProjectColumnSpec>, ReasonCode> {
    let call = arrow_call_arguments(node).ok_or(ReasonCode::IndUnmodeledOp)?;
    let arguments = direct_nodes(&call);
    let [array] = arguments.as_slice() else {
        return Err(ReasonCode::IndUnmodeledOp);
    };
    let specs = project_column_spec_array(array)?;
    let mut aliases = BTreeSet::new();
    if specs.iter().any(|spec| !aliases.insert(spec.alias.clone())) {
        return Err(ReasonCode::IndUnmodeledOp);
    }
    Ok(specs)
}

fn project_column_spec_array(node: &GreenNode) -> Result<Vec<ProjectColumnSpec>, ReasonCode> {
    if node.kind() != SyntaxKind::COLUMN_SPEC_ARRAY || contains_error_node(node) {
        return Err(ReasonCode::IndUnmodeledOp);
    }
    let elements = node
        .children()
        .iter()
        .filter(|element| !element_is_trivia(element))
        .collect::<Vec<_>>();
    let mut index = 0;
    if !takes_token(elements.get(index), SyntaxKind::TILDE) {
        return Err(ReasonCode::IndUnmodeledOp);
    }
    index += 1;
    if !takes_token(elements.get(index), SyntaxKind::BRACKET_OPEN) {
        return Err(ReasonCode::IndUnmodeledOp);
    }
    index += 1;

    let mut specs = Vec::new();
    loop {
        let Some(element) = elements.get(index) else {
            return Err(ReasonCode::IndUnmodeledOp);
        };
        if takes_token(Some(element), SyntaxKind::BRACKET_CLOSE) {
            return if specs.is_empty() {
                Err(ReasonCode::IndUnmodeledOp)
            } else {
                Ok(specs)
            };
        }
        let Some(spec) = element.as_node() else {
            return Err(ReasonCode::IndUnmodeledOp);
        };
        specs.push(project_column_spec(spec)?);
        index += 1;

        if takes_token(elements.get(index), SyntaxKind::BRACKET_CLOSE) {
            return Ok(specs);
        }
        if !takes_token(elements.get(index), SyntaxKind::COMMA) {
            return Err(ReasonCode::IndUnmodeledOp);
        }
        index += 1;
    }
}

fn project_column_spec(node: &GreenNode) -> Result<ProjectColumnSpec, ReasonCode> {
    if node.kind() != SyntaxKind::COLUMN_SPEC || contains_error_node(node) {
        return Err(ReasonCode::IndUnmodeledOp);
    }
    let elements = node
        .children()
        .iter()
        .filter(|element| !element_is_trivia(element))
        .collect::<Vec<_>>();
    let [
        GreenElement::Node(alias),
        GreenElement::Token(colon),
        GreenElement::Node(lambda),
    ] = elements.as_slice()
    else {
        return Err(ReasonCode::IndUnmodeledOp);
    };
    if colon.kind() != SyntaxKind::COLON {
        return Err(ReasonCode::IndUnmodeledOp);
    }
    let alias = project_alias(alias).ok_or(ReasonCode::IndUnmodeledOp)?;
    let lambda = project_lambda_body(lambda).ok_or(ReasonCode::IndUnmodeledOp)?;
    Ok(ProjectColumnSpec {
        alias,
        source: node.clone(),
        lambda,
    })
}

fn project_alias(node: &GreenNode) -> Option<Name> {
    if node.kind() != SyntaxKind::COLUMN_NAME || contains_error_node(node) {
        return None;
    }
    let tokens = significant_tokens(node);
    let [token] = tokens.as_slice() else {
        return None;
    };
    match token.kind() {
        SyntaxKind::IDENT => Name::new(token.text()).ok(),
        SyntaxKind::STRING => pure_string(token.text()).and_then(|name| Name::new(name).ok()),
        _ => None,
    }
}

fn project_lambda_body(node: &GreenNode) -> Option<LambdaBody> {
    if node.kind() != SyntaxKind::LAMBDA_EXPR
        || significant_tokens(node).iter().any(|token| {
            matches!(
                token.kind(),
                SyntaxKind::BRACE_OPEN | SyntaxKind::BRACE_CLOSE
            )
        })
    {
        return None;
    }
    lambda_body(node)
}

#[derive(Debug, Clone)]
struct JoinArguments {
    right: GreenNode,
    left_parameter: Name,
    right_parameter: Name,
    predicate: GreenNode,
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

fn arrow_call_arguments(node: &GreenNode) -> Option<GreenNode> {
    let children = direct_nodes(node);
    let calls = children
        .iter()
        .filter(|child| child.kind() == SyntaxKind::CALL_ARGS)
        .collect::<Vec<_>>();
    let [call] = calls.as_slice() else {
        return None;
    };
    Some((*call).clone())
}

fn strict_distinct_selectors(call: &GreenNode) -> Option<GreenNode> {
    let elements = call
        .children()
        .iter()
        .filter(|element| !element_is_trivia(element))
        .collect::<Vec<_>>();
    let [
        GreenElement::Token(open),
        GreenElement::Node(selectors),
        GreenElement::Token(close),
    ] = elements.as_slice()
    else {
        return None;
    };
    (open.kind() == SyntaxKind::PAREN_OPEN
        && selectors.kind() == SyntaxKind::COLUMN_SPEC_ARRAY
        && close.kind() == SyntaxKind::PAREN_CLOSE)
        .then(|| (*selectors).clone())
}

fn selector_reason(reason: &ColumnSelectorOpaqueReason) -> ReasonCode {
    match reason {
        ColumnSelectorOpaqueReason::Missing(_)
        | ColumnSelectorOpaqueReason::DuplicateSchemaName(_) => ReasonCode::IndUnresolvedSchema,
        ColumnSelectorOpaqueReason::UnsupportedForm
        | ColumnSelectorOpaqueReason::Malformed
        | ColumnSelectorOpaqueReason::UnsupportedBody
        | ColumnSelectorOpaqueReason::DuplicateSelector(_) => ReasonCode::IndUnmodeledOp,
    }
}

#[derive(Debug, Clone)]
struct SortArgument {
    nodes: Vec<GreenNode>,
    range: TextRange,
}

fn call_argument_groups(call: &GreenNode) -> Option<Vec<SortArgument>> {
    if call.kind() != SyntaxKind::CALL_ARGS {
        return None;
    }

    let mut elements = call
        .children()
        .iter()
        .filter(|element| !element_is_trivia(element));
    let Some(GreenElement::Token(open)) = elements.next() else {
        return None;
    };
    if open.kind() != SyntaxKind::PAREN_OPEN {
        return None;
    }

    let mut arguments = Vec::new();
    let mut nodes = Vec::new();
    while let Some(element) = elements.next() {
        match element {
            GreenElement::Token(close) if close.kind() == SyntaxKind::PAREN_CLOSE => {
                if elements.next().is_some() {
                    return None;
                }
                if nodes.is_empty() {
                    return arguments.is_empty().then_some(arguments);
                }
                arguments.push(sort_argument(std::mem::take(&mut nodes))?);
                return Some(arguments);
            }
            GreenElement::Token(comma) if comma.kind() == SyntaxKind::COMMA => {
                arguments.push(sort_argument(std::mem::take(&mut nodes))?);
            }
            GreenElement::Node(node) => nodes.push(node.clone()),
            GreenElement::Token(_) => return None,
        }
    }
    None
}

fn sort_arguments(call: &GreenNode) -> Option<Vec<SortArgument>> {
    let arguments = call_argument_groups(call)?;
    let [argument] = arguments.as_slice() else {
        return Some(arguments);
    };
    let [collection] = argument.nodes.as_slice() else {
        return Some(arguments);
    };
    if collection.kind() != SyntaxKind::COLLECTION_LITERAL {
        return Some(arguments);
    }
    collection_item_groups(collection)
}

fn collection_item_groups(collection: &GreenNode) -> Option<Vec<SortArgument>> {
    if collection.kind() != SyntaxKind::COLLECTION_LITERAL || contains_error_node(collection) {
        return None;
    }
    let mut elements = collection
        .children()
        .iter()
        .filter(|element| !element_is_trivia(element));
    let Some(GreenElement::Token(open)) = elements.next() else {
        return None;
    };
    if open.kind() != SyntaxKind::BRACKET_OPEN {
        return None;
    }

    let mut arguments = Vec::new();
    let mut nodes = Vec::new();
    while let Some(element) = elements.next() {
        match element {
            GreenElement::Token(close) if close.kind() == SyntaxKind::BRACKET_CLOSE => {
                if elements.next().is_some() {
                    return None;
                }
                if nodes.is_empty() {
                    return arguments.is_empty().then_some(arguments);
                }
                arguments.push(sort_argument(std::mem::take(&mut nodes))?);
                return Some(arguments);
            }
            GreenElement::Token(comma) if comma.kind() == SyntaxKind::COMMA => {
                arguments.push(sort_argument(std::mem::take(&mut nodes))?);
            }
            GreenElement::Node(node) => nodes.push(node.clone()),
            GreenElement::Token(_) => return None,
        }
    }
    None
}

fn sort_argument(nodes: Vec<GreenNode>) -> Option<SortArgument> {
    let first = nodes.first()?.text_range();
    let last = nodes.last()?.text_range();
    Some(SortArgument {
        nodes,
        range: TextRange::new(first.start(), last.end()),
    })
}

fn sort_key_parts(nodes: &[GreenNode]) -> Option<(SortDirection, GreenNode)> {
    match nodes {
        [selector, direction]
            if selector.kind() == SyntaxKind::COLUMN_SPEC
                && named_empty_arrow(direction, "ascending") =>
        {
            Some((SortDirection::Ascending, selector.clone()))
        }
        [selector, direction]
            if selector.kind() == SyntaxKind::COLUMN_SPEC
                && named_empty_arrow(direction, "descending") =>
        {
            Some((SortDirection::Descending, selector.clone()))
        }
        [function] => function_sort_selector(function),
        _ => None,
    }
}

fn function_sort_selector(function: &GreenNode) -> Option<(SortDirection, GreenNode)> {
    if function.kind() != SyntaxKind::FUNCTION_CALL {
        return None;
    }
    let children = direct_nodes(function);
    let [name, call] = children.as_slice() else {
        return None;
    };
    let direction = match bare_qualified_name(name)?.as_str() {
        "ascending" => SortDirection::Ascending,
        "descending" => SortDirection::Descending,
        _ => return None,
    };
    let arguments = call_argument_groups(call)?;
    let [argument] = arguments.as_slice() else {
        return None;
    };
    let [selector] = argument.nodes.as_slice() else {
        return None;
    };
    (selector.kind() == SyntaxKind::COLUMN_SPEC).then(|| (direction, selector.clone()))
}

fn named_empty_arrow(node: &GreenNode, expected: &str) -> bool {
    if node.kind() != SyntaxKind::ARROW_CALL {
        return false;
    }
    let children = direct_nodes(node);
    let [name, call] = children.as_slice() else {
        return false;
    };
    bare_qualified_name(name).is_some_and(|name| name.as_str() == expected)
        && empty_call_arguments(call)
}

fn bare_qualified_name(node: &GreenNode) -> Option<Name> {
    if node.kind() != SyntaxKind::QUALIFIED_NAME || !direct_nodes(node).is_empty() {
        return None;
    }
    let path = QName::new(compact_text(node)).ok()?;
    path.package()
        .is_none()
        .then(|| Name::new(path.simple_name()).ok())
        .flatten()
}
fn arrow_lambda(node: &GreenNode) -> Option<LambdaBody> {
    let call = arrow_call_arguments(node)?;
    let arguments = direct_nodes(&call);
    let [lambda] = arguments.as_slice() else {
        return None;
    };
    lambda_body(lambda)
}

fn join_arguments(node: &GreenNode) -> Result<JoinArguments, ReasonCode> {
    let call = arrow_call_arguments(node).ok_or(ReasonCode::IndUnmodeledOp)?;
    let arguments = direct_nodes(&call);
    let [right, kind, kind_member, lambda] = arguments.as_slice() else {
        return Err(ReasonCode::IndUnmodeledOp);
    };
    if kind.kind() != SyntaxKind::QUALIFIED_NAME
        || compact_text(kind) != "JoinKind"
        || property_name(kind_member).is_none_or(|name| name.as_str() != "INNER")
    {
        return Err(ReasonCode::IndUnmodeledOp);
    }
    let (left_parameter, right_parameter, predicate) = join_lambda_body(lambda)?;
    Ok(JoinArguments {
        right: right.clone(),
        left_parameter,
        right_parameter,
        predicate,
    })
}

fn join_lambda_body(node: &GreenNode) -> Result<(Name, Name, GreenNode), ReasonCode> {
    if node.kind() != SyntaxKind::LAMBDA_EXPR {
        return Err(ReasonCode::IndUnmodeledOp);
    }
    let tokens = significant_tokens(node);
    let (Some(first), Some(last)) = (tokens.first(), tokens.last()) else {
        return Err(ReasonCode::IndUnmodeledOp);
    };
    if first.kind() != SyntaxKind::BRACE_OPEN || last.kind() != SyntaxKind::BRACE_CLOSE {
        return Err(ReasonCode::IndUnmodeledOp);
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
    let ([parameters], [block]) = (parameters.as_slice(), blocks.as_slice()) else {
        return Err(ReasonCode::IndUnmodeledOp);
    };
    let (left_parameter, right_parameter) =
        join_lambda_parameters(parameters).ok_or(ReasonCode::IndUnmodeledOp)?;
    if left_parameter == right_parameter {
        return Err(ReasonCode::IndOpaquePredicate);
    }
    let body_nodes = direct_nodes(block);
    let [body] = body_nodes.as_slice() else {
        return Err(ReasonCode::IndUnmodeledOp);
    };
    if body.kind() != SyntaxKind::QUERY_EXPR {
        return Err(ReasonCode::IndUnmodeledOp);
    }
    Ok((left_parameter, right_parameter, body.clone()))
}

fn join_lambda_parameters(node: &GreenNode) -> Option<(Name, Name)> {
    if !direct_nodes(node).is_empty()
        || !significant_tokens(node)
            .iter()
            .map(|token| token.kind())
            .eq([SyntaxKind::IDENT, SyntaxKind::COMMA, SyntaxKind::IDENT])
    {
        return None;
    }
    let names = node
        .tokens()
        .filter(|token| token.kind() == SyntaxKind::IDENT)
        .collect::<Vec<_>>();
    let [left, right] = names.as_slice() else {
        return None;
    };
    Some((Name::new(left.text()).ok()?, Name::new(right.text()).ok()?))
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

fn references_column(expression: &ScalarExpression, column: ColumnId) -> bool {
    match expression.operator() {
        ScalarOperator::Column(candidate) => *candidate == column,
        ScalarOperator::Literal(_) => false,
        ScalarOperator::Navigation { input, .. } | ScalarOperator::Not { input } => {
            references_column(input, column)
        }
        ScalarOperator::Equal { left, right } => {
            references_column(left, column) || references_column(right, column)
        }
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
    Multiplicity::new(EXACTLY_ONE, Some(EXACTLY_ONE)).map_err(|_| ReasonCode::IndUnmodeledOp)
}

fn is_exactly_one(multiplicity: Multiplicity) -> bool {
    multiplicity.lower() == EXACTLY_ONE && multiplicity.upper() == Some(EXACTLY_ONE)
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
    merged_source_origin_with_models(SourceSpan::new(file, range), origins, extra_model_origins)
}

fn merged_source_origin(source: SourceSpan, origins: &[&IrOrigin]) -> IrOrigin {
    merged_source_origin_with_models(source, origins, &[])
}

fn merged_source_origin_with_models(
    source: SourceSpan,
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
    IrOrigin::new(source, model_origins)
}

fn opaque(reason: ReasonCode, origin: IrOrigin) -> RelationalOutcome {
    RelationalOutcome::opaque(OpaqueOutcome::new(reason, origin))
}

fn takes_token(element: Option<&&GreenElement>, kind: SyntaxKind) -> bool {
    matches!(element, Some(GreenElement::Token(token)) if token.kind() == kind)
}

fn significant_tokens(node: &GreenNode) -> Vec<&pure_analyzer_syntax::GreenToken> {
    node.tokens()
        .filter(|token| !is_trivia(token.kind()))
        .collect()
}

fn significant_range(node: &GreenNode) -> Option<TextRange> {
    let tokens = significant_tokens(node);
    let first = tokens.first()?;
    let last = tokens.last()?;
    Some(TextRange::new(
        first.text_range().start(),
        last.text_range().end(),
    ))
}

fn compact_text(node: &GreenNode) -> String {
    node.tokens()
        .filter(|token| !is_trivia(token.kind()))
        .map(|token| token.text())
        .collect()
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use pure_analyzer_lexer::lex;
    use pure_analyzer_model::ModelGraph;
    use pure_analyzer_parser::parse_query;
    use pure_analyzer_syntax::{GreenNodeBuilder, TextSize};

    use super::*;

    const ZERO: u32 = 0;

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
    fn arrow_call_arguments_requires_exactly_one_direct_call() {
        let source = "->distinct()()";
        let tokens = lex(source);
        let mut builder = GreenNodeBuilder::new(source, &tokens);
        builder.open(SyntaxKind::ROOT);
        builder.open(SyntaxKind::ARROW_CALL);
        builder.advance();
        builder.open(SyntaxKind::QUALIFIED_NAME);
        builder.advance();
        builder.close();
        for _ in 0..2 {
            builder.open(SyntaxKind::CALL_ARGS);
            builder.advance();
            builder.advance();
            builder.close();
        }
        builder.close();
        builder.close();
        let root = builder.finish().expect("fixture tree must build");
        let arrow = direct_nodes(&root)
            .into_iter()
            .next()
            .expect("fixture must contain an arrow call");

        assert_eq!(arrow.kind(), SyntaxKind::ARROW_CALL);
        assert_eq!(arrow_call_arguments(&arrow), None);
    }

    #[test]
    fn sort_key_groups_preserve_verified_collection_order_and_direction_forms() {
        let source = "model::Person.all()->sort([ascending(~first), ~second->descending()])";
        let parsed = parse_query(source, FileId::new(91)).expect("fixture source must parse");
        assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
        let query = top_level_queries(&parsed.green)
            .into_iter()
            .next()
            .expect("fixture must contain one query");
        let sort = direct_nodes(&query)
            .into_iter()
            .find(|node| {
                node.kind() == SyntaxKind::ARROW_CALL
                    && arrow_name(node).is_some_and(|name| name.as_str() == "sort")
            })
            .expect("fixture must contain a sort arrow call");
        let call = arrow_call_arguments(&sort).expect("sort must have arguments");
        let arguments = sort_arguments(&call).expect("sort arguments must group exactly");

        assert_eq!(arguments.len(), 2);
        let (first_direction, first_selector) =
            sort_key_parts(&arguments[0].nodes).expect("first key must be proven");
        let (second_direction, second_selector) =
            sort_key_parts(&arguments[1].nodes).expect("second key must be proven");
        assert_eq!(first_direction, SortDirection::Ascending);
        assert_eq!(first_selector.text(), "~first");
        assert_eq!(second_direction, SortDirection::Descending);
        assert_eq!(second_selector.text(), "~second");
        assert_eq!(
            &source[usize::from(arguments[0].range.start())..usize::from(arguments[0].range.end())],
            "ascending(~first)"
        );
        assert_eq!(
            &source[usize::from(arguments[1].range.start())..usize::from(arguments[1].range.end())],
            "~second->descending()"
        );
    }

    #[test]
    fn sort_column_resolution_treats_duplicate_schema_names_as_unresolved() {
        let source = "~value->ascending()";
        let file = FileId::new(91);
        let parsed = parse_query(source, file).expect("fixture source must parse");
        assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
        let query = top_level_queries(&parsed.green)
            .into_iter()
            .next()
            .expect("fixture must contain one query");
        let selector = direct_nodes(&query)
            .into_iter()
            .find(|node| node.kind() == SyntaxKind::COLUMN_SPEC)
            .expect("fixture must contain a column selector");
        let multiplicity =
            Multiplicity::new(EXACTLY_ONE, Some(EXACTLY_ONE)).expect("fixture multiplicity");
        let type_ref = TypeRef::new(
            QName::new(STRING_TYPE).expect("fixture type path must be valid"),
            Vec::new(),
        );
        let schema = RelationSchema::new(vec![
            Column::new(
                ColumnId::new(ZERO),
                Name::new("value").expect("fixture name must be valid"),
                type_ref.clone(),
                multiplicity,
                Nullability::Unknown,
                origin(file, selector.text_range(), Vec::new()),
            ),
            Column::new(
                ColumnId::new(EXACTLY_ONE),
                Name::new("value").expect("fixture name must be valid"),
                type_ref,
                multiplicity,
                Nullability::Unknown,
                origin(file, selector.text_range(), Vec::new()),
            ),
        ])
        .expect("fixture schema must be valid");
        let model = ModelGraph::default();
        let mut lowerer = QueryLowerer::new(file, &model, query.text_range());
        let input_origin = origin(file, query.text_range(), Vec::new());

        assert_eq!(
            lowerer.resolve_sort_column(&selector, &schema, &input_origin),
            Err(ReasonCode::IndUnresolvedSchema)
        );
        assert_eq!(
            &source[usize::from(lowerer.failure_origin().source().range().start())
                ..usize::from(lowerer.failure_origin().source().range().end())],
            "value"
        );
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

    /// Build `depth` levels of nested `PAREN_EXPR` wrapping one token, so
    /// [`contains_error_node`]'s own recursion can be exercised at a depth no
    /// real parse ever reaches (`pure_analyzer_parser::m3::MAX_PARSE_DEPTH`
    /// already refuses source nested past 256; see `MAX_SYNTAX_TREE_DEPTH`).
    fn nested_parens(depth: usize) -> GreenNode {
        let source = "x";
        let tokens = lex(source);
        let mut builder = GreenNodeBuilder::new(source, &tokens);
        builder.open(SyntaxKind::ROOT);
        for _ in 0..depth {
            builder.open(SyntaxKind::PAREN_EXPR);
        }
        builder.advance();
        for _ in 0..depth {
            builder.close();
        }
        builder.close();
        let root = builder.finish().expect("fixture tree must build");
        direct_nodes(&root)
            .into_iter()
            .next()
            .expect("fixture must contain the outermost paren")
    }

    #[test]
    fn contains_error_node_stops_at_its_depth_budget_in_both_directions() {
        // Pinned exactly, in both directions: lowering the budget would
        // reject ordinary parser-accepted nesting (see the module doc on
        // `MAX_SYNTAX_TREE_DEPTH`); raising it defeats the stack-overflow
        // guard this test exists to prove.
        const EXPECTED_DEPTH_BUDGET: usize = 512;

        // `EXPECTED_DEPTH_BUDGET` nested nodes put the deepest visited call at
        // depth `EXPECTED_DEPTH_BUDGET - 1`, one below the `>=` guard.
        let at_budget = nested_parens(EXPECTED_DEPTH_BUDGET);
        assert!(
            !contains_error_node(&at_budget),
            "nesting exactly inside the budget must not be reported as an error"
        );

        let over_budget = nested_parens(EXPECTED_DEPTH_BUDGET * 2);
        assert!(
            contains_error_node(&over_budget),
            "nesting past the budget must fail closed as if it were an error, not recurse further"
        );
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
        let one_or_more =
            Multiplicity::new(EXACTLY_ONE, None).expect("fixture multiplicity must be valid");

        assert!(!is_exactly_one(one_or_more));
    }

    #[test]
    fn trivia_classification_retains_whitespace() {
        assert!(is_trivia(SyntaxKind::WHITESPACE));
        assert!(!is_trivia(SyntaxKind::IDENT));
    }

    /// Build the single outer child of a hand-built fixture tree.
    fn outer_node(source: &str, build: impl FnOnce(&mut GreenNodeBuilder<'_>)) -> GreenNode {
        let tokens = lex(source);
        let mut builder = GreenNodeBuilder::new(source, &tokens);
        builder.open(SyntaxKind::ROOT);
        build(&mut builder);
        builder.close();
        let root = builder.finish().expect("fixture tree must build");
        direct_nodes(&root)
            .into_iter()
            .next()
            .expect("fixture must contain its outer node")
    }

    /// A valid `x:row` column spec's children (alias, colon, lambda), built
    /// under whatever kind the caller opens. `project_lambda_body` forbids
    /// direct brace tokens, so this lambda is deliberately brace-free.
    fn valid_column_spec_children(builder: &mut GreenNodeBuilder<'_>) {
        builder.open(SyntaxKind::COLUMN_NAME);
        builder.advance(); // x
        builder.close();
        builder.advance(); // :
        builder.open(SyntaxKind::LAMBDA_EXPR);
        builder.open(SyntaxKind::LAMBDA_PARAMS);
        builder.advance(); // row
        builder.close();
        builder.open(SyntaxKind::CODE_BLOCK);
        builder.open(SyntaxKind::QUERY_EXPR);
        builder.close();
        builder.close();
        builder.close();
    }

    /// Regression for a `project_column_spec` `||` -> `&&` mutant: a
    /// mislabeled outer node whose children are otherwise a fully valid
    /// alias/colon/lambda triple must still be rejected on kind alone, not
    /// silently accepted because no error node happens to be present.
    #[test]
    fn project_column_spec_rejects_a_mislabeled_but_otherwise_valid_node() {
        let node = outer_node("x:row", |builder| {
            builder.open(SyntaxKind::ARROW_CALL);
            valid_column_spec_children(builder);
            builder.close();
        });

        assert_eq!(node.kind(), SyntaxKind::ARROW_CALL);
        assert!(matches!(
            project_column_spec(&node),
            Err(ReasonCode::IndUnmodeledOp)
        ));
    }

    /// Regression for a `project_column_spec_array` `||` -> `&&` mutant: same
    /// technique, one level up (`~[x:row]` under a mislabeled outer kind).
    #[test]
    fn project_column_spec_array_rejects_a_mislabeled_but_otherwise_valid_node() {
        let node = outer_node("~[x:row]", |builder| {
            builder.open(SyntaxKind::ARROW_CALL);
            builder.advance(); // ~
            builder.advance(); // [
            builder.open(SyntaxKind::COLUMN_SPEC);
            valid_column_spec_children(builder);
            builder.close();
            builder.advance(); // ]
            builder.close();
        });

        assert_eq!(node.kind(), SyntaxKind::ARROW_CALL);
        assert!(matches!(
            project_column_spec_array(&node),
            Err(ReasonCode::IndUnmodeledOp)
        ));
    }

    /// Regression for a `project_alias` `||` -> `&&` mutant: a mislabeled
    /// outer node wrapping one otherwise-valid `IDENT` must still be
    /// rejected on kind alone.
    #[test]
    fn project_alias_rejects_a_mislabeled_but_otherwise_valid_node() {
        let node = outer_node("foo", |builder| {
            builder.open(SyntaxKind::ARROW_CALL);
            builder.advance(); // foo
            builder.close();
        });

        assert_eq!(node.kind(), SyntaxKind::ARROW_CALL);
        assert_eq!(project_alias(&node), None);
    }

    /// Regression for a `bare_qualified_name` `||` -> `&&` mutant: a
    /// mislabeled outer node with no nested child nodes, wrapping otherwise
    /// valid `IDENT` text, must still be rejected on kind alone.
    #[test]
    fn bare_qualified_name_rejects_a_mislabeled_but_otherwise_valid_node() {
        let node = outer_node("Foo", |builder| {
            builder.open(SyntaxKind::ARROW_CALL);
            builder.advance(); // Foo
            builder.close();
        });

        assert_eq!(node.kind(), SyntaxKind::ARROW_CALL);
        assert_eq!(bare_qualified_name(&node), None);
    }

    /// Regression for a `collection_item_groups` `||` -> `&&` mutant: a
    /// mislabeled outer node wrapping an otherwise well-formed one-element
    /// bracketed group must still be rejected on kind alone.
    #[test]
    fn collection_item_groups_rejects_a_mislabeled_but_otherwise_valid_node() {
        let node = outer_node("[5]", |builder| {
            builder.open(SyntaxKind::ARROW_CALL);
            builder.advance(); // [
            builder.open(SyntaxKind::LITERAL_EXPR);
            builder.advance(); // 5
            builder.close();
            builder.advance(); // ]
            builder.close();
        });

        assert_eq!(node.kind(), SyntaxKind::ARROW_CALL);
        assert!(collection_item_groups(&node).is_none());
    }

    /// Regression for a `join_lambda_body` `||` -> `&&` mutant on its
    /// leading/trailing brace check: a lambda whose *first* significant
    /// token is not `{` (but whose last is `}`) must still be rejected, not
    /// accepted just because the closing brace happens to be present.
    #[test]
    fn join_lambda_body_rejects_a_missing_open_brace_with_a_present_close_brace() {
        let node = outer_node("x,y}", |builder| {
            builder.open(SyntaxKind::LAMBDA_EXPR);
            builder.open(SyntaxKind::LAMBDA_PARAMS);
            builder.advance(); // x
            builder.advance(); // ,
            builder.advance(); // y
            builder.close();
            builder.open(SyntaxKind::CODE_BLOCK);
            builder.open(SyntaxKind::QUERY_EXPR);
            builder.close();
            builder.close();
            builder.advance(); // }
            builder.close();
        });

        assert_eq!(node.kind(), SyntaxKind::LAMBDA_EXPR);
        assert!(matches!(
            join_lambda_body(&node),
            Err(ReasonCode::IndUnmodeledOp)
        ));
    }

    /// Regression for a `join_lambda_parameters` `||` -> `&&` mutant: an
    /// extra, token-free nested node makes `direct_nodes` non-empty while
    /// leaving the significant `IDENT, COMMA, IDENT` token sequence intact,
    /// so only the node-emptiness half of the guard can reject it.
    #[test]
    fn join_lambda_parameters_rejects_an_extra_empty_child_node() {
        let node = outer_node("x,y", |builder| {
            builder.open(SyntaxKind::LAMBDA_PARAMS);
            builder.advance(); // x
            builder.advance(); // ,
            builder.open(SyntaxKind::QUERY_EXPR);
            builder.close();
            builder.advance(); // y
            builder.close();
        });

        assert_eq!(node.kind(), SyntaxKind::LAMBDA_PARAMS);
        assert!(!direct_nodes(&node).is_empty());
        assert_eq!(join_lambda_parameters(&node), None);
    }

    /// Regression for `call_argument_groups`' comma-arm match-guard becoming
    /// unconditional: a non-comma separator (`;`) between two argument nodes
    /// must still be rejected, not silently treated as if it were a comma.
    #[test]
    fn call_argument_groups_rejects_a_non_comma_separator() {
        let node = outer_node("(1;2)", |builder| {
            builder.open(SyntaxKind::CALL_ARGS);
            builder.advance(); // (
            builder.open(SyntaxKind::LITERAL_EXPR);
            builder.advance(); // 1
            builder.close();
            builder.advance(); // ;
            builder.open(SyntaxKind::LITERAL_EXPR);
            builder.advance(); // 2
            builder.close();
            builder.advance(); // )
            builder.close();
        });

        assert_eq!(node.kind(), SyntaxKind::CALL_ARGS);
        assert!(call_argument_groups(&node).is_none());
    }

    /// Regression for `collection_item_groups`' comma-arm match-guard
    /// becoming unconditional: same shape as
    /// `call_argument_groups_rejects_a_non_comma_separator`, one level up in
    /// a `COLLECTION_LITERAL`.
    #[test]
    fn collection_item_groups_rejects_a_non_comma_separator() {
        let node = outer_node("[1;2]", |builder| {
            builder.open(SyntaxKind::COLLECTION_LITERAL);
            builder.advance(); // [
            builder.open(SyntaxKind::LITERAL_EXPR);
            builder.advance(); // 1
            builder.close();
            builder.advance(); // ;
            builder.open(SyntaxKind::LITERAL_EXPR);
            builder.advance(); // 2
            builder.close();
            builder.advance(); // ]
            builder.close();
        });

        assert_eq!(node.kind(), SyntaxKind::COLLECTION_LITERAL);
        assert!(collection_item_groups(&node).is_none());
    }

    /// Regression for a `references_column` `==` -> `!=` mutant: a column
    /// read must be reported as referencing exactly the column it names, not
    /// every column except it.
    #[test]
    fn references_column_matches_only_the_named_column_id() {
        let file = FileId::new(91);
        let source = origin(
            file,
            TextRange::new(TextSize::from(0), TextSize::from(1)),
            Vec::new(),
        );
        let target = ColumnId::new(7);
        let other = ColumnId::new(8);
        let expression = ScalarExpression::new(
            ScalarOperator::Column(target),
            TypeRef::new(
                QName::new(STRING_TYPE).expect("fixture type is valid"),
                Vec::new(),
            ),
            Multiplicity::new(EXACTLY_ONE, Some(EXACTLY_ONE)).expect("fixture multiplicity"),
            Nullability::NonNullable,
            Knowledge::<Totality>::unknown(),
            source,
        );

        assert!(references_column(&expression, target));
        assert!(!references_column(&expression, other));
    }
}
