//! Validation shared by the M4a comparison renderers.

use libpure::{
    ComparisonOutcome, FileId, IrOrigin, LineColumn, SourceFile, SourceStore,
    StructuralDifferenceKind, TextRange,
};

use crate::{ComparisonOriginRole, ComparisonRenderInput, RenderError};

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

/// One validated query origin and its contributing model anchors.
pub(crate) struct PreparedOrigin<'a> {
    pub(crate) source: &'a SourceFile,
    pub(crate) span: TextRange,
    pub(crate) start: LineColumn,
    pub(crate) end: LineColumn,
    pub(crate) model_origins: Vec<PreparedModelAnchor<'a>>,
}

/// One validated model-definition anchor contributing to an IR origin.
pub(crate) enum PreparedModelAnchor<'a> {
    /// A source-backed model definition without a precise declaration span.
    Document {
        /// Retained model source.
        source: &'a SourceFile,
    },
    /// A source-backed model definition with a precise declaration span.
    Span {
        /// Retained model source.
        source: &'a SourceFile,
        /// Exact definition span.
        span: TextRange,
        /// One-based location at the beginning of `span`.
        start: LineColumn,
        /// One-based location at the end of `span`.
        end: LineColumn,
    },
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

fn prepare_origin<'a>(
    sources: &'a SourceStore,
    role: ComparisonOriginRole,
    origin: &IrOrigin,
) -> Result<PreparedOrigin<'a>, RenderError> {
    let source_span = origin.source();
    let (source, start, end) = validate_span(
        sources,
        role.clone(),
        source_span.file(),
        source_span.range(),
    )?;
    let model_origins = origin
        .model_origins()
        .iter()
        .enumerate()
        .map(|(index, model_origin)| {
            let definition = model_origin.definition();
            prepare_model_anchor(
                sources,
                role.clone().model(index),
                FileId::new(definition.source().index()),
                definition.span(),
            )
        })
        .collect::<Result<Vec<_>, _>>()?;

    Ok(PreparedOrigin {
        source,
        span: source_span.range(),
        start,
        end,
        model_origins,
    })
}

fn prepare_model_anchor<'a>(
    sources: &'a SourceStore,
    role: ComparisonOriginRole,
    file: FileId,
    span: Option<TextRange>,
) -> Result<PreparedModelAnchor<'a>, RenderError> {
    match span {
        Some(span) => {
            let (source, start, end) = validate_span(sources, role, file, span)?;
            Ok(PreparedModelAnchor::Span {
                source,
                span,
                start,
                end,
            })
        }
        None => {
            let source = sources
                .get(file)
                .ok_or(RenderError::UnknownComparisonFile { role, file })?;
            Ok(PreparedModelAnchor::Document { source })
        }
    }
}

fn validate_span(
    sources: &SourceStore,
    role: ComparisonOriginRole,
    file: FileId,
    span: TextRange,
) -> Result<(&SourceFile, LineColumn, LineColumn), RenderError> {
    let source = sources
        .get(file)
        .ok_or_else(|| RenderError::UnknownComparisonFile {
            role: role.clone(),
            file,
        })?;
    let start = span.start();
    let end = span.end();
    let invalid = || RenderError::InvalidComparisonSpan {
        role: role.clone(),
        file,
        start: u32::from(start),
        end: u32::from(end),
    };
    if start > end {
        return Err(invalid());
    }
    let start_location = source.line_column(start).ok_or_else(invalid)?;
    let end_location = source.line_column(end).ok_or_else(invalid)?;
    Ok((source, start_location, end_location))
}
