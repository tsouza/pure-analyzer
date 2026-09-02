#![forbid(unsafe_code)]
#![deny(missing_docs)]

//! Facade exposing shared diagnostics, explain lookup, and analyzer-crate versions.

mod driver;
mod source;

pub use driver::{
    AnalysisDriver, AnalysisOutput, ComparisonOutput, ComparisonRequest, DefinitionPosition,
    DefinitionResult, DefinitionTarget, DefinitionUnavailable, DiagnosticPolicy, DriverError,
    FormatOutput, FormattedSource, LintRequest, ModelInput, ParseOutput, ParsedSource,
    RequestError, SourceRequest,
};
/// Formatter API exposed to front ends through the workspace facade.
pub use pure_analyzer_analysis::{
    ComparisonIndecision, ComparisonOutcome, FormatResult, IrOrigin, NormalizationBudget,
    OutputSchemaField, SourceSpan, StructuralDifference, StructuralDifferenceKind, format_query,
    format_query_with_width,
};
pub use pure_analyzer_diagnostics::{
    ALL_DIAG_CODES, DiagCode, Diagnostic, EXPLAIN_INDEX_URL, ExplainClassification, ExplainContent,
    ExplainKind, FileId, FixPlan, FixPlanError, PlannedChange, PlannedFile, ReasonCode, Severity,
    TextRange, TextSize, UnknownExplainIdentifier,
};
pub use pure_analyzer_model::ModelError;
pub use pure_analyzer_syntax::{BuildError, GreenNode};
pub use source::{
    LineColumn, SourceFile, SourceInput, SourceOrigin, SourceStore, SourceStoreError,
};

/// Look up renderer-neutral explain content for an exact diagnostic or reason identifier.
///
/// # Errors
///
/// Returns [`UnknownExplainIdentifier`] when `identifier` is not an exact,
/// registered diagnostic or reason identifier.
pub fn explain(identifier: &str) -> Result<&'static ExplainContent, UnknownExplainIdentifier> {
    pure_analyzer_diagnostics::lookup_explanation(identifier)
}

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

    #[test]
    fn exposes_exact_renderer_neutral_explain_content() {
        let diagnostic = explain("PUR2001").expect("registered diagnostic");
        assert_eq!(diagnostic.identifier, "PUR2001");
        assert_eq!(diagnostic.kind, ExplainKind::Diagnostic);

        let reason = explain("IND_WINDOW").expect("registered reason");
        assert_eq!(reason.identifier, "IND_WINDOW");
        assert_eq!(reason.kind, ExplainKind::Reason);

        let error = explain("pur2001").expect_err("identifier parsing is exact");
        assert_eq!(error.value(), "pur2001");
    }
}
