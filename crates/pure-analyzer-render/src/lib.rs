#![forbid(unsafe_code)]
#![deny(missing_docs)]

//! Deterministic human, JSON, and SARIF renderers for analyzer results.
//!
//! This crate is deliberately a presentation-only front end. It consumes the
//! retained [`libpure::SourceStore`] snapshot and immutable analyzer results;
//! it neither re-reads sources nor changes findings while rendering them.

mod canonical_emission;
mod canonical_emission_human;
mod canonical_emission_json;
mod comparison;
mod comparison_human;
mod comparison_json;
mod error;
mod human;
mod input;
mod json;
mod origin;
mod sarif;

pub use error::{CanonicalEmissionOriginRole, ComparisonOriginRole, RenderError, SpanKind};

use libpure::{CanonicalEmissionOutcome, ComparisonOutcome, SourceStore};
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

/// Borrowed source snapshots and an exact M4a comparison outcome.
///
/// Construct this only from the same retained source snapshot that produced
/// the comparison outcome. Comparison renderers validate every source and
/// model-origin anchor before producing any output.
#[derive(Clone, Copy, Debug)]
pub struct ComparisonRenderInput<'a> {
    sources: &'a SourceStore,
    outcome: &'a ComparisonOutcome,
}

impl<'a> ComparisonRenderInput<'a> {
    /// Construct renderer input from retained source snapshots and a comparison outcome.
    #[must_use]
    pub const fn new(sources: &'a SourceStore, outcome: &'a ComparisonOutcome) -> Self {
        Self { sources, outcome }
    }
}

/// Borrowed source snapshots and an exact canonical-emission outcome.
///
/// Construct this only from the same retained source snapshot that produced
/// the emission outcome. Refusal renderers validate every source and
/// model-origin anchor before producing any output.
#[derive(Clone, Copy, Debug)]
pub struct CanonicalEmissionRenderInput<'a> {
    sources: &'a SourceStore,
    outcome: &'a CanonicalEmissionOutcome,
}

impl<'a> CanonicalEmissionRenderInput<'a> {
    /// Construct renderer input from retained source snapshots and an emission outcome.
    #[must_use]
    pub const fn new(sources: &'a SourceStore, outcome: &'a CanonicalEmissionOutcome) -> Self {
        Self { sources, outcome }
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
/// Regions use one-based Unicode code-point columns and artifact locations use
/// percent-encoded URI paths.
///
/// # Errors
///
/// Returns a renderer-internal error if a finding contains an invalid span or
/// SARIF serialization fails.
pub fn render_sarif(input: RenderInput<'_>) -> Result<String, RenderError> {
    sarif::render(input)
}

/// Render an M4a comparison outcome as terminal-oriented text.
///
/// Structural refutations name their canonical primary and secondary origins;
/// they never manufacture a data witness. Indecision output retains the exact
/// reason and origin supplied by analysis.
///
/// # Errors
///
/// Returns a renderer-internal error if a comparison origin references an
/// unknown, stale, reversed, out-of-bounds, or non-UTF-8-boundary span.
pub fn render_comparison_human(
    input: ComparisonRenderInput<'_>,
    options: HumanOptions,
) -> Result<String, RenderError> {
    comparison_human::render(input, options)
}

/// Render an M4a comparison outcome in the versioned JSON envelope.
///
/// Structural refutations retain their exact kind, detail, and canonical
/// origins. The representation contains no M4b witness field.
///
/// # Errors
///
/// Returns a renderer-internal error if a comparison origin is invalid or JSON
/// serialization fails.
pub fn render_comparison_json(input: ComparisonRenderInput<'_>) -> Result<String, RenderError> {
    comparison_json::render(input)
}

/// Render a canonical-emission outcome as terminal-oriented text.
///
/// Emitted text is identified as a canonical result rather than a lossless
/// source rewrite. Indecision output retains the exact reason and origin from
/// analysis.
///
/// # Errors
///
/// Returns a renderer-internal error if a refusal origin references an
/// unknown, stale, reversed, out-of-bounds, or non-UTF-8-boundary span.
pub fn render_canonical_emission_human(
    input: CanonicalEmissionRenderInput<'_>,
    options: HumanOptions,
) -> Result<String, RenderError> {
    canonical_emission_human::render(input, options)
}

/// Render a canonical-emission outcome in the versioned JSON envelope.
///
/// The envelope distinguishes a proven emitted normal form from a typed
/// indecision and never claims to preserve source layout or comments.
///
/// # Errors
///
/// Returns a renderer-internal error if a refusal origin is invalid or JSON
/// serialization fails.
pub fn render_canonical_emission_json(
    input: CanonicalEmissionRenderInput<'_>,
) -> Result<String, RenderError> {
    canonical_emission_json::render(input)
}
