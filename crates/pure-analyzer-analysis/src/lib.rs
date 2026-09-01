#![forbid(unsafe_code)]
#![deny(missing_docs)]

//! Conservative analysis passes over Pure syntax and model facts.

mod format;
mod lint;
mod local;
mod lowering;
mod pass;
mod relational;
mod validate;

pub use format::{FormatResult, format_query, format_query_with_width};
pub use lint::{MilestoningArityLintPass, NavigationLintPass};
pub use local::{LocalNavigationAnalysis, LocalResolution, LocalResolutionSite, analyze_m3_locals};
pub use lowering::lower_m3_query;
pub use pass::{
    AnalysisEngine, AnalysisInput, AnalysisPass, AnalysisResult, FindingPolicy, ModelAvailability,
};
pub use relational::{
    CandidateKey, Column, ColumnId, IrOrigin, JoinKind, Knowledge, ModelOrigin, ModelOriginKind,
    Nullability, OpaqueOutcome, Projection, RelationExpression, RelationExpressionError,
    RelationFacts, RelationOperator, RelationSchema, RelationSource, RelationalOutcome,
    RelationalQuery, ResolvedNavigation, RowSemantics, ScalarExpression, ScalarLiteral,
    ScalarOperator, SchemaError, SourceSpan, Totality,
};
pub use validate::ValidatePass;

/// The crate's semantic version, as declared in `Cargo.toml`.
#[must_use]
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_matches_workspace_version() {
        assert_eq!(version(), "0.1.0");
    }
}
