#![forbid(unsafe_code)]
#![deny(missing_docs)]

//! The shared `Diagnostic` model (design doc §6.1).
//!
//! Every pass in the analysis engine — the parser's syntax errors, `lint`'s
//! milestoning-arity findings, `eq`'s verdicts — emits [`Diagnostic`] values
//! and nothing else. This crate defines that shape and **no renderer**: the
//! CLI (`ariadne`/`codespan-reporting`) and, later, the LSP
//! (`codespan-lsp`/`lsp_types`) are the only crates allowed to turn a
//! `Diagnostic` into human- or protocol-facing output (see
//! `ast-grep-rules/no-front-end-deps-in-core.yml`). Keeping this crate a leaf
//! with a tiny, stable, serializable shape is what lets the CLI and LSP stay
//! thin adapters over identical findings.

mod diagnostic;
mod file;
mod fix;
mod verdict;

pub use diagnostic::{Diagnostic, DiagnosticBuilder, Label, Severity};
pub use file::FileId;
pub use fix::{Applicability, Fix, TextEdit};
pub use text_size::{TextRange, TextSize};
pub use verdict::{ReasonBucket, ReasonCode, Verdict};
