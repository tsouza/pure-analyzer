#![forbid(unsafe_code)]
#![deny(missing_docs)]

//! The shared `Diagnostic` model.
//!
//! Every pass in the analysis engine — the parser's syntax errors, `lint`'s
//! milestoning-arity findings, `eq`'s verdicts — emits [`Diagnostic`] values
//! and nothing else. This crate defines that shape and **no renderer**;
//! rendering belongs in front-end crates. Keeping this crate a leaf with a
//! small, serializable shape keeps diagnostics independent of presentation.

mod code;
mod diagnostic;
mod explain;
mod file;
mod fix;
mod fix_plan;
mod verdict;

pub use code::{ALL_DIAG_CODES, DiagCode, DiagFamily, UnknownDiagCode};
pub use diagnostic::{Diagnostic, DiagnosticBuilder, Label, Severity};
pub use explain::{
    EXPLAIN_INDEX_URL, ExplainClassification, ExplainContent, ExplainKind,
    UnknownExplainIdentifier, lookup_explanation,
};
pub use file::FileId;
pub use fix::{Applicability, Fix, FixProvenance, TextEdit};
pub use fix_plan::{FixPlan, FixPlanError, PlannedChange, PlannedFile};
pub use text_size::{TextRange, TextSize};
pub use verdict::{ALL_REASON_CODES, ReasonBucket, ReasonCode, UnknownReasonCode, Verdict};
