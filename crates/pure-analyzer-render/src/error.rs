//! Renderer error types.

use std::fmt;

use pure_analyzer_diagnostics::FileId;
use thiserror::Error;

/// The role of an invalid origin in a comparison result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ComparisonOriginRole {
    /// The canonical primary origin of a structural refutation.
    StructuralPrimary,
    /// The canonical secondary origin of a structural refutation.
    StructuralSecondary,
    /// The origin attached to an indecisive comparison result.
    Indecision,
    /// A model anchor contributing to the canonical primary origin.
    StructuralPrimaryModel(usize),
    /// A model anchor contributing to the canonical secondary origin.
    StructuralSecondaryModel(usize),
    /// A model anchor contributing to an indecisive origin.
    IndecisionModel(usize),
}

impl ComparisonOriginRole {
    pub(crate) const fn model(self, index: usize) -> Self {
        match self {
            Self::StructuralPrimary | Self::StructuralPrimaryModel(_) => {
                Self::StructuralPrimaryModel(index)
            }
            Self::StructuralSecondary | Self::StructuralSecondaryModel(_) => {
                Self::StructuralSecondaryModel(index)
            }
            Self::Indecision | Self::IndecisionModel(_) => Self::IndecisionModel(index),
        }
    }
}

impl fmt::Display for ComparisonOriginRole {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::StructuralPrimary => formatter.write_str("primary structural"),
            Self::StructuralSecondary => formatter.write_str("secondary structural"),
            Self::Indecision => formatter.write_str("indecision"),
            Self::StructuralPrimaryModel(index) => {
                write!(formatter, "primary structural model anchor #{index}")
            }
            Self::StructuralSecondaryModel(index) => {
                write!(formatter, "secondary structural model anchor #{index}")
            }
            Self::IndecisionModel(index) => {
                write!(formatter, "indecision model anchor #{index}")
            }
        }
    }
}

/// The role of an invalid span in a diagnostic.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SpanKind {
    /// The finding's primary location.
    Primary,
    /// A secondary location, identified by its zero-based index.
    Secondary(usize),
    /// A structured-fix edit, identified by its zero-based index.
    FixEdit(usize),
}

impl fmt::Display for SpanKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Primary => formatter.write_str("primary label"),
            Self::Secondary(index) => write!(formatter, "secondary label #{index}"),
            Self::FixEdit(index) => write!(formatter, "fix edit #{index}"),
        }
    }
}

/// A failure internal to rendering, never a replacement diagnostic.
#[derive(Debug, Error)]
pub enum RenderError {
    /// A label or edit refers to no file in the retained source snapshot.
    #[error("diagnostic #{diagnostic_index} {kind} refers to unknown source file {file}")]
    UnknownFile {
        /// Index of the original diagnostic slice entry.
        diagnostic_index: usize,
        /// Role of the bad span.
        kind: SpanKind,
        /// Unknown source identity.
        file: FileId,
    },
    /// A label or edit has invalid byte bounds for its retained source.
    #[error("diagnostic #{diagnostic_index} {kind} has invalid byte span {start}..{end} in {file}")]
    InvalidSpan {
        /// Index of the original diagnostic slice entry.
        diagnostic_index: usize,
        /// Role of the bad span.
        kind: SpanKind,
        /// Source identity owning the span.
        file: FileId,
        /// Start byte offset supplied by the finding.
        start: u32,
        /// End byte offset supplied by the finding.
        end: u32,
    },
    /// A comparison origin refers to no file in the retained source snapshot.
    #[error("comparison {role} origin refers to unknown source file {file}")]
    UnknownComparisonFile {
        /// Role of the invalid comparison origin.
        role: ComparisonOriginRole,
        /// Unknown source identity.
        file: FileId,
    },
    /// A comparison origin has invalid byte bounds for its retained source.
    #[error("comparison {role} origin has invalid byte span {start}..{end} in {file}")]
    InvalidComparisonSpan {
        /// Role of the invalid comparison origin.
        role: ComparisonOriginRole,
        /// Source identity owning the invalid span.
        file: FileId,
        /// Start byte offset supplied by the comparison result.
        start: u32,
        /// End byte offset supplied by the comparison result.
        end: u32,
    },
    /// A structured output document could not be serialized.
    #[error("could not serialize {format} renderer output: {source}")]
    Serialization {
        /// Stable name of the failed output representation.
        format: &'static str,
        /// Underlying serializer failure.
        #[source]
        source: serde_json::Error,
    },
}
