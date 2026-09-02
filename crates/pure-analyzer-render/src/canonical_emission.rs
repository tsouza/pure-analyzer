//! Validation shared by canonical-emission renderers.

use libpure::CanonicalEmissionOutcome;

use crate::{
    CanonicalEmissionOriginRole, CanonicalEmissionRenderInput, RenderError,
    origin::{PreparedOrigin, prepare_origin},
};

/// Validated canonical-emission data used by each presentation format.
pub(crate) enum PreparedCanonicalEmission<'a> {
    /// A proven normal form emitted as deterministic Pure.
    Emitted(&'a str),
    /// An intentionally uncommitted result with its exact reason and origin.
    Indecisive(PreparedCanonicalIndecision<'a>),
}

/// An indecisive canonical-emission result after its origin has been validated.
pub(crate) struct PreparedCanonicalIndecision<'a> {
    pub(crate) reason_id: &'static str,
    pub(crate) reason_blurb: &'static str,
    pub(crate) origin: PreparedOrigin<'a>,
}

impl<'a> PreparedCanonicalEmission<'a> {
    pub(crate) fn new(input: CanonicalEmissionRenderInput<'a>) -> Result<Self, RenderError> {
        match input.outcome {
            CanonicalEmissionOutcome::Emitted(emitted) => Ok(Self::Emitted(emitted.as_str())),
            CanonicalEmissionOutcome::Indecisive(indecision) => {
                let reason = indecision.reason();
                Ok(Self::Indecisive(PreparedCanonicalIndecision {
                    reason_id: reason.id(),
                    reason_blurb: reason.blurb(),
                    origin: prepare_origin(
                        input.sources,
                        CanonicalEmissionOriginRole::Indecision,
                        indecision.origin(),
                    )?,
                }))
            }
        }
    }
}
