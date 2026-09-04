//! Fail-closed emission of a narrow, proven relational normal-form subset.
//!
//! This is deliberately not the source-layout formatter. It accepts only
//! normal forms whose observable facts can be recreated by the supported M3
//! query syntax, and otherwise returns a typed indecision.

use std::collections::{BTreeMap, BTreeSet};

use pure_analyzer_diagnostics::ReasonCode;

use crate::relational::MAX_RELATIONAL_RECURSION_DEPTH;
use crate::{
    ColumnId, IrOrigin, NormalizationBudget, NormalizationOutcome, NormalizedQuery, ProjectionKind,
    RelationExpression, RelationFacts, RelationOperator, RelationSchema, RelationSource,
    RelationalOutcome, RowSemantics, ScalarExpression, ScalarLiteral, ScalarOperator,
    SortDirection, normalize_relational_query_with_budget,
};

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
    /// Live `relation`/`scalar` call-stack depth. See
    /// [`MAX_RELATIONAL_RECURSION_DEPTH`]: emission walks the same
    /// potentially-unbounded relational IR that normalization does, and needs
    /// the identical stack-depth budget rather than a node-count one.
    depth: usize,
}

impl Emitter {
    /// Claim one stack-depth slot; see `Normalizer::enter` for why this
    /// increments here and is matched by a plain `self.depth -= 1` in each
    /// wrapper below, rather than an RAII guard (which would hold a `&mut
    /// Emitter` borrow across the further `&mut self` calls the body makes).
    fn enter(&mut self, origin: &IrOrigin) -> EmissionResult<()> {
        if self.depth >= MAX_RELATIONAL_RECURSION_DEPTH {
            return Err(EmissionFailure::unsupported(origin));
        }
        self.depth += 1;
        Ok(())
    }

    fn relation(&mut self, expression: &RelationExpression) -> EmissionResult<EmittedRelation> {
        self.enter(expression.origin())?;
        let result = self.relation_at_depth(expression);
        self.depth -= 1;
        result
    }

    fn relation_at_depth(
        &mut self,
        expression: &RelationExpression,
    ) -> EmissionResult<EmittedRelation> {
        match expression.operator() {
            RelationOperator::Scan(source) => self.scan(expression, source),
            RelationOperator::Filter { input, predicate } => {
                self.filter(expression, input, predicate)
            }
            RelationOperator::Project {
                input,
                projections,
                kind,
            } => self.project(expression, input, projections, *kind),
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
        let RelationSource::Class(class) = source;
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
            || !expression.facts().matches(input_expression.facts())
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
        kind: ProjectionKind,
    ) -> EmissionResult<EmittedRelation> {
        let input = self.relation(input_expression)?;
        if !facts_are_unknown(expression.facts()) || !project_shape_matches(expression, projections)
        {
            return Err(EmissionFailure::unsupported(expression.origin()));
        }

        // `kind` — not a schema/name heuristic — is the sole authority on
        // which construct to emit: a `Scalar`-kind node came from `->map`/
        // `.property` and must never be re-emitted as `->project(~[...])`
        // (a `Relation<>`), and vice versa. Either arm fails closed
        // (Indecisive) rather than falling through to the other's emission
        // when its own preconditions are not met.
        if kind == ProjectionKind::Scalar {
            if !is_map_shape(projections) || input.binding == BindingKind::None {
                return Err(EmissionFailure::unsupported(expression.origin()));
            }
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
        {
            return Err(EmissionFailure::unsupported(expression.origin()));
        }
        // Exhaustive rather than a defensive `!=` comparison: `JoinKind` has a
        // single lowered variant today (see its rustdoc), so matching here
        // keeps `kind` genuinely load-bearing for the emitted text and forces
        // a compile error — not a silently-false runtime check — the moment a
        // second variant is added.
        let kind_text = match kind {
            crate::JoinKind::Inner => "JoinKind.INNER",
        };
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
                "{}->join({}, {kind_text}, {{{left_binder}, {right_binder} | {condition}}})",
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
        // `~[...]` relation-selector syntax below only exists on `Relation<>`
        // (`Row`, from `project`'s row arm, or `None`, from `join`); a
        // `Column`-bound input is a class extent (`T[*]`), which has no
        // schema for the selector to name. Confirmed against a live Legend
        // 4.113.0 engine: `Person.all()->distinct(~[Person])` fails to
        // compile there even though this crate's own class-scan schema (one
        // column named after the scanned class) makes `~[Person]` resolve.
        if input.binding == BindingKind::Column
            || !facts_are_unknown(expression.facts())
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
        // Same `~[...]`/`Relation<>` requirement as `distinct_on` above:
        // confirmed live against Legend 4.113.0, `Person.all()->sort([ascending(~Person)])`
        // is rejected with "Can't find a match for function
        // 'sort(Person[*],SortInfo[1])'" — `sort` on a class extent has a
        // different, binder-based signature (`sort(T[m], Function<...>[0..1])`).
        if input.binding == BindingKind::Column
            || expression.schema() != input_expression.schema()
            || !expression.facts().matches(input_expression.facts())
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
        self.enter(expression.origin())?;
        let result = self.scalar_at_depth(expression, references);
        self.depth -= 1;
        result
    }

    fn scalar_at_depth(
        &mut self,
        expression: &ScalarExpression,
        references: &BTreeMap<ColumnId, String>,
    ) -> EmissionResult<String> {
        // Unlike the relation-level `facts_are_unknown`/`RelationFacts::matches`
        // guards, totality is never checked here. No lowering call site proves
        // a `Knowledge<Totality>` fact today (issue #404), so it is always
        // `Unknown` and this would-be guard could never fire.
        // Nor would it need to once a producer lands: `Totality` may never be
        // inferred from model multiplicity alone (issues #51/#185), so any
        // sound producer necessarily derives it from query-structural facts
        // that re-lowering the same emitted text reproduces identically —
        // unlike a proven candidate key or row-semantics fact, which can rest
        // on non-local reasoning `facts_are_unknown`/`RelationFacts::matches`
        // must keep honest.
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

/// Structural precondition for `->map(f)` emission: exactly one output
/// column. The map-vs-project *construct* itself is decided by the caller
/// from the IR's own [`ProjectionKind`], never from this shape or a column
/// name — a user-chosen alias carries no semantic weight (issue #264).
fn is_map_shape(projections: &[crate::Projection]) -> bool {
    projections.len() == 1
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

/// White-box unit tests for `Emitter`'s private boolean guards and the
/// shape-matching/equality helpers that feed them.
///
/// `tests/canonical_emission.rs` exercises this module end to end (parse →
/// lower → normalize → emit), which is the right contract-level coverage but
/// cannot isolate a single guard term: normalization and lowering only ever
/// hand `Emitter` internally-consistent IR, so any one guard clause that is
/// implied by another (e.g. `Filter`'s schema-equality term, which
/// `RelationExpression::new` already enforces structurally) never gets
/// exercised in isolation through that path. Every fixture below therefore
/// constructs `RelationExpression`/`ScalarExpression` values directly and
/// calls `Emitter`'s private methods or the free helper functions directly,
/// so each guard term and shape/equality helper can be driven independently
/// — the same style already established by `relational.rs`'s and
/// `normalizer.rs`'s own `#[cfg(test)] mod tests`.
///
/// Each test's doc comment names the exact mutant (file:line, and the
/// operator swap) it regresses, per
/// https://github.com/tsouza/pure-analyzer/issues/442. Every one was
/// hand-verified by applying that exact source mutation, confirming the
/// named test fails, and reverting.
#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use std::collections::BTreeMap;

    use pure_analyzer_diagnostics::{FileId, TextRange, TextSize};
    use pure_analyzer_model::{
        Multiplicity, Name, PmcdDocument, QName, TypeRef, load_pmcd_documents,
    };
    use pure_analyzer_resolve::{Resolution, ResolvedClass, Resolver};
    use serde_json::json;

    use crate::{
        CandidateKey, Column, JoinKind, Knowledge, Nullability, Projection, SortKey, SourceSpan,
    };

    use super::*;

    const QUERY_FILE: u32 = 89;
    const EXACTLY_ONE: u32 = 1;

    fn origin() -> IrOrigin {
        IrOrigin::new(
            SourceSpan::new(
                FileId::new(QUERY_FILE),
                TextRange::new(TextSize::from(0), TextSize::from(1)),
            ),
            Vec::new(),
        )
    }

    fn one_multiplicity() -> Multiplicity {
        Multiplicity::new(EXACTLY_ONE, Some(EXACTLY_ONE)).expect("fixture multiplicity is valid")
    }

    fn string_type() -> TypeRef {
        TypeRef::new(
            QName::new("String").expect("fixture type is valid"),
            Vec::new(),
        )
    }

    fn boolean_type() -> TypeRef {
        TypeRef::new(
            QName::new("Boolean").expect("fixture type is valid"),
            Vec::new(),
        )
    }

    fn resolved_class(package: &str, name: &str) -> ResolvedClass {
        let document = json!({
            "_type": "data",
            "elements": [{
                "_type": "class",
                "package": package,
                "name": name,
                "stereotypes": [],
                "superTypes": [],
                "properties": [],
                "qualifiedProperties": []
            }]
        })
        .to_string();
        let graph = load_pmcd_documents(&[PmcdDocument::new("canonical-unit-fixture", &document)])
            .expect("fixture model loads");
        let path = QName::new(format!("{package}::{name}")).expect("fixture path is valid");
        match Resolver::new(&graph).resolve_class(&path) {
            Resolution::Found(class) => class,
            outcome => panic!("fixture class must resolve, got {outcome:?}"),
        }
    }

    fn column(id: u32, name: &str, type_ref: TypeRef) -> Column {
        Column::new(
            ColumnId::new(id),
            Name::new(name).expect("fixture name is valid"),
            type_ref,
            one_multiplicity(),
            Nullability::Unknown,
            origin(),
        )
    }

    /// A single-column schema shaped exactly like `class_scan_schema_matches`
    /// requires, except for a display name that never matches the scanned
    /// class's own simple name.
    fn schema_with_mismatched_name(path: &str) -> RelationSchema {
        let type_ref = TypeRef::new(QName::new(path).expect("fixture path is valid"), Vec::new());
        RelationSchema::new(vec![column(1, "NotTheScannedClass", type_ref)])
            .expect("fixture schema is valid")
    }

    /// A valid, directly emittable `Scan` over `class`, whose single-column
    /// schema exactly satisfies `class_scan_schema_matches`.
    fn class_scan(class: ResolvedClass, column_id: u32) -> RelationExpression {
        let simple_name = class
            .path()
            .as_str()
            .rsplit("::")
            .next()
            .unwrap_or_else(|| class.path().as_str());
        let type_ref = TypeRef::new(class.path().clone(), Vec::new());
        let schema = RelationSchema::new(vec![column(column_id, simple_name, type_ref)])
            .expect("fixture scan schema is valid");
        RelationExpression::new(
            RelationOperator::Scan(RelationSource::Class(class)),
            schema,
            RelationFacts::unknown(),
            origin(),
        )
        .expect("fixture scan is valid")
    }

    /// A trivially valid carrier `RelationExpression` for tests that only
    /// need a `.schema()`/`.facts()`/`.origin()` triple: `Emitter`'s guards
    /// never read a node's own `.operator()`, only the input(s) supplied
    /// alongside it, so a `Scan` stands in for any operator shape.
    fn carrier(schema: RelationSchema, facts: RelationFacts) -> RelationExpression {
        RelationExpression::new(
            RelationOperator::Scan(RelationSource::Class(resolved_class("model", "Carrier"))),
            schema,
            facts,
            origin(),
        )
        .expect("fixture carrier is valid")
    }

    fn column_scalar(column: &Column) -> ScalarExpression {
        ScalarExpression::new(
            ScalarOperator::Column(column.id()),
            column.type_ref().clone(),
            column.multiplicity(),
            column.nullability(),
            Knowledge::unknown(),
            origin(),
        )
    }

    fn boolean_literal(value: bool) -> ScalarExpression {
        ScalarExpression::new(
            ScalarOperator::Literal(ScalarLiteral::Boolean(value)),
            boolean_type(),
            one_multiplicity(),
            Nullability::NonNullable,
            Knowledge::unknown(),
            origin(),
        )
    }

    fn trivial_equal_condition(column: &Column) -> ScalarExpression {
        ScalarExpression::new(
            ScalarOperator::Equal {
                left: Box::new(column_scalar(column)),
                right: Box::new(column_scalar(column)),
            },
            boolean_type(),
            one_multiplicity(),
            Nullability::NonNullable,
            Knowledge::unknown(),
            origin(),
        )
    }

    /// A valid `Join` over two class scans, whose condition trivially
    /// compares the left scan's own column to itself.
    fn valid_join(left: RelationExpression, right: RelationExpression) -> RelationExpression {
        let condition = trivial_equal_condition(&left.schema().columns()[0]);
        let mut columns = left.schema().columns().to_vec();
        columns.extend(right.schema().columns().iter().cloned());
        let schema = RelationSchema::new(columns).expect("fixture join schema is valid");
        RelationExpression::new(
            RelationOperator::Join {
                kind: JoinKind::Inner,
                left: Box::new(left),
                right: Box::new(right),
                condition,
            },
            schema,
            RelationFacts::unknown(),
            origin(),
        )
        .expect("fixture join is valid")
    }

    /// A valid, `Row`-bound `Project(Relation)` over `input`'s single
    /// column, renamed to `out_name` under a fresh id. `Project`'s
    /// `Relation` arm is the only source of `Row` binding in `Emitter`'s
    /// output, used where a fixture needs an input that is neither
    /// `Column`- nor `None`-bound.
    fn row_bound_project(
        input: RelationExpression,
        out_id: u32,
        out_name: &str,
    ) -> RelationExpression {
        let input_column = input.schema().columns()[0].clone();
        let output = column(out_id, out_name, input_column.type_ref().clone());
        let projections = vec![Projection::new(output.id(), column_scalar(&input_column))];
        let schema = RelationSchema::new(vec![output]).expect("fixture schema is valid");
        RelationExpression::new(
            RelationOperator::Project {
                input: Box::new(input),
                projections,
                kind: ProjectionKind::Relation,
            },
            schema,
            RelationFacts::unknown(),
            origin(),
        )
        .expect("fixture row-bound project is valid")
    }

    // -- `Emitter::scan` --------------------------------------------------

    /// Regression for `Emitter::scan` (canonical.rs:253) `||` -> `&&`: a
    /// scan whose facts and class path are both fine must still be refused
    /// once its schema does not match the scanned class.
    #[test]
    fn scan_refuses_a_schema_that_does_not_match_its_scanned_class() {
        let class = resolved_class("model", "Person");
        let schema = schema_with_mismatched_name(class.path().as_str());
        let expression = RelationExpression::new(
            RelationOperator::Scan(RelationSource::Class(class)),
            schema,
            RelationFacts::unknown(),
            origin(),
        )
        .expect("fixture scan carrier is valid");
        let RelationOperator::Scan(source) = expression.operator() else {
            panic!("fixture operator is a scan");
        };

        let result = Emitter::default().scan(&expression, source);

        assert!(
            result.is_err(),
            "a schema mismatched against the scanned class must not emit"
        );
    }

    // -- `Emitter::filter` --------------------------------------------------

    /// Regression for `Emitter::filter` (canonical.rs:271) `||` -> `&&`: a
    /// filter's own facts must be refused when they diverge from its
    /// input's, even though nothing here can also make the schemas diverge
    /// (`RelationExpression::new` already enforces `Filter`'s
    /// schema-preserving invariant structurally, so that term can never be
    /// true for a validly constructed value).
    #[test]
    fn filter_refuses_facts_that_diverge_from_its_input_even_with_matching_schema() {
        let input = class_scan(resolved_class("model", "Person"), 1);
        let predicate = boolean_literal(true);
        let proven_facts = RelationFacts::new(
            Knowledge::unknown(),
            Knowledge::proven(RowSemantics::Set, origin()),
        );
        let filter_expression = RelationExpression::new(
            RelationOperator::Filter {
                input: Box::new(input.clone()),
                predicate: predicate.clone(),
            },
            input.schema().clone(),
            proven_facts,
            origin(),
        )
        .expect("fixture filter is valid");

        let result = Emitter::default().filter(&filter_expression, &input, &predicate);

        assert!(
            result.is_err(),
            "facts diverging from the input must not emit, even with a matching schema"
        );
    }

    // -- `Emitter::project` -------------------------------------------------

    /// Regression for `Emitter::project` (canonical.rs:293) `||` -> `&&`: a
    /// project node with unknown facts must still be refused when its
    /// projections do not match its own output shape.
    #[test]
    fn project_refuses_a_shape_mismatched_against_its_own_output_schema() {
        let input = class_scan(resolved_class("model", "Person"), 1);
        let schema =
            RelationSchema::new(vec![column(2, "value", string_type())]).expect("valid schema");
        let expression = carrier(schema, RelationFacts::unknown());
        let projections: Vec<Projection> = Vec::new();

        let result =
            Emitter::default().project(&expression, &input, &projections, ProjectionKind::Relation);

        assert!(
            result.is_err(),
            "an empty projection list must not match a non-empty output schema"
        );
    }

    /// Regression for `Emitter::project` (canonical.rs:305) `||` -> `&&`: a
    /// `Scalar`-kind projection with more than one output column must be
    /// refused even though its input is column-bound.
    #[test]
    fn project_refuses_a_multi_column_scalar_projection_even_with_a_column_bound_input() {
        let input = class_scan(resolved_class("model", "Person"), 1);
        let input_column = input.schema().columns()[0].clone();
        let output_a = column(2, "a", string_type());
        let output_b = column(3, "b", string_type());
        let schema = RelationSchema::new(vec![output_a.clone(), output_b.clone()])
            .expect("fixture schema is valid");
        let expression = carrier(schema, RelationFacts::unknown());
        let projections = vec![
            Projection::new(output_a.id(), column_scalar(&input_column)),
            Projection::new(output_b.id(), column_scalar(&input_column)),
        ];

        let result =
            Emitter::default().project(&expression, &input, &projections, ProjectionKind::Scalar);

        assert!(
            result.is_err(),
            "a Scalar-kind projection with more than one output column must not emit as ->map"
        );
    }

    /// Regression for `Emitter::project` (canonical.rs:319) `||` -> `&&`: a
    /// `Relation`-kind projection with duplicate output names must be
    /// refused even though its input is column-bound.
    #[test]
    fn project_refuses_duplicate_output_names_even_with_a_column_bound_input() {
        let input = class_scan(resolved_class("model", "Person"), 1);
        let input_column = input.schema().columns()[0].clone();
        let output_a = column(2, "dup", string_type());
        let output_b = column(3, "dup", string_type());
        let schema = RelationSchema::new(vec![output_a.clone(), output_b.clone()])
            .expect("fixture schema is valid");
        let expression = carrier(schema, RelationFacts::unknown());
        let projections = vec![
            Projection::new(output_a.id(), column_scalar(&input_column)),
            Projection::new(output_b.id(), column_scalar(&input_column)),
        ];

        let result =
            Emitter::default().project(&expression, &input, &projections, ProjectionKind::Relation);

        assert!(
            result.is_err(),
            "duplicate output names must not emit as ->project(~[...]), even column-bound"
        );
    }

    // -- `Emitter::join` ------------------------------------------------

    /// Regression for `Emitter::join` (canonical.rs:355) `||` -> `&&`: a
    /// join with column-bound, schema-matching inputs must still be refused
    /// once any relational fact is proven for the join's own output, since
    /// no lowering path proves inner-join facts today.
    ///
    /// This does not also exercise the mutants at 360/361 below: `&&` binds
    /// tighter than `||` in Rust, so mutating either of those only ANDs its
    /// own two immediately adjacent terms — with the leading term true (as
    /// here), the guard's outermost `||` still short-circuits to `true`
    /// regardless of that inner sub-clause.
    #[test]
    fn join_refuses_a_proven_fact_even_with_matching_column_bound_inputs() {
        let left = class_scan(resolved_class("model", "Person"), 1);
        let right = class_scan(resolved_class("model", "Manager"), 2);
        let left_column = left.schema().columns()[0].clone();
        let condition = trivial_equal_condition(&left_column);
        let mut columns = left.schema().columns().to_vec();
        columns.extend(right.schema().columns().iter().cloned());
        let schema = RelationSchema::new(columns).expect("fixture schema is valid");
        let proven_facts = RelationFacts::new(
            Knowledge::proven(vec![CandidateKey::new(vec![left_column.id()])], origin()),
            Knowledge::unknown(),
        );
        let expression = carrier(schema, proven_facts);

        let result =
            Emitter::default().join(&expression, JoinKind::Inner, &left, &right, &condition);

        assert!(
            result.is_err(),
            "a proven fact on a join's output must not emit, even with valid inputs"
        );
    }

    /// Regression for `Emitter::join` (canonical.rs:360,361) `||` -> `&&`:
    /// unlike the proven-fact fixture above, this keeps the leading two
    /// guard terms false and makes exactly the left input's binding term
    /// true, which the mutated `&&` at either position incorrectly
    /// swallows (its own two adjacent terms become the only ones checked,
    /// and the other one of the pair is false).
    #[test]
    fn join_refuses_a_row_bound_left_input_even_with_a_column_bound_right_and_matching_schema() {
        let left = row_bound_project(class_scan(resolved_class("model", "Person"), 1), 2, "value");
        let right = class_scan(resolved_class("model", "Manager"), 3);
        let left_column = left.schema().columns()[0].clone();
        let condition = trivial_equal_condition(&left_column);
        let mut columns = left.schema().columns().to_vec();
        columns.extend(right.schema().columns().iter().cloned());
        let schema = RelationSchema::new(columns).expect("fixture schema is valid");
        let expression = carrier(schema, RelationFacts::unknown());

        let result =
            Emitter::default().join(&expression, JoinKind::Inner, &left, &right, &condition);

        assert!(
            result.is_err(),
            "a row-bound (non-column) left input must not emit ->join(), even with a matching schema"
        );
    }

    // -- `Emitter::distinct` ------------------------------------------------

    /// Regression for `Emitter::distinct` (canonical.rs:403,404) `||` ->
    /// `&&`: a schema mismatch alone must refuse `->distinct()` even with a
    /// column-bound input and proven distinct-set facts. Unlike a
    /// proven-fact-only fixture, this keeps `input.binding == None` false
    /// throughout, so `&&` binding tighter than `||` cannot make either
    /// mutated position short-circuit past the schema-mismatch term.
    #[test]
    fn distinct_refuses_a_schema_mismatch_even_with_a_column_bound_input_and_distinct_set_facts() {
        let input = class_scan(resolved_class("model", "Person"), 1);
        let mismatched_schema = RelationSchema::new(vec![column(99, "different", string_type())])
            .expect("fixture schema is valid");
        let distinct_set_facts = RelationFacts::new(
            Knowledge::unknown(),
            Knowledge::proven(RowSemantics::Set, origin()),
        );
        let expression = carrier(mismatched_schema, distinct_set_facts);

        let result = Emitter::default().distinct(&expression, &input);

        assert!(
            result.is_err(),
            "a schema mismatch must refuse ->distinct(), even column-bound with distinct-set facts"
        );
    }

    // -- `Emitter::distinct_on` ----------------------------------------------

    /// Regression for `Emitter::distinct_on` (canonical.rs:430) `||` ->
    /// `&&`: selectors over a non-unique input schema must be refused even
    /// when the input is not a class extent and the outer node's own facts
    /// are unknown.
    #[test]
    fn distinct_on_refuses_non_unique_input_names_even_off_a_class_extent() {
        let person = resolved_class("model", "Person");
        let left = class_scan(person.clone(), 1);
        let left_column_id = left.schema().columns()[0].id();
        let expression = left.clone();
        let right = class_scan(person, 2);
        let join_expression = valid_join(left, right);

        let result =
            Emitter::default().distinct_on(&expression, &join_expression, &[left_column_id]);

        assert!(
            result.is_err(),
            "non-unique input names must refuse ->distinct(~[...]), even off a class extent"
        );
    }

    // -- `Emitter::sort` ------------------------------------------------

    /// Regression for `Emitter::sort` (canonical.rs:467,468) `||` -> `&&`:
    /// a join result must be refused when its own facts diverge from the
    /// input's, even off a class extent and with a matching schema.
    #[test]
    fn sort_refuses_facts_that_diverge_from_a_non_extent_input_with_matching_schema() {
        let left = class_scan(resolved_class("model", "Person"), 1);
        let right = class_scan(resolved_class("model", "Manager"), 2);
        let join_expression = valid_join(left, right);
        let left_column_id = join_expression.schema().columns()[0].id();
        let proven_facts = RelationFacts::new(
            Knowledge::unknown(),
            Knowledge::proven(RowSemantics::Set, origin()),
        );
        let expression = carrier(join_expression.schema().clone(), proven_facts);
        let keys = vec![SortKey::new(
            left_column_id,
            SortDirection::Ascending,
            origin(),
        )];

        let result = Emitter::default().sort(&expression, &join_expression, &keys);

        assert!(
            result.is_err(),
            "diverging facts must refuse ->sort(), even off a class extent with a matching schema"
        );
    }

    // -- `Emitter::relation` / `Emitter::scalar` depth bookkeeping ----------

    /// Regression for `Emitter::relation` (canonical.rs:213) `-=` -> `+=`
    /// and `-=` -> `/=`: a single non-nested `relation` call must return the
    /// emitter's recursion depth to exactly zero, not leave it incremented
    /// or unchanged.
    #[test]
    fn relation_restores_depth_to_zero_after_a_single_call() {
        let mut emitter = Emitter::default();
        let scan = class_scan(resolved_class("model", "Person"), 1);

        let result = emitter.relation(&scan);

        assert!(result.is_ok(), "fixture scan must emit");
        assert_eq!(
            emitter.depth, 0,
            "depth must return to zero once the call unwinds"
        );
    }

    /// Regression for `Emitter::scalar` (canonical.rs:505) `-=` -> `+=` and
    /// `-=` -> `/=`: same invariant as `relation` above, for the scalar
    /// recursion budget.
    #[test]
    fn scalar_restores_depth_to_zero_after_a_single_call() {
        let mut emitter = Emitter::default();
        let literal = boolean_literal(true);

        let result = emitter.scalar(&literal, &BTreeMap::new());

        assert!(result.is_ok(), "fixture literal must emit");
        assert_eq!(
            emitter.depth, 0,
            "depth must return to zero once the call unwinds"
        );
    }

    // -- shape-matching / equality helpers -----------------------------------

    /// Regression for `class_scan_schema_matches` (canonical.rs:567)
    /// `-> bool` => `true`, and its five `&&` -> `||` body mutants
    /// (canonical.rs:571-575): a name mismatch alone must refuse the match
    /// even though every other field (type, arguments, multiplicity,
    /// nullability) is otherwise exactly right. Since a leading false
    /// conjunct forces the whole `&&` chain false regardless of the rest,
    /// and any single `&&` flipped to `||` at any position downstream of
    /// that leading false term would instead let the (all-true) remainder
    /// force it back to true, this one fixture distinguishes every position
    /// in the chain.
    #[test]
    fn class_scan_schema_matches_rejects_a_name_mismatch_with_every_other_field_valid() {
        let path = "model::Person";
        let schema = schema_with_mismatched_name(path);

        assert!(
            !class_scan_schema_matches(&schema, path),
            "a name mismatch alone must refuse the match"
        );
    }

    /// Regression for `project_shape_matches` (canonical.rs:582)
    /// `-> bool` => `true`, and its `&&` -> `||` body mutant
    /// (canonical.rs:583): a projection-count mismatch alone must refuse
    /// the shape even though the (vacuous, zero-length) zip comparison
    /// alone would trivially pass.
    #[test]
    fn project_shape_matches_rejects_a_projection_count_mismatch() {
        let schema =
            RelationSchema::new(vec![column(1, "value", string_type())]).expect("valid schema");
        let expression = carrier(schema, RelationFacts::unknown());

        assert!(
            !project_shape_matches(&expression, &[]),
            "an empty projection list must not match a non-empty output schema"
        );
    }

    /// Regression for `is_map_shape` (canonical.rs:594) `-> bool` => `true`.
    #[test]
    fn is_map_shape_rejects_more_than_one_output_column() {
        let projections = vec![
            Projection::new(ColumnId::new(1), boolean_literal(true)),
            Projection::new(ColumnId::new(2), boolean_literal(true)),
        ];

        assert!(
            !is_map_shape(&projections),
            "more than one output column is not a ->map shape"
        );
    }

    /// Regression for `join_schema_matches` (canonical.rs:602) `-> bool`
    /// => `true`: an output schema that drops a right-input column must be
    /// refused.
    #[test]
    fn join_schema_matches_rejects_an_output_schema_missing_a_right_column() {
        let left_schema =
            RelationSchema::new(vec![column(1, "left", string_type())]).expect("valid schema");
        let right_schema =
            RelationSchema::new(vec![column(2, "right", string_type())]).expect("valid schema");
        let expression = carrier(left_schema.clone(), RelationFacts::unknown());

        assert!(
            !join_schema_matches(&expression, &left_schema, &right_schema),
            "an output schema missing a right-input column must not match"
        );
    }

    /// Regression for `facts_are_distinct_set` (canonical.rs:645)
    /// `-> bool` => `true`, and its `&&` -> `||` body mutant
    /// (canonical.rs:646): a proven candidate key alone must refuse the
    /// distinct-set claim even when row-semantics separately proves `Set`.
    #[test]
    fn facts_are_distinct_set_rejects_a_proven_candidate_key_even_with_proven_set_semantics() {
        let facts = RelationFacts::new(
            Knowledge::proven(vec![CandidateKey::new(vec![ColumnId::new(1)])], origin()),
            Knowledge::proven(RowSemantics::Set, origin()),
        );

        assert!(
            !facts_are_distinct_set(&facts),
            "a proven candidate key must refuse the distinct-set claim"
        );
    }

    /// Regression for `schema_names_are_unique` (canonical.rs:653)
    /// `-> bool` => `true`.
    #[test]
    fn schema_names_are_unique_rejects_a_duplicate_column_name() {
        let schema = RelationSchema::new(vec![
            column(1, "dup", string_type()),
            column(2, "dup", string_type()),
        ])
        .expect("fixture schema is valid");

        assert!(!schema_names_are_unique(&schema));
    }

    /// Regression for `references_for_binding` (canonical.rs:622) `delete
    /// !`, and (canonical.rs:626) `delete !`: unique, emittable column
    /// names must resolve row references. Deleting the negation at either
    /// site flips it into refusing this otherwise-valid input.
    #[test]
    fn references_for_binding_row_accepts_unique_emittable_names() {
        let schema = RelationSchema::new(vec![
            column(1, "alpha", string_type()),
            column(2, "beta", string_type()),
        ])
        .expect("fixture schema is valid");

        let result = references_for_binding(BindingKind::Row, &schema, "v0");

        assert!(
            result.is_ok(),
            "unique, emittable column names must resolve row references"
        );
    }

    /// Regression for `references_for_binding` (canonical.rs:623) `||` ->
    /// `&&`: duplicate column names alone must refuse row references, even
    /// though every name is individually emittable.
    #[test]
    fn references_for_binding_row_refuses_duplicate_column_names() {
        let schema = RelationSchema::new(vec![
            column(1, "dup", string_type()),
            column(2, "dup", string_type()),
        ])
        .expect("fixture schema is valid");

        let result = references_for_binding(BindingKind::Row, &schema, "v0");

        assert!(
            result.is_err(),
            "duplicate column names must refuse row references"
        );
    }

    /// Regression for `is_emittable_path` (canonical.rs:661) `-> bool` =>
    /// `true`.
    #[test]
    fn is_emittable_path_rejects_a_segment_that_is_not_a_valid_identifier() {
        assert!(!is_emittable_path("model::9Bad"));
    }

    /// Regression for `is_emittable_identifier` (canonical.rs:669) `==` ->
    /// `!=`: a leading digit must be refused. Under the mutant, a
    /// non-alphabetic, non-underscore leading character satisfies `first !=
    /// '_'` unconditionally, wrongly accepting it.
    #[test]
    fn is_emittable_identifier_rejects_a_leading_digit() {
        assert!(!is_emittable_identifier("9x"));
    }

    // -- `CanonicalEmissionOutcome` accessors --------------------------------

    /// Regression for `CanonicalEmissionOutcome::emitted` (canonical.rs:81)
    /// `-> Option<&CanonicalPure>` => `None`.
    #[test]
    fn emitted_accessor_returns_text_only_for_the_emitted_variant() {
        let emitted = CanonicalEmissionOutcome::Emitted(CanonicalPure {
            text: "x".to_owned(),
        });
        let indecisive = CanonicalEmissionOutcome::Indecisive(CanonicalEmissionIndecision::new(
            ReasonCode::IndUnmodeledOp,
            origin(),
        ));

        assert_eq!(emitted.emitted().map(CanonicalPure::as_str), Some("x"));
        assert!(indecisive.emitted().is_none());
    }

    /// Regression for `CanonicalEmissionOutcome::indecision`
    /// (canonical.rs:90) `-> Option<&CanonicalEmissionIndecision>` =>
    /// `None`.
    #[test]
    fn indecision_accessor_returns_the_refusal_only_for_the_indecisive_variant() {
        let emitted = CanonicalEmissionOutcome::Emitted(CanonicalPure {
            text: "x".to_owned(),
        });
        let indecisive = CanonicalEmissionOutcome::Indecisive(CanonicalEmissionIndecision::new(
            ReasonCode::IndUnmodeledOp,
            origin(),
        ));

        assert!(emitted.indecision().is_none());
        assert_eq!(
            indecisive
                .indecision()
                .map(CanonicalEmissionIndecision::reason),
            Some(ReasonCode::IndUnmodeledOp)
        );
    }
}
