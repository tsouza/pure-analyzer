#![forbid(unsafe_code)]
#![deny(missing_docs)]

//! Facade exposing shared diagnostics and analyzer-crate versions.

mod driver;
mod source;

pub use driver::{
    AnalysisDriver, AnalysisOutput, DriverError, FormatOutput, FormattedSource, LintRequest,
    ModelInput, ParseOutput, ParsedSource, RequestError, SourceRequest,
};
/// Formatter API exposed to front ends through the workspace facade.
pub use pure_analyzer_analysis::{FormatResult, format_query};
pub use pure_analyzer_diagnostics::{
    DiagCode, Diagnostic, FileId, FixPlan, FixPlanError, PlannedChange, PlannedFile, Severity,
    TextRange, TextSize,
};
pub use pure_analyzer_model::ModelError;
pub use pure_analyzer_syntax::{BuildError, GreenNode};
pub use source::{
    LineColumn, SourceFile, SourceInput, SourceOrigin, SourceStore, SourceStoreError,
};

/// One entry of [`engine_crate_versions`]: a crate name paired with its
/// `Cargo.toml`-declared semantic version.
pub type CrateVersion = (&'static str, &'static str);

/// The version of every crate in the analysis-engine dependency order.
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
