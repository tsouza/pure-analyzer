#![forbid(unsafe_code)]
#![deny(missing_docs)]

//! Deterministic human, JSON, and SARIF renderers for analyzer diagnostics.
//!
//! This crate is deliberately a presentation-only front end. It consumes the
//! retained [`libpure::SourceStore`] snapshot and immutable
//! [`pure_analyzer_diagnostics::Diagnostic`] values; it neither re-reads
//! sources nor changes findings while rendering them.

mod error;
mod human;
mod input;
mod json;
mod sarif;

pub use error::{RenderError, SpanKind};

use libpure::SourceStore;
use pure_analyzer_diagnostics::Diagnostic;

/// Borrowed source snapshots and structured findings supplied to a renderer.
///
/// Construct this only from the same retained source snapshot that produced
/// the diagnostics. Each rendering entry point validates every label and fix
/// edit before producing any output.
#[derive(Clone, Copy, Debug)]
pub struct RenderInput<'a> {
    sources: &'a SourceStore,
    diagnostics: &'a [Diagnostic],
}

impl<'a> RenderInput<'a> {
    /// Construct renderer input from retained source snapshots and findings.
    #[must_use]
    pub const fn new(sources: &'a SourceStore, diagnostics: &'a [Diagnostic]) -> Self {
        Self {
            sources,
            diagnostics,
        }
    }
}

/// Options that affect only the terminal-oriented human renderer.
///
/// `color` is already resolved by the caller. Front ends own choices such as
/// `auto`, `always`, and `never` plus TTY detection; this crate merely emits
/// ANSI color sequences when requested.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HumanOptions {
    /// Whether rendered human output includes ANSI color sequences.
    pub color: bool,
}

/// Render findings as grouped, labeled terminal-oriented text.
///
/// # Errors
///
/// Returns a renderer-internal error if a finding references an unknown,
/// stale, reversed, out-of-bounds, or non-UTF-8-boundary span.
pub fn render_human(input: RenderInput<'_>, options: HumanOptions) -> Result<String, RenderError> {
    human::render(input, options)
}

/// Render findings in the versioned JSON envelope.
///
/// The output contains byte offsets plus one-based line and byte-column
/// positions. It always uses the same canonical finding order.
///
/// # Errors
///
/// Returns a renderer-internal error if a finding contains an invalid span or
/// JSON serialization fails.
pub fn render_json(input: RenderInput<'_>) -> Result<String, RenderError> {
    json::render(input)
}

/// Render findings as a SARIF 2.1.0 log.
///
/// # Errors
///
/// Returns a renderer-internal error if a finding contains an invalid span or
/// SARIF serialization fails.
pub fn render_sarif(input: RenderInput<'_>) -> Result<String, RenderError> {
    sarif::render(input)
}
