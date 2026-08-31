//! Renderer error types.

use std::fmt;

use pure_analyzer_diagnostics::FileId;
use thiserror::Error;

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
