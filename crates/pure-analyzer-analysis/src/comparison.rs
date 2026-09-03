//! Fail-closed structural comparison of already-lowered relational queries.

use std::cmp::Ordering;

use pure_analyzer_diagnostics::ReasonCode;
use pure_analyzer_model::Provenance;

use crate::{
    Column, IrOrigin, ModelOrigin, ModelOriginKind, NormalizationBudget, NormalizationFailure,
    NormalizationOutcome, NormalizedQuery, Nullability, OpaqueOutcome, RelationalOutcome,
    RelationalQuery, normalize_relational_query_with_budget,
};

/// A sound, deliberately incomplete comparison of two relational queries.
///
/// The comparison commits to equality only for matching proven normal forms.
/// It commits to non-equivalence only for an incompatible ordered output
/// schema. Every other difference stays explicit as [`Self::Indecisive`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ComparisonOutcome {
    /// Both queries reached the same proven normal-form identity.
    Equivalent,
    /// The queries have a deterministic incompatible ordered output schema.
    NotEquivalent(StructuralDifference),
    /// No sound commitment can be made for the returned reason.
    Indecisive(ComparisonIndecision),
}

impl ComparisonOutcome {
    /// Borrow the proven structural difference, if this comparison refuted equality.
    #[must_use]
    pub const fn difference(&self) -> Option<&StructuralDifference> {
        match self {
            Self::NotEquivalent(difference) => Some(difference),
            Self::Equivalent | Self::Indecisive(_) => None,
        }
    }

    /// Borrow the typed reason that prevented a sound commitment, if any.
    #[must_use]
    pub const fn indecision(&self) -> Option<&ComparisonIndecision> {
        match self {
            Self::Indecisive(indecision) => Some(indecision),
            Self::Equivalent | Self::NotEquivalent(_) => None,
        }
    }
}

/// One typed, source-anchored reason a comparison remained inconclusive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComparisonIndecision {
    reason: ReasonCode,
    origin: IrOrigin,
}

impl ComparisonIndecision {
    fn new(reason: ReasonCode, origin: IrOrigin) -> Self {
        Self { reason, origin }
    }

    /// Return the exact registered reason for the inconclusive comparison.
    #[must_use]
    pub const fn reason(&self) -> ReasonCode {
        self.reason
    }

    /// Return the deterministically selected source/model origin for the limit.
    #[must_use]
    pub const fn origin(&self) -> &IrOrigin {
        &self.origin
    }
}

/// A deterministic proof that two ordered output schemas cannot be equivalent.
///
/// The two origins are in canonical normal-form-key order, not caller order,
/// so reversing comparison inputs produces the same structural proof.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructuralDifference {
    kind: StructuralDifferenceKind,
    primary_origin: IrOrigin,
    secondary_origin: IrOrigin,
}

impl StructuralDifference {
    fn new(
        kind: StructuralDifferenceKind,
        primary_origin: IrOrigin,
        secondary_origin: IrOrigin,
    ) -> Self {
        Self {
            kind,
            primary_origin,
            secondary_origin,
        }
    }

    /// Return the exact ordered-schema incompatibility that proves distinction.
    #[must_use]
    pub const fn kind(&self) -> &StructuralDifferenceKind {
        &self.kind
    }

    /// Return the first deterministic source/model origin involved in the proof.
    #[must_use]
    pub const fn primary_origin(&self) -> &IrOrigin {
        &self.primary_origin
    }

    /// Return the second deterministic source/model origin involved in the proof.
    #[must_use]
    pub const fn secondary_origin(&self) -> &IrOrigin {
        &self.secondary_origin
    }
}

/// The closed set of schema incompatibilities this M4a slice can prove.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StructuralDifferenceKind {
    /// The output schemas contain different numbers of ordered columns.
    OutputColumnCount {
        /// The number of columns associated with the primary origin.
        primary_count: usize,
        /// The number of columns associated with the secondary origin.
        secondary_count: usize,
    },
    /// The first differing ordered output column has incompatible metadata.
    OutputColumn {
        /// Zero-based position in the ordered output schema.
        index: usize,
        /// The first incompatible field at this output position.
        field: OutputSchemaField,
    },
}

/// An output-column field whose mismatch proves schema incompatibility.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputSchemaField {
    /// The output column aliases differ.
    Name,
    /// The declared Pure types differ.
    Type,
    /// The declared value multiplicities differ.
    Multiplicity,
    /// The explicit nullability facts differ.
    Nullability,
}

/// Compare two already-lowered relational queries using the default finite budget.
///
/// This library boundary does not parse, lower, execute, or search for a
/// witness. It only compares the existing typed relational IR.
#[must_use]
pub fn compare_relational_queries(
    left: &RelationalQuery,
    right: &RelationalQuery,
) -> ComparisonOutcome {
    compare_relational_queries_with_budget(left, right, NormalizationBudget::default())
}

/// Compare two already-lowered relational queries with a finite per-query budget.
///
/// Each input is normalized independently. Budget exhaustion or any
/// unrecognized normal-form difference returns a typed indecision rather than
/// exposing a partial normal form or guessing at equivalence.
#[must_use]
pub fn compare_relational_queries_with_budget(
    left: &RelationalQuery,
    right: &RelationalQuery,
    budget: NormalizationBudget,
) -> ComparisonOutcome {
    let left = normalize_relational_query_with_budget(left, budget);
    let right = normalize_relational_query_with_budget(right, budget);

    match (left, right) {
        (NormalizationOutcome::Normalized(left), NormalizationOutcome::Normalized(right)) => {
            compare_normalized(&left, &right)
        }
        (NormalizationOutcome::Indecisive(left), NormalizationOutcome::Indecisive(right)) => {
            ComparisonOutcome::Indecisive(canonical_failure_indecision(&left, &right))
        }
        (NormalizationOutcome::Indecisive(failure), NormalizationOutcome::Normalized(_))
        | (NormalizationOutcome::Normalized(_), NormalizationOutcome::Indecisive(failure)) => {
            ComparisonOutcome::Indecisive(indecision_from_failure(&failure))
        }
    }
}

/// Compare two lowering outcomes using the default finite normalization budget.
///
/// A lowering outcome that is opaque, unresolved, or malformed is an explicit
/// semantic limit rather than an error to be guessed through. This helper
/// preserves that fail-closed boundary before comparing two supported
/// relational queries.
#[must_use]
pub fn compare_lowered_queries(
    left: &RelationalOutcome,
    right: &RelationalOutcome,
) -> ComparisonOutcome {
    compare_lowered_queries_with_budget(left, right, NormalizationBudget::default())
}

/// Compare two lowering outcomes with a finite per-query normalization budget.
///
/// The only committed results come from two supported queries. If either
/// lowered input is opaque, the returned outcome is indecisive with the
/// lowering reason and a deterministic selected origin.
#[must_use]
pub fn compare_lowered_queries_with_budget(
    left: &RelationalOutcome,
    right: &RelationalOutcome,
    budget: NormalizationBudget,
) -> ComparisonOutcome {
    match (left, right) {
        (RelationalOutcome::Supported(left), RelationalOutcome::Supported(right)) => {
            compare_relational_queries_with_budget(left, right, budget)
        }
        (RelationalOutcome::Opaque(left), RelationalOutcome::Opaque(right)) => {
            ComparisonOutcome::Indecisive(canonical_opaque_indecision(left, right))
        }
        (RelationalOutcome::Opaque(opaque), RelationalOutcome::Supported(_))
        | (RelationalOutcome::Supported(_), RelationalOutcome::Opaque(opaque)) => {
            ComparisonOutcome::Indecisive(indecision_from_opaque(opaque))
        }
    }
}

fn compare_normalized(left: &NormalizedQuery, right: &NormalizedQuery) -> ComparisonOutcome {
    if left.equivalence_key() == right.equivalence_key() {
        return ComparisonOutcome::Equivalent;
    }

    if let Some(difference) = output_schema_difference(left, right) {
        return ComparisonOutcome::NotEquivalent(difference);
    }

    ComparisonOutcome::Indecisive(ComparisonIndecision::new(
        ReasonCode::IndMissingRewrite,
        canonical_normalized_origin(left, right),
    ))
}

fn output_schema_difference(
    left: &NormalizedQuery,
    right: &NormalizedQuery,
) -> Option<StructuralDifference> {
    let left_columns = left.root().schema().columns();
    let right_columns = right.root().schema().columns();

    if left_columns.len() != right_columns.len() {
        return Some(output_column_count_difference(
            left,
            right,
            left_columns.len(),
            right_columns.len(),
        ));
    }

    left_columns.iter().zip(right_columns).enumerate().find_map(
        |(index, (left_column, right_column))| {
            output_column_field_difference(left_column, right_column).map(|field| {
                let (primary_origin, secondary_origin) = canonical_normalized_origins(
                    left,
                    right,
                    left_column.origin(),
                    right_column.origin(),
                );
                StructuralDifference::new(
                    StructuralDifferenceKind::OutputColumn { index, field },
                    primary_origin,
                    secondary_origin,
                )
            })
        },
    )
}

fn output_column_count_difference(
    left: &NormalizedQuery,
    right: &NormalizedQuery,
    left_count: usize,
    right_count: usize,
) -> StructuralDifference {
    let (primary_origin, secondary_origin, primary_count, secondary_count) =
        if left.structural_key() <= right.structural_key() {
            (
                left.root().origin().clone(),
                right.root().origin().clone(),
                left_count,
                right_count,
            )
        } else {
            (
                right.root().origin().clone(),
                left.root().origin().clone(),
                right_count,
                left_count,
            )
        };
    StructuralDifference::new(
        StructuralDifferenceKind::OutputColumnCount {
            primary_count,
            secondary_count,
        },
        primary_origin,
        secondary_origin,
    )
}

fn output_column_field_difference(left: &Column, right: &Column) -> Option<OutputSchemaField> {
    if left.name() != right.name() {
        Some(OutputSchemaField::Name)
    } else if left.type_ref() != right.type_ref() {
        Some(OutputSchemaField::Type)
    } else if left.multiplicity() != right.multiplicity() {
        Some(OutputSchemaField::Multiplicity)
    } else if nullability_contradicts(left.nullability(), right.nullability()) {
        Some(OutputSchemaField::Nullability)
    } else {
        None
    }
}

/// Whether two nullability facts are proven incompatible.
///
/// [`Nullability::Unknown`] means the available facts establish nothing, not
/// that a side differs from the other. Only the genuinely contradictory pair
/// (`NonNullable` vs `Nullable`) refutes equivalence; any comparison
/// involving `Unknown` stays inconclusive.
const fn nullability_contradicts(left: Nullability, right: Nullability) -> bool {
    matches!(
        (left, right),
        (Nullability::NonNullable, Nullability::Nullable)
            | (Nullability::Nullable, Nullability::NonNullable)
    )
}

fn canonical_normalized_origins(
    left: &NormalizedQuery,
    right: &NormalizedQuery,
    left_origin: &IrOrigin,
    right_origin: &IrOrigin,
) -> (IrOrigin, IrOrigin) {
    if left.structural_key() <= right.structural_key() {
        (left_origin.clone(), right_origin.clone())
    } else {
        (right_origin.clone(), left_origin.clone())
    }
}

fn canonical_normalized_origin(left: &NormalizedQuery, right: &NormalizedQuery) -> IrOrigin {
    if left.structural_key() <= right.structural_key() {
        left.root().origin().clone()
    } else {
        right.root().origin().clone()
    }
}

fn indecision_from_failure(failure: &NormalizationFailure) -> ComparisonIndecision {
    ComparisonIndecision::new(failure.reason(), failure.origin().clone())
}

fn indecision_from_opaque(opaque: &OpaqueOutcome) -> ComparisonIndecision {
    ComparisonIndecision::new(opaque.reason(), opaque.origin().clone())
}

fn canonical_failure_indecision(
    left: &NormalizationFailure,
    right: &NormalizationFailure,
) -> ComparisonIndecision {
    let order = left
        .reason()
        .id()
        .cmp(right.reason().id())
        .then_with(|| compare_origins(left.origin(), right.origin()));
    if order.is_le() {
        indecision_from_failure(left)
    } else {
        indecision_from_failure(right)
    }
}

fn canonical_opaque_indecision(
    left: &OpaqueOutcome,
    right: &OpaqueOutcome,
) -> ComparisonIndecision {
    let order = left
        .reason()
        .id()
        .cmp(right.reason().id())
        .then_with(|| compare_origins(left.origin(), right.origin()));
    if order.is_le() {
        indecision_from_opaque(left)
    } else {
        indecision_from_opaque(right)
    }
}

fn compare_origins(left: &IrOrigin, right: &IrOrigin) -> Ordering {
    let left_source = left.source();
    let right_source = right.source();
    left_source
        .file()
        .cmp(&right_source.file())
        .then_with(|| {
            u32::from(left_source.range().start()).cmp(&u32::from(right_source.range().start()))
        })
        .then_with(|| {
            u32::from(left_source.range().end()).cmp(&u32::from(right_source.range().end()))
        })
        .then_with(|| {
            model_origin_keys(left.model_origins()).cmp(&model_origin_keys(right.model_origins()))
        })
}

fn model_origin_keys(origins: &[ModelOrigin]) -> Vec<ModelOriginKey> {
    let mut keys = origins
        .iter()
        .map(|origin| {
            let definition = origin.definition();
            let span = definition
                .span()
                .map(|span| (u32::from(span.start()), u32::from(span.end())));
            ModelOriginKey {
                kind: model_origin_kind_name(origin.kind()),
                provenance: origin.provenance(),
                source: definition.source().index(),
                span,
                identity: origin.structural_identity_key(),
            }
        })
        .collect::<Vec<_>>();
    keys.sort_unstable();
    keys.dedup();
    keys
}

#[derive(PartialEq, Eq, PartialOrd, Ord)]
struct ModelOriginKey {
    kind: &'static str,
    provenance: Provenance,
    source: u32,
    span: Option<(u32, u32)>,
    identity: String,
}

const fn model_origin_kind_name(kind: ModelOriginKind) -> &'static str {
    match kind {
        ModelOriginKind::Class => "class",
        ModelOriginKind::Member => "member",
        ModelOriginKind::Unspecified => "unspecified",
    }
}
