//! Conservative, terminating normalization for the typed relational IR.
//!
//! The current public IR accepts caller-constructed facts. Its constructors
//! validate shape, but those facts are not an analyzer-issued proof boundary.
//! This module therefore applies only intrinsic rewrites: an identity
//! projection made solely of direct input-column reads and a filter with an
//! exact Boolean `true` literal. All rewrites that could alter bag, order,
//! null, partiality, or output-schema behavior remain frozen until their
//! required proof producer is private and trustworthy.

use std::collections::BTreeMap;
use std::fmt::Write;

use pure_analyzer_diagnostics::ReasonCode;
use pure_analyzer_model::{Multiplicity, Provenance, QpKind, Temporal, TypeRef};
use pure_analyzer_resolve::{DefinitionAnchor, ResolvedClass, ResolvedMember, ResolvedMemberKind};

use crate::{
    CandidateKey, Column, ColumnId, IrOrigin, JoinKind, Knowledge, ModelOrigin, ModelOriginKind,
    Nullability, Projection, RelationExpression, RelationFacts, RelationOperator, RelationSchema,
    RelationSource, RelationalQuery, RowSemantics, ScalarExpression, ScalarLiteral, ScalarOperator,
    SortDirection, SortKey, Totality,
};

/// Default upper bound on relation and scalar nodes visited by normalization.
pub const DEFAULT_NORMALIZATION_STEP_LIMIT: usize = 4_096;

/// A finite work budget for one normalization request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NormalizationBudget {
    max_steps: usize,
}

impl NormalizationBudget {
    /// Construct a finite normalization budget.
    ///
    /// A zero budget deterministically returns an IND_MISSING_REWRITE result.
    #[must_use]
    pub const fn new(max_steps: usize) -> Self {
        Self { max_steps }
    }

    /// Return the maximum number of relation and scalar nodes to visit.
    #[must_use]
    pub const fn max_steps(self) -> usize {
        self.max_steps
    }
}

impl Default for NormalizationBudget {
    fn default() -> Self {
        Self::new(DEFAULT_NORMALIZATION_STEP_LIMIT)
    }
}

/// Allocation-independent semantic normal-form identity.
///
/// This excludes source locations and model provenance so the same query can
/// compare across documents. Its companion, StructuralKey, retains the full
/// input provenance for deterministic diagnostics and auditing.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EquivalenceKey(String);

impl EquivalenceKey {
    /// Borrow the stable, length-delimited representation.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Complete deterministic key for a normal form and its input provenance.
///
/// The key is independent of ColumnId allocation and model-origin collection
/// order. It intentionally retains all original source/model origins,
/// including an identity projection that normalization removes.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StructuralKey(String);

impl StructuralKey {
    /// Borrow the stable, length-delimited representation.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A normalized query with semantic and provenance-complete identities.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedQuery {
    root: RelationExpression,
    equivalence_key: EquivalenceKey,
    structural_key: StructuralKey,
}

impl NormalizedQuery {
    /// Return the normalized relational root.
    #[must_use]
    pub const fn root(&self) -> &RelationExpression {
        &self.root
    }

    /// Return the allocation-independent semantic normal-form identity.
    #[must_use]
    pub const fn equivalence_key(&self) -> &EquivalenceKey {
        &self.equivalence_key
    }

    /// Return the provenance-complete deterministic normal-form identity.
    #[must_use]
    pub const fn structural_key(&self) -> &StructuralKey {
        &self.structural_key
    }

    /// Consume this value into its root and two deterministic identities.
    #[must_use]
    pub fn into_parts(self) -> (RelationExpression, EquivalenceKey, StructuralKey) {
        (self.root, self.equivalence_key, self.structural_key)
    }
}

/// A fail-closed bounded-normalization result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizationFailure {
    reason: ReasonCode,
    origin: IrOrigin,
}

impl NormalizationFailure {
    fn missing_rewrite(origin: IrOrigin) -> Self {
        Self {
            reason: ReasonCode::IndMissingRewrite,
            origin,
        }
    }

    /// Return the stable reason explaining why no normal form was returned.
    #[must_use]
    pub const fn reason(&self) -> ReasonCode {
        self.reason
    }

    /// Return the source/model origin at which normalization stopped.
    #[must_use]
    pub const fn origin(&self) -> &IrOrigin {
        &self.origin
    }
}

/// Outcome of one bounded normalization request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NormalizationOutcome {
    /// The input reached a conservative normal form.
    Normalized(Box<NormalizedQuery>),
    /// The finite normalization budget was exhausted or rebuilding failed.
    Indecisive(NormalizationFailure),
}

impl NormalizationOutcome {
    /// Borrow the normal form if normalization succeeded.
    #[must_use]
    pub const fn normalized(&self) -> Option<&NormalizedQuery> {
        match self {
            Self::Normalized(value) => Some(value),
            Self::Indecisive(_) => None,
        }
    }

    /// Borrow the typed failure if normalization stopped.
    #[must_use]
    pub const fn failure(&self) -> Option<&NormalizationFailure> {
        match self {
            Self::Normalized(_) => None,
            Self::Indecisive(value) => Some(value),
        }
    }
}

/// Normalize a query using DEFAULT_NORMALIZATION_STEP_LIMIT.
#[must_use]
pub fn normalize_relational_query(query: &RelationalQuery) -> NormalizationOutcome {
    normalize_relational_query_with_budget(query, NormalizationBudget::default())
}

/// Normalize a query with an explicit finite work budget.
///
/// Exhaustion returns IND_MISSING_REWRITE and never exposes a partial normal
/// form. The implementation visits each relation/scalar node once, so every
/// successful run has a strictly decreasing remaining-work measure.
#[must_use]
pub fn normalize_relational_query_with_budget(
    query: &RelationalQuery,
    budget: NormalizationBudget,
) -> NormalizationOutcome {
    let mut normalizer = Normalizer {
        remaining_steps: budget.max_steps(),
    };
    let root = match normalizer.relation(query.root()) {
        Ok(root) => root,
        Err(failure) => return NormalizationOutcome::Indecisive(failure),
    };

    let semantic = KeyEncoder::semantic().encode_relation(&root).finish();
    let provenance = KeyEncoder::full_provenance()
        .encode_relation(query.root())
        .finish();
    let mut structural = String::new();
    write_fragment(&mut structural, "normal-form-v1");
    write_fragment(&mut structural, &semantic);
    write_fragment(&mut structural, "input-provenance-v1");
    write_fragment(&mut structural, &provenance);

    NormalizationOutcome::Normalized(Box::new(NormalizedQuery {
        root,
        equivalence_key: EquivalenceKey(semantic),
        structural_key: StructuralKey(structural),
    }))
}

struct Normalizer {
    remaining_steps: usize,
}

impl Normalizer {
    fn consume(&mut self, origin: &IrOrigin) -> Result<(), NormalizationFailure> {
        let Some(remaining) = self.remaining_steps.checked_sub(1) else {
            return Err(NormalizationFailure::missing_rewrite(origin.clone()));
        };
        self.remaining_steps = remaining;
        Ok(())
    }

    fn relation(
        &mut self,
        expression: &RelationExpression,
    ) -> Result<RelationExpression, NormalizationFailure> {
        self.consume(expression.origin())?;
        let origin = expression.origin().clone();
        match expression.operator() {
            RelationOperator::Scan(_) => Ok(expression.clone()),
            RelationOperator::Filter { input, predicate } => {
                let input = self.relation(input)?;
                let predicate = self.scalar(predicate)?;
                if is_literal_true_filter(
                    &input,
                    expression.schema(),
                    expression.facts(),
                    &predicate,
                ) {
                    return Ok(input);
                }
                Self::rebuild(
                    RelationOperator::Filter {
                        input: Box::new(input),
                        predicate,
                    },
                    expression.schema().clone(),
                    expression.facts().clone(),
                    origin,
                )
            }
            RelationOperator::Project { input, projections } => {
                let input = self.relation(input)?;
                let projections = projections
                    .iter()
                    .map(|projection| self.projection(projection))
                    .collect::<Result<Vec<_>, _>>()?;
                if is_identity_project(
                    &input,
                    expression.schema(),
                    expression.facts(),
                    &projections,
                ) {
                    return Ok(input);
                }
                Self::rebuild(
                    RelationOperator::Project {
                        input: Box::new(input),
                        projections,
                    },
                    expression.schema().clone(),
                    expression.facts().clone(),
                    origin,
                )
            }
            RelationOperator::Join {
                kind,
                left,
                right,
                condition,
            } => Self::rebuild(
                RelationOperator::Join {
                    kind: *kind,
                    left: Box::new(self.relation(left)?),
                    right: Box::new(self.relation(right)?),
                    condition: self.scalar(condition)?,
                },
                expression.schema().clone(),
                expression.facts().clone(),
                origin,
            ),
            RelationOperator::Distinct { input } => Self::rebuild(
                RelationOperator::Distinct {
                    input: Box::new(self.relation(input)?),
                },
                expression.schema().clone(),
                expression.facts().clone(),
                origin,
            ),
            RelationOperator::DistinctOn { input, columns } => Self::rebuild(
                RelationOperator::DistinctOn {
                    input: Box::new(self.relation(input)?),
                    columns: columns.clone(),
                },
                expression.schema().clone(),
                expression.facts().clone(),
                origin,
            ),
            RelationOperator::Sort { input, keys } => Self::rebuild(
                RelationOperator::Sort {
                    input: Box::new(self.relation(input)?),
                    keys: keys.clone(),
                },
                expression.schema().clone(),
                expression.facts().clone(),
                origin,
            ),
        }
    }

    fn projection(&mut self, projection: &Projection) -> Result<Projection, NormalizationFailure> {
        Ok(Projection::new(
            projection.column(),
            self.scalar(projection.expression())?,
        ))
    }

    fn scalar(
        &mut self,
        expression: &ScalarExpression,
    ) -> Result<ScalarExpression, NormalizationFailure> {
        self.consume(expression.origin())?;
        match expression.operator() {
            ScalarOperator::Column(_) | ScalarOperator::Literal(_) => {}
            ScalarOperator::Navigation { input, .. } | ScalarOperator::Not { input } => {
                self.scalar(input)?;
            }
            ScalarOperator::Equal { left, right }
            | ScalarOperator::And { left, right }
            | ScalarOperator::Or { left, right } => {
                self.scalar(left)?;
                self.scalar(right)?;
            }
        }
        Ok(expression.clone())
    }

    fn rebuild(
        operator: RelationOperator,
        schema: RelationSchema,
        facts: RelationFacts,
        origin: IrOrigin,
    ) -> Result<RelationExpression, NormalizationFailure> {
        RelationExpression::new(operator, schema, facts, origin.clone())
            .map_err(|_| NormalizationFailure::missing_rewrite(origin))
    }
}

fn is_identity_project(
    input: &RelationExpression,
    schema: &RelationSchema,
    facts: &RelationFacts,
    projections: &[Projection],
) -> bool {
    schema == input.schema()
        && facts == input.facts()
        && projections.len() == schema.columns().len()
        && projections
            .iter()
            .zip(schema.columns())
            .all(|(projection, column)| is_identity_read(projection, column))
}

fn is_identity_read(projection: &Projection, column: &Column) -> bool {
    let expression = projection.expression();
    projection.column() == column.id()
        && expression.type_ref() == column.type_ref()
        && expression.multiplicity() == column.multiplicity()
        && expression.nullability() == column.nullability()
        && expression.totality().is_unknown()
        && matches!(
            expression.operator(),
            ScalarOperator::Column(id) if *id == column.id()
        )
}

fn is_literal_true_filter(
    input: &RelationExpression,
    schema: &RelationSchema,
    facts: &RelationFacts,
    predicate: &ScalarExpression,
) -> bool {
    schema == input.schema()
        && facts == input.facts()
        && predicate.totality().is_unknown()
        && matches!(
            predicate.operator(),
            ScalarOperator::Literal(ScalarLiteral::Boolean(true))
        )
}

struct KeyEncoder {
    include_provenance: bool,
    output: String,
}

impl KeyEncoder {
    fn semantic() -> Self {
        Self {
            include_provenance: false,
            output: String::new(),
        }
    }

    fn full_provenance() -> Self {
        Self {
            include_provenance: true,
            output: String::new(),
        }
    }

    fn encode_relation(mut self, expression: &RelationExpression) -> Self {
        self.relation(expression);
        self
    }

    fn finish(self) -> String {
        self.output
    }

    fn relation(&mut self, expression: &RelationExpression) {
        write_fragment(&mut self.output, "relation");
        let output = ColumnScope::from_schema(expression.schema());
        self.schema(expression.schema(), &output);
        self.facts(expression.facts(), &output);
        if self.include_provenance {
            self.origin(expression.origin());
        }
        match expression.operator() {
            RelationOperator::Scan(source) => {
                write_fragment(&mut self.output, "scan");
                self.source(source);
            }
            RelationOperator::Filter { input, predicate } => {
                write_fragment(&mut self.output, "filter");
                self.relation(input);
                self.scalar(predicate, &ColumnScope::from_schema(input.schema()));
            }
            RelationOperator::Project { input, projections } => {
                write_fragment(&mut self.output, "project");
                self.relation(input);
                let input_scope = ColumnScope::from_schema(input.schema());
                write_usize(&mut self.output, projections.len());
                for projection in projections {
                    self.column_id(&output, projection.column());
                    self.scalar(projection.expression(), &input_scope);
                }
            }
            RelationOperator::Join {
                kind,
                left,
                right,
                condition,
            } => {
                write_fragment(&mut self.output, "join");
                write_fragment(
                    &mut self.output,
                    match kind {
                        JoinKind::Inner => "inner",
                    },
                );
                self.relation(left);
                self.relation(right);
                self.scalar(
                    condition,
                    &ColumnScope::from_join(left.schema(), right.schema()),
                );
            }
            RelationOperator::Distinct { input } => {
                write_fragment(&mut self.output, "distinct");
                self.relation(input);
            }
            RelationOperator::DistinctOn { input, columns } => {
                write_fragment(&mut self.output, "distinct-on");
                self.relation(input);
                let input_scope = ColumnScope::from_schema(input.schema());
                write_usize(&mut self.output, columns.len());
                for column in columns {
                    self.column_id(&input_scope, *column);
                }
            }
            RelationOperator::Sort { input, keys } => {
                write_fragment(&mut self.output, "sort");
                self.relation(input);
                let input_scope = ColumnScope::from_schema(input.schema());
                write_usize(&mut self.output, keys.len());
                for key in keys {
                    self.sort_key(key, &input_scope);
                }
            }
        }
    }

    fn schema(&mut self, schema: &RelationSchema, scope: &ColumnScope) {
        write_fragment(&mut self.output, "schema");
        write_usize(&mut self.output, schema.columns().len());
        for column in schema.columns() {
            self.column(column, scope);
        }
    }

    fn column(&mut self, column: &Column, scope: &ColumnScope) {
        write_fragment(&mut self.output, "column");
        self.column_id(scope, column.id());
        write_fragment(&mut self.output, column.name().as_str());
        self.type_ref(column.type_ref());
        self.multiplicity(column.multiplicity());
        self.nullability(column.nullability());
        if self.include_provenance {
            self.origin(column.origin());
        }
    }

    fn facts(&mut self, facts: &RelationFacts, scope: &ColumnScope) {
        write_fragment(&mut self.output, "facts");
        self.keys(facts.candidate_keys(), scope);
        self.row_semantics(facts.row_semantics());
    }

    fn keys(&mut self, knowledge: &Knowledge<Vec<CandidateKey>>, scope: &ColumnScope) {
        write_fragment(&mut self.output, "keys");
        match knowledge {
            Knowledge::Unknown => write_fragment(&mut self.output, "unknown"),
            Knowledge::Proven { value, origin } => {
                write_fragment(&mut self.output, "proven");
                let mut keys = value
                    .iter()
                    .map(|key| {
                        let mut columns = key
                            .columns()
                            .iter()
                            .filter_map(|column| scope.position(*column))
                            .collect::<Vec<_>>();
                        columns.sort_unstable();
                        columns
                    })
                    .collect::<Vec<_>>();
                keys.sort_unstable();
                keys.dedup();
                write_usize(&mut self.output, keys.len());
                for key in keys {
                    write_usize(&mut self.output, key.len());
                    for column in key {
                        write_usize(&mut self.output, column);
                    }
                }
                if self.include_provenance {
                    self.origin(origin);
                }
            }
        }
    }

    fn row_semantics(&mut self, knowledge: &Knowledge<RowSemantics>) {
        write_fragment(&mut self.output, "row-semantics");
        match knowledge {
            Knowledge::Unknown => write_fragment(&mut self.output, "unknown"),
            Knowledge::Proven { value, origin } => {
                write_fragment(&mut self.output, "proven");
                write_fragment(
                    &mut self.output,
                    match value {
                        RowSemantics::Set => "set",
                        RowSemantics::Bag => "bag",
                    },
                );
                if self.include_provenance {
                    self.origin(origin);
                }
            }
        }
    }

    fn source(&mut self, source: &RelationSource) {
        match source {
            RelationSource::Class(class) => {
                write_fragment(&mut self.output, "class");
                self.class(class);
            }
            RelationSource::Member(member) => {
                write_fragment(&mut self.output, "member");
                self.member(member);
            }
        }
    }

    fn sort_key(&mut self, key: &SortKey, scope: &ColumnScope) {
        write_fragment(&mut self.output, "sort-key");
        self.column_id(scope, key.column());
        write_fragment(
            &mut self.output,
            match key.direction() {
                SortDirection::Ascending => "ascending",
                SortDirection::Descending => "descending",
            },
        );
        if self.include_provenance {
            self.origin(key.origin());
        }
    }

    fn scalar(&mut self, expression: &ScalarExpression, scope: &ColumnScope) {
        write_fragment(&mut self.output, "scalar");
        self.type_ref(expression.type_ref());
        self.multiplicity(expression.multiplicity());
        self.nullability(expression.nullability());
        self.totality(expression.totality());
        if self.include_provenance {
            self.origin(expression.origin());
        }
        match expression.operator() {
            ScalarOperator::Column(column) => {
                write_fragment(&mut self.output, "column");
                self.column_id(scope, *column);
            }
            ScalarOperator::Literal(literal) => self.literal(literal),
            ScalarOperator::Navigation { input, navigation } => {
                write_fragment(&mut self.output, "navigation");
                self.scalar(input, scope);
                self.member(navigation.member());
            }
            ScalarOperator::Equal { left, right } => {
                write_fragment(&mut self.output, "equal");
                self.scalar(left, scope);
                self.scalar(right, scope);
            }
            ScalarOperator::And { left, right } => {
                write_fragment(&mut self.output, "and");
                self.scalar(left, scope);
                self.scalar(right, scope);
            }
            ScalarOperator::Or { left, right } => {
                write_fragment(&mut self.output, "or");
                self.scalar(left, scope);
                self.scalar(right, scope);
            }
            ScalarOperator::Not { input } => {
                write_fragment(&mut self.output, "not");
                self.scalar(input, scope);
            }
        }
    }

    fn literal(&mut self, literal: &ScalarLiteral) {
        match literal {
            ScalarLiteral::Boolean(value) => {
                write_fragment(&mut self.output, "boolean");
                write_fragment(&mut self.output, if *value { "true" } else { "false" });
            }
            ScalarLiteral::Integer(value) => {
                write_fragment(&mut self.output, "integer");
                write_fragment(&mut self.output, &value.to_string());
            }
            ScalarLiteral::String(value) => {
                write_fragment(&mut self.output, "string");
                write_fragment(&mut self.output, value);
            }
            ScalarLiteral::Null => write_fragment(&mut self.output, "null"),
        }
    }

    fn totality(&mut self, knowledge: &Knowledge<Totality>) {
        write_fragment(&mut self.output, "totality");
        match knowledge {
            Knowledge::Unknown => write_fragment(&mut self.output, "unknown"),
            Knowledge::Proven { value, origin } => {
                write_fragment(&mut self.output, "proven");
                write_fragment(
                    &mut self.output,
                    match value {
                        Totality::Total => "total",
                        Totality::Partial => "partial",
                    },
                );
                if self.include_provenance {
                    self.origin(origin);
                }
            }
        }
    }

    fn type_ref(&mut self, type_ref: &TypeRef) {
        write_fragment(&mut self.output, "type");
        write_fragment(&mut self.output, type_ref.raw_type().as_str());
        write_usize(&mut self.output, type_ref.type_arguments().len());
        for argument in type_ref.type_arguments() {
            self.type_ref(argument);
        }
    }

    fn multiplicity(&mut self, multiplicity: Multiplicity) {
        write_fragment(&mut self.output, "multiplicity");
        write_fragment(&mut self.output, &multiplicity.lower().to_string());
        match multiplicity.upper() {
            Some(upper) => write_fragment(&mut self.output, &upper.to_string()),
            None => write_fragment(&mut self.output, "unbounded"),
        }
    }

    fn nullability(&mut self, nullability: Nullability) {
        write_fragment(
            &mut self.output,
            match nullability {
                Nullability::NonNullable => "non-null",
                Nullability::Nullable => "nullable",
                Nullability::Unknown => "unknown",
            },
        );
    }

    fn origin(&mut self, origin: &IrOrigin) {
        write_fragment(&mut self.output, "origin");
        let source = origin.source();
        write_fragment(&mut self.output, &source.file().index().to_string());
        self.range(source.range());
        let mut model_origins = origin
            .model_origins()
            .iter()
            .map(model_origin_key)
            .collect::<Vec<_>>();
        model_origins.sort_unstable();
        model_origins.dedup();
        write_usize(&mut self.output, model_origins.len());
        for model_origin in model_origins {
            write_fragment(&mut self.output, &model_origin);
        }
    }

    fn class(&mut self, class: &ResolvedClass) {
        write_fragment(&mut self.output, "resolved-class");
        write_fragment(&mut self.output, class.path().as_str());
        write_fragment(
            &mut self.output,
            match class.temporal() {
                None => "non-temporal",
                Some(Temporal::Bitemporal) => "bitemporal",
                Some(Temporal::BusinessTemporal) => "business-temporal",
                Some(Temporal::ProcessingTemporal) => "processing-temporal",
            },
        );
        if self.include_provenance {
            self.provenance(class.provenance());
            self.anchor(class.definition());
        }
    }

    fn member(&mut self, member: &ResolvedMember) {
        write_fragment(&mut self.output, "resolved-member");
        self.class(member.owner());
        write_fragment(&mut self.output, member.name().as_str());
        self.type_ref(member.target());
        self.multiplicity(member.multiplicity());
        self.member_kind(member.kind());
        match member.signature() {
            Some(signature) => {
                write_fragment(&mut self.output, "signature");
                write_usize(&mut self.output, signature.len());
                for argument in signature {
                    self.type_ref(argument);
                }
            }
            None => write_fragment(&mut self.output, "no-signature"),
        }
        match member.target_temporal_arity() {
            Some(arity) => write_fragment(&mut self.output, &arity.to_string()),
            None => write_fragment(&mut self.output, "no-target-temporal-arity"),
        }
        if self.include_provenance {
            self.provenance(member.provenance());
            self.anchor(member.definition());
        }
    }

    fn member_kind(&mut self, kind: &ResolvedMemberKind) {
        match kind {
            ResolvedMemberKind::Qualified(kind) => {
                write_fragment(&mut self.output, "qualified");
                write_fragment(
                    &mut self.output,
                    match kind {
                        QpKind::UserQualified => "user",
                        QpKind::MilestonedPoint => "milestoned-point",
                        QpKind::AllVersions => "all-versions",
                        QpKind::AllVersionsInRange => "all-versions-in-range",
                        QpKind::EdgePoint => "edge-point",
                    },
                );
            }
            ResolvedMemberKind::Property => write_fragment(&mut self.output, "property"),
            ResolvedMemberKind::AssociationEnd { association } => {
                write_fragment(&mut self.output, "association-end");
                write_fragment(&mut self.output, association.as_str());
            }
        }
    }

    fn provenance(&mut self, provenance: Provenance) {
        write_fragment(
            &mut self.output,
            match provenance {
                Provenance::Pmcd => "pmcd",
                Provenance::PureFile => "pure-file",
            },
        );
    }

    fn anchor(&mut self, anchor: DefinitionAnchor) {
        write_fragment(&mut self.output, "anchor");
        write_fragment(&mut self.output, &anchor.source().index().to_string());
        match anchor.span() {
            Some(span) => self.range(span),
            None => write_fragment(&mut self.output, "no-span"),
        }
    }

    fn range(&mut self, range: pure_analyzer_diagnostics::TextRange) {
        write_fragment(&mut self.output, &u32::from(range.start()).to_string());
        write_fragment(&mut self.output, &u32::from(range.end()).to_string());
    }

    fn column_id(&mut self, scope: &ColumnScope, column: ColumnId) {
        write_fragment(&mut self.output, "column-id");
        match scope.position(column) {
            Some(position) => write_usize(&mut self.output, position),
            None => write_fragment(&mut self.output, "missing"),
        }
    }
}

/// Canonical column identities for one explicit relation-schema scope.
///
/// Raw ColumnId values can be reused by a caller in a nested projection. The
/// normal-form key must alpha-normalize each input/output schema independently
/// rather than leaking an outer raw allocation into that inner scope.
struct ColumnScope {
    positions: BTreeMap<ColumnId, usize>,
}

impl ColumnScope {
    fn from_schema(schema: &RelationSchema) -> Self {
        Self::from_columns(schema.columns().iter())
    }

    fn from_join(left: &RelationSchema, right: &RelationSchema) -> Self {
        Self::from_columns(left.columns().iter().chain(right.columns()))
    }

    fn from_columns<'column>(columns: impl Iterator<Item = &'column Column>) -> Self {
        Self {
            positions: columns
                .enumerate()
                .map(|(position, column)| (column.id(), position))
                .collect(),
        }
    }

    fn position(&self, column: ColumnId) -> Option<usize> {
        self.positions.get(&column).copied()
    }
}

fn model_origin_key(origin: &ModelOrigin) -> String {
    let mut output = String::new();
    write_fragment(
        &mut output,
        match origin.kind() {
            ModelOriginKind::Class => "class",
            ModelOriginKind::Member => "member",
            ModelOriginKind::Unspecified => "unspecified",
        },
    );
    write_fragment(
        &mut output,
        match origin.provenance() {
            Provenance::Pmcd => "pmcd",
            Provenance::PureFile => "pure-file",
        },
    );
    let anchor = origin.definition();
    write_fragment(&mut output, &anchor.source().index().to_string());
    match anchor.span() {
        Some(span) => {
            write_fragment(&mut output, &u32::from(span.start()).to_string());
            write_fragment(&mut output, &u32::from(span.end()).to_string());
        }
        None => write_fragment(&mut output, "no-span"),
    }
    write_fragment(&mut output, &origin.structural_identity_key());
    output
}

fn write_fragment(output: &mut String, value: &str) {
    let _ = write!(output, "{}:", value.len());
    output.push_str(value);
}

fn write_usize(output: &mut String, value: usize) {
    write_fragment(output, &value.to_string());
}
