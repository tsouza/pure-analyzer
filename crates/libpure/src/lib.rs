#![forbid(unsafe_code)]
#![deny(missing_docs)]

//! `libpure`: the analysis engine as one product, `pub use`-facaded over its
//! layered crates (design doc §1.5, §3).
//!
//! `pure-analyzer-cli` (and, in v0.2, `pure-analyzer-lsp`) are thin adapters
//! over this crate: they render the `Diagnostic`s it produces and turn
//! `Fix`es into their own protocol's edit representation, but the parsing,
//! model loading, resolution, and pass logic all live below this facade.
//!
//! **Scaffold status.** The sub-crates below (`pure-analyzer-lexer` through
//! `pure-analyzer-analysis`) are currently placeholders — see each crate's
//! own docs — so this facade currently re-exports only what already exists:
//! the shared `Diagnostic` model and each sub-crate's version, so the DAG
//! (design doc §3) is exercised end-to-end from the first commit rather than
//! wired up later as an afterthought.

pub use pure_analyzer_diagnostics::{Diagnostic, Severity};

/// One entry of [`engine_crate_versions`]: a crate name paired with its
/// `Cargo.toml`-declared semantic version.
pub type CrateVersion = (&'static str, &'static str);

/// The version of every crate in the analysis-engine DAG, in dependency
/// order (design doc §3): lexer, syntax, parser, model, resolve, analysis.
#[must_use]
pub fn engine_crate_versions() -> Vec<CrateVersion> {
    vec![
        ("pure-analyzer-lexer", pure_analyzer_lexer::version()),
        ("pure-analyzer-syntax", pure_analyzer_syntax::version()),
        ("pure-analyzer-parser", pure_analyzer_parser::version()),
        ("pure-analyzer-model", pure_analyzer_model::version()),
        ("pure-analyzer-resolve", pure_analyzer_resolve::version()),
        ("pure-analyzer-analysis", pure_analyzer_analysis::version()),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_every_engine_crate() {
        let versions = engine_crate_versions();
        assert_eq!(versions.len(), 6);
        for (name, version) in versions {
            assert!(
                name.starts_with("pure-analyzer-"),
                "unexpected crate name: {name}"
            );
            assert!(!version.is_empty(), "{name} reported an empty version");
        }
    }
}
