//! Validation shared by the M4a comparison renderers.

use libpure::{ComparisonOutcome, StructuralDifferenceKind};

use crate::{
    ComparisonOriginRole, ComparisonRenderInput, RenderError,
    origin::{PreparedOrigin, prepare_origin},
};

/// Validated comparison data used by each presentation format.
pub(crate) enum PreparedComparison<'a> {
    /// A proven equal normal form.
    Equivalent,
    /// A structural schema refutation with canonical origins.
    NotEquivalent(PreparedDifference<'a>),
    /// An intentionally uncommitted result with its exact reason and origin.
    Indecisive(PreparedIndecision<'a>),
}

/// A structural schema refutation after every origin has been validated.
pub(crate) struct PreparedDifference<'a> {
    pub(crate) kind: &'a StructuralDifferenceKind,
    pub(crate) primary_origin: PreparedOrigin<'a>,
    pub(crate) secondary_origin: PreparedOrigin<'a>,
}

/// An indecisive comparison after its origin has been validated.
pub(crate) struct PreparedIndecision<'a> {
    pub(crate) reason_id: &'static str,
    pub(crate) reason_blurb: &'static str,
    pub(crate) origin: PreparedOrigin<'a>,
}

impl<'a> PreparedComparison<'a> {
    pub(crate) fn new(input: ComparisonRenderInput<'a>) -> Result<Self, RenderError> {
        match input.outcome {
            ComparisonOutcome::Equivalent => Ok(Self::Equivalent),
            ComparisonOutcome::NotEquivalent(difference) => {
                Ok(Self::NotEquivalent(PreparedDifference {
                    kind: difference.kind(),
                    primary_origin: prepare_origin(
                        input.sources,
                        ComparisonOriginRole::StructuralPrimary,
                        difference.primary_origin(),
                    )?,
                    secondary_origin: prepare_origin(
                        input.sources,
                        ComparisonOriginRole::StructuralSecondary,
                        difference.secondary_origin(),
                    )?,
                }))
            }
            ComparisonOutcome::Indecisive(indecision) => {
                let reason = indecision.reason();
                Ok(Self::Indecisive(PreparedIndecision {
                    reason_id: reason.id(),
                    reason_blurb: reason.blurb(),
                    origin: prepare_origin(
                        input.sources,
                        ComparisonOriginRole::Indecision,
                        indecision.origin(),
                    )?,
                }))
            }
        }
    }
}
