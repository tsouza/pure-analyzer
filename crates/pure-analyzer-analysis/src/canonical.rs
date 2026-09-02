//! Fail-closed emission of a narrow, proven relational normal-form subset.
//!
//! This is deliberately not the source-layout formatter. It accepts only
//! normal forms whose observable facts can be recreated by the supported M3
//! query syntax, and otherwise returns a typed indecision.

use std::collections::{BTreeMap, BTreeSet};

use pure_analyzer_diagnostics::ReasonCode;

use crate::{
    ColumnId, IrOrigin, Knowledge, NormalizationBudget, NormalizationOutcome, NormalizedQuery,
    RelationExpression, RelationFacts, RelationOperator, RelationSchema, RelationSource,
    RelationalOutcome, RowSemantics, ScalarExpression, ScalarLiteral, ScalarOperator,
    SortDirection, normalize_relational_query_with_budget,
};

const MAP_OUTPUT_NAME: &str = "value";

/// Deterministic Pure text emitted from a supported relational normal form.
///
/// The text intentionally contains no source trivia or comments. Canonical
/// emission makes no claim to preserve source layout; callers that need that
/// contract must use the lossless layout formatter instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalPure {
    text: String,
}

impl CanonicalPure {
    /// Borrow the deterministic emitted Pure query text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.text
    }

    /// Consume this result into its deterministic Pure query text.
    #[must_use]
    pub fn into_string(self) -> String {
        self.text
    }
}

/// A source-anchored reason canonical emission could not make a safe claim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalEmissionIndecision {
    reason: ReasonCode,
    origin: IrOrigin,
}

impl CanonicalEmissionIndecision {
    fn new(reason: ReasonCode, origin: IrOrigin) -> Self {
        Self { reason, origin }
    }

    /// Return the registered reason that prevents canonical emission.
    #[must_use]
    pub const fn reason(&self) -> ReasonCode {
        self.reason
    }

    /// Return the exact query/model origin associated with the refusal.
    #[must_use]
    pub const fn origin(&self) -> &IrOrigin {
        &self.origin
    }
}

/// The fail-closed result of canonical Pure emission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CanonicalEmissionOutcome {
    /// A supported normal form was emitted as deterministic Pure text.
    Emitted(CanonicalPure),
    /// Emission declined because normalization or syntax support is incomplete.
    Indecisive(CanonicalEmissionIndecision),
}

impl CanonicalEmissionOutcome {
    /// Borrow emitted Pure text when the supported subset accepted the input.
    #[must_use]
    pub const fn emitted(&self) -> Option<&CanonicalPure> {
        match self {
            Self::Emitted(value) => Some(value),
            Self::Indecisive(_) => None,
        }
    }

    /// Borrow the typed refusal when no deterministic Pure query was emitted.
    #[must_use]
    pub const fn indecision(&self) -> Option<&CanonicalEmissionIndecision> {
        match self {
            Self::Emitted(_) => None,
            Self::Indecisive(value) => Some(value),
        }
    }
}

/// Emit deterministic Pure from one proven relational normal form.
///
/// The supported subset contains resolved class scans, the currently lowered
/// filter/project/map/join/distinct/selected-distinct/sort forms, and the
/// scalar forms their parser/lowerer can recreate. Every fact or operator that
/// cannot be represented without changing the semantic normal form returns
/// [`CanonicalEmissionOutcome::Indecisive`].
#[must_use]
pub fn emit_canonical_normal_form(normalized: &NormalizedQuery) -> CanonicalEmissionOutcome {
    match Emitter::default().relation(normalized.root()) {
        Ok(emitted) => CanonicalEmissionOutcome::Emitted(CanonicalPure { text: emitted.text }),
        Err(failure) => CanonicalEmissionOutcome::Indecisive(CanonicalEmissionIndecision::new(
            ReasonCode::IndUnmodeledOp,
            failure.origin,
        )),
    }
}

/// Emit deterministic Pure from either a proven normal form or its explicit failure.
///
/// A normalization failure is propagated as the same reason and origin rather
/// than being turned into a partial or guessed canonical query.
#[must_use]
pub fn emit_canonical_normalization(
    normalization: &NormalizationOutcome,
) -> CanonicalEmissionOutcome {
    match normalization {
        NormalizationOutcome::Normalized(normalized) => emit_canonical_normal_form(normalized),
        NormalizationOutcome::Indecisive(failure) => CanonicalEmissionOutcome::Indecisive(
            CanonicalEmissionIndecision::new(failure.reason(), failure.origin().clone()),
        ),
    }
}

/// Emit deterministic Pure from a lowered query using the default finite normalization budget.
///
/// Opaque lowering is preserved as the original typed indecision. A supported
/// lowering is normalized before emission, so this helper never emits text
/// from an unproven relational representation.
#[must_use]
pub fn emit_canonical_lowered_query(lowered: &RelationalOutcome) -> CanonicalEmissionOutcome {
    emit_canonical_lowered_query_with_budget(lowered, NormalizationBudget::default())
}

/// Emit deterministic Pure from a lowered query with an explicit finite normalization budget.
///
/// This is the single-query counterpart to the guarded comparison boundary:
/// opaque lowering, exhausted normalization, and unsupported normal forms all
/// remain typed indecisions rather than becoming partial emitted text.
#[must_use]
pub fn emit_canonical_lowered_query_with_budget(
    lowered: &RelationalOutcome,
    budget: NormalizationBudget,
) -> CanonicalEmissionOutcome {
    match lowered {
        RelationalOutcome::Supported(query) => {
            emit_canonical_normalization(&normalize_relational_query_with_budget(query, budget))
        }
        RelationalOutcome::Opaque(opaque) => CanonicalEmissionOutcome::Indecisive(
            CanonicalEmissionIndecision::new(opaque.reason(), opaque.origin().clone()),
        ),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BindingKind {
    Column,
    Row,
    None,
}

struct EmittedRelation {
    text: String,
    binding: BindingKind,
}

struct EmissionFailure {
    origin: IrOrigin,
}

impl EmissionFailure {
    fn unsupported(origin: &IrOrigin) -> Self {
        Self {
            origin: origin.clone(),
        }
    }
}

type EmissionResult<T> = Result<T, EmissionFailure>;

#[derive(Default)]
struct Emitter {
    next_binder: usize,
}

impl Emitter {
    fn relation(&mut self, expression: &RelationExpression) -> EmissionResult<EmittedRelation> {
        match expression.operator() {
            RelationOperator::Scan(source) => self.scan(expression, source),
            RelationOperator::Filter { input, predicate } => {
                self.filter(expression, input, predicate)
            }
            RelationOperator::Project { input, projections } => {
                self.project(expression, input, projections)
            }
            RelationOperator::Join {
                kind,
                left,
                right,
                condition,
            } => self.join(expression, *kind, left, right, condition),
            RelationOperator::Distinct { input } => self.distinct(expression, input),
            RelationOperator::DistinctOn { input, columns } => {
                self.distinct_on(expression, input, columns)
            }
            RelationOperator::Sort { input, keys } => self.sort(expression, input, keys),
        }
    }

    fn scan(
        &mut self,
        expression: &RelationExpression,
        source: &RelationSource,
    ) -> EmissionResult<EmittedRelation> {
        let RelationSource::Class(class) = source else {
            return Err(EmissionFailure::unsupported(expression.origin()));
        };
        if !facts_are_unknown(expression.facts())
            || !is_emittable_path(class.path().as_str())
            || !class_scan_schema_matches(expression.schema(), class.path().as_str())
        {
            return Err(EmissionFailure::unsupported(expression.origin()));
        }
        Ok(EmittedRelation {
            text: format!("{}.all()", class.path().as_str()),
            binding: BindingKind::Column,
        })
    }

    fn filter(
        &mut self,
        expression: &RelationExpression,
        input_expression: &RelationExpression,
        predicate: &ScalarExpression,
    ) -> EmissionResult<EmittedRelation> {
        let input = self.relation(input_expression)?;
        if expression.schema() != input_expression.schema()
            || !facts_match(expression.facts(), input_expression.facts())
        {
            return Err(EmissionFailure::unsupported(expression.origin()));
        }
        let binder = self.binder(expression.origin())?;
        let references = references_for_binding(input.binding, input_expression.schema(), &binder)
            .map_err(|_| EmissionFailure::unsupported(expression.origin()))?;
        let predicate = self.scalar(predicate, &references)?;
        Ok(EmittedRelation {
            text: format!("{}->filter({binder}| {predicate})", input.text),
            binding: input.binding,
        })
    }

    fn project(
        &mut self,
        expression: &RelationExpression,
        input_expression: &RelationExpression,
        projections: &[crate::Projection],
    ) -> EmissionResult<EmittedRelation> {
        let input = self.relation(input_expression)?;
        if !facts_are_unknown(expression.facts()) || !project_shape_matches(expression, projections)
        {
            return Err(EmissionFailure::unsupported(expression.origin()));
        }

        let use_map = is_map_shape(expression, projections);
        if use_map && input.binding != BindingKind::None {
            let binder = self.binder(expression.origin())?;
            let references =
                references_for_binding(input.binding, input_expression.schema(), &binder)
                    .map_err(|_| EmissionFailure::unsupported(expression.origin()))?;
            let scalar = self.scalar(projections[0].expression(), &references)?;
            return Ok(EmittedRelation {
                text: format!("{}->map({binder}| {scalar})", input.text),
                binding: BindingKind::Column,
            });
        }

        if input.binding != BindingKind::Column || !schema_names_are_unique(expression.schema()) {
            return Err(EmissionFailure::unsupported(expression.origin()));
        }
        let binder = self.binder(expression.origin())?;
        let references = references_for_binding(input.binding, input_expression.schema(), &binder)
            .map_err(|_| EmissionFailure::unsupported(expression.origin()))?;
        let specs = projections
            .iter()
            .zip(expression.schema().columns())
            .map(|(projection, column)| {
                self.scalar(projection.expression(), &references)
                    .map(|scalar| {
                        format!(
                            "{}: {binder} | {scalar}",
                            column_name(column.name().as_str())
                        )
                    })
            })
            .collect::<EmissionResult<Vec<_>>>()?;
        Ok(EmittedRelation {
            text: format!("{}->project(~[{}])", input.text, specs.join(", ")),
            binding: BindingKind::Row,
        })
    }

    fn join(
        &mut self,
        expression: &RelationExpression,
        kind: crate::JoinKind,
        left_expression: &RelationExpression,
        right_expression: &RelationExpression,
        condition: &ScalarExpression,
    ) -> EmissionResult<EmittedRelation> {
        let left = self.relation(left_expression)?;
        let right = self.relation(right_expression)?;
        if !facts_are_unknown(expression.facts())
            || !join_schema_matches(
                expression,
                left_expression.schema(),
                right_expression.schema(),
            )
            || left.binding != BindingKind::Column
            || right.binding != BindingKind::Column
            || kind != crate::JoinKind::Inner
        {
            return Err(EmissionFailure::unsupported(expression.origin()));
        }
        let left_binder = self.binder(expression.origin())?;
        let right_binder = self.binder(expression.origin())?;
        let mut references =
            references_for_binding(left.binding, left_expression.schema(), &left_binder)
                .map_err(|_| EmissionFailure::unsupported(expression.origin()))?;
        for (column, reference) in
            references_for_binding(right.binding, right_expression.schema(), &right_binder)
                .map_err(|_| EmissionFailure::unsupported(expression.origin()))?
        {
            if references.insert(column, reference).is_some() {
                return Err(EmissionFailure::unsupported(expression.origin()));
            }
        }
        let condition = self.scalar(condition, &references)?;
        Ok(EmittedRelation {
            text: format!(
                "{}->join({}, JoinKind.INNER, {{{left_binder}, {right_binder} | {condition}}})",
                left.text, right.text
            ),
            binding: BindingKind::None,
        })
    }

    fn distinct(
        &mut self,
        expression: &RelationExpression,
        input_expression: &RelationExpression,
    ) -> EmissionResult<EmittedRelation> {
        let input = self.relation(input_expression)?;
        if input.binding == BindingKind::None
            || expression.schema() != input_expression.schema()
            || !facts_are_distinct_set(expression.facts())
        {
            return Err(EmissionFailure::unsupported(expression.origin()));
        }
        Ok(EmittedRelation {
            text: format!("{}->distinct()", input.text),
            binding: input.binding,
        })
    }

    fn distinct_on(
        &mut self,
        expression: &RelationExpression,
        input_expression: &RelationExpression,
        columns: &[ColumnId],
    ) -> EmissionResult<EmittedRelation> {
        let input = self.relation(input_expression)?;
        if !facts_are_unknown(expression.facts())
            || !schema_names_are_unique(input_expression.schema())
        {
            return Err(EmissionFailure::unsupported(expression.origin()));
        }
        let selectors = columns
            .iter()
            .map(|column| {
                input_expression
                    .schema()
                    .column(*column)
                    .map(|column| column_name(column.name().as_str()))
                    .ok_or_else(|| EmissionFailure::unsupported(expression.origin()))
            })
            .collect::<EmissionResult<Vec<_>>>()?;
        if selectors.is_empty() {
            return Err(EmissionFailure::unsupported(expression.origin()));
        }
        Ok(EmittedRelation {
            text: format!("{}->distinct(~[{}])", input.text, selectors.join(", ")),
            binding: input.binding,
        })
    }

    fn sort(
        &mut self,
        expression: &RelationExpression,
        input_expression: &RelationExpression,
        keys: &[crate::SortKey],
    ) -> EmissionResult<EmittedRelation> {
        let input = self.relation(input_expression)?;
        if expression.schema() != input_expression.schema()
            || !facts_match(expression.facts(), input_expression.facts())
            || !schema_names_are_unique(input_expression.schema())
        {
            return Err(EmissionFailure::unsupported(expression.origin()));
        }
        let keys = keys
            .iter()
            .map(|key| {
                let column = input_expression
                    .schema()
                    .column(key.column())
                    .ok_or_else(|| EmissionFailure::unsupported(expression.origin()))?;
                let direction = match key.direction() {
                    SortDirection::Ascending => "ascending",
                    SortDirection::Descending => "descending",
                };
                Ok(format!(
                    "{direction}(~{})",
                    column_name(column.name().as_str())
                ))
            })
            .collect::<EmissionResult<Vec<_>>>()?;
        if keys.is_empty() {
            return Err(EmissionFailure::unsupported(expression.origin()));
        }
        Ok(EmittedRelation {
            text: format!("{}->sort([{}])", input.text, keys.join(", ")),
            binding: input.binding,
        })
    }

    fn scalar(
        &mut self,
        expression: &ScalarExpression,
        references: &BTreeMap<ColumnId, String>,
    ) -> EmissionResult<String> {
        if !expression.totality().is_unknown() {
            return Err(EmissionFailure::unsupported(expression.origin()));
        }
        match expression.operator() {
            ScalarOperator::Column(column) => references
                .get(column)
                .cloned()
                .ok_or_else(|| EmissionFailure::unsupported(expression.origin())),
            ScalarOperator::Literal(literal) => literal_text(literal)
                .ok_or_else(|| EmissionFailure::unsupported(expression.origin())),
            ScalarOperator::Navigation { input, navigation } => {
                let input = self.scalar(input, references)?;
                let member = navigation.member().name().as_str();
                if !is_emittable_identifier(member) {
                    return Err(EmissionFailure::unsupported(expression.origin()));
                }
                Ok(format!("{input}.{member}"))
            }
            ScalarOperator::Equal { left, right } => {
                let left = self.scalar(left, references)?;
                let right = self.scalar(right, references)?;
                Ok(format!("({left} == {right})"))
            }
            ScalarOperator::Not { input } => {
                let ScalarOperator::Equal { left, right } = input.operator() else {
                    return Err(EmissionFailure::unsupported(expression.origin()));
                };
                let left = self.scalar(left, references)?;
                let right = self.scalar(right, references)?;
                Ok(format!("({left} != {right})"))
            }
            ScalarOperator::And { .. } | ScalarOperator::Or { .. } => {
                Err(EmissionFailure::unsupported(expression.origin()))
            }
        }
    }

    fn binder(&mut self, origin: &IrOrigin) -> EmissionResult<String> {
        let index = self.next_binder;
        self.next_binder = self
            .next_binder
            .checked_add(1)
            .ok_or_else(|| EmissionFailure::unsupported(origin))?;
        Ok(format!("v{index}"))
    }
}

fn class_scan_schema_matches(schema: &RelationSchema, path: &str) -> bool {
    let [column] = schema.columns() else {
        return false;
    };
    column.name().as_str() == path.rsplit("::").next().unwrap_or(path)
        && column.type_ref().raw_type().as_str() == path
        && column.type_ref().type_arguments().is_empty()
        && column.multiplicity().lower() == 1
        && column.multiplicity().upper() == Some(1)
        && matches!(column.nullability(), crate::Nullability::Unknown)
}

fn project_shape_matches(
    expression: &RelationExpression,
    projections: &[crate::Projection],
) -> bool {
    projections.len() == expression.schema().columns().len()
        && projections
            .iter()
            .zip(expression.schema().columns())
            .all(|(projection, column)| projection.column() == column.id())
}

fn is_map_shape(expression: &RelationExpression, projections: &[crate::Projection]) -> bool {
    projections.len() == 1
        && expression
            .schema()
            .columns()
            .first()
            .is_some_and(|column| column.name().as_str() == MAP_OUTPUT_NAME)
}

fn join_schema_matches(
    expression: &RelationExpression,
    left: &RelationSchema,
    right: &RelationSchema,
) -> bool {
    expression
        .schema()
        .columns()
        .iter()
        .eq(left.columns().iter().chain(right.columns()))
}

fn references_for_binding(
    binding: BindingKind,
    schema: &RelationSchema,
    binder: &str,
) -> Result<BTreeMap<ColumnId, String>, ()> {
    match binding {
        BindingKind::Column => {
            let [column] = schema.columns() else {
                return Err(());
            };
            Ok(BTreeMap::from([(column.id(), format!("${binder}"))]))
        }
        BindingKind::Row => {
            if !schema_names_are_unique(schema)
                || schema
                    .columns()
                    .iter()
                    .any(|column| !is_emittable_identifier(column.name().as_str()))
            {
                return Err(());
            }
            Ok(schema
                .columns()
                .iter()
                .map(|column| (column.id(), format!("${binder}.{}", column.name().as_str())))
                .collect())
        }
        BindingKind::None => Err(()),
    }
}

fn facts_are_unknown(facts: &RelationFacts) -> bool {
    facts.candidate_keys().is_unknown() && facts.row_semantics().is_unknown()
}

fn facts_are_distinct_set(facts: &RelationFacts) -> bool {
    facts.candidate_keys().is_unknown()
        && matches!(
            facts.row_semantics().as_proven(),
            Some((RowSemantics::Set, _))
        )
}

fn facts_match(left: &RelationFacts, right: &RelationFacts) -> bool {
    knowledge_matches(left.candidate_keys(), right.candidate_keys())
        && knowledge_matches(left.row_semantics(), right.row_semantics())
}

fn knowledge_matches<T: PartialEq>(left: &Knowledge<T>, right: &Knowledge<T>) -> bool {
    match (left.as_proven(), right.as_proven()) {
        (None, None) => true,
        (Some((left, _)), Some((right, _))) => left == right,
        _ => false,
    }
}

fn schema_names_are_unique(schema: &RelationSchema) -> bool {
    let mut names = BTreeSet::new();
    schema
        .columns()
        .iter()
        .all(|column| names.insert(column.name().as_str()))
}

fn is_emittable_path(path: &str) -> bool {
    path.split("::").all(is_emittable_identifier)
}

fn is_emittable_identifier(value: &str) -> bool {
    let mut characters = value.chars();
    let Some(first) = characters.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == '_')
        && characters.all(|character| character.is_ascii_alphanumeric() || character == '_')
        && !matches!(value, "true" | "false")
}

fn column_name(value: &str) -> String {
    if is_emittable_identifier(value) {
        value.to_owned()
    } else {
        format!("'{}'", value.replace('\'', "''"))
    }
}

fn literal_text(literal: &ScalarLiteral) -> Option<String> {
    match literal {
        ScalarLiteral::Boolean(value) => Some(value.to_string()),
        ScalarLiteral::Integer(value) => Some(value.to_string()),
        ScalarLiteral::String(value) => Some(format!("'{}'", value.replace('\'', "''"))),
        ScalarLiteral::Null => None,
    }
}
