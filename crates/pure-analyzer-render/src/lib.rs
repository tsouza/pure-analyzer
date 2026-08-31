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

/// Terminal color policy resolved by a front end before human rendering.
///
/// The front end supplies whether its chosen output stream is a terminal, so
/// the renderer remains deterministic and never inspects process-global I/O.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ColorPolicy {
    /// Emit color only when the selected output stream is a terminal.
    #[default]
    Auto,
    /// Always emit ANSI color sequences.
    Always,
    /// Never emit ANSI color sequences.
    Never,
}

impl ColorPolicy {
    /// Resolve this policy using the front end's terminal detection result.
    #[must_use]
    pub const fn resolve(self, is_terminal: bool) -> HumanOptions {
        HumanOptions {
            color: match self {
                Self::Auto => is_terminal,
                Self::Always => true,
                Self::Never => false,
            },
        }
    }
}

/// Options that affect only the terminal-oriented human renderer.
///
/// Construct this with [`ColorPolicy::resolve`] when a front end supports
/// `auto`, `always`, and `never` color choices. This crate emits ANSI color
/// sequences only when `color` is true.
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
/// Regions use one-based UTF-8 code-unit columns and artifact locations use
/// percent-encoded URI paths.
///
/// # Errors
///
/// Returns a renderer-internal error if a finding contains an invalid span or
/// SARIF serialization fails.
pub fn render_sarif(input: RenderInput<'_>) -> Result<String, RenderError> {
    sarif::render(input)
}
