#![forbid(unsafe_code)]
#![deny(missing_docs)]

//! Conservative analysis passes over Pure syntax and model facts.

mod canonical;
mod column_selectors;
mod comparison;
mod cst_util;
mod format;
mod lint;
mod local;
mod lowering;
mod normalizer;
mod pass;
mod relational;
mod validate;
pub use canonical::{
    CanonicalEmissionIndecision, CanonicalEmissionOutcome, CanonicalPure,
    emit_canonical_lowered_query, emit_canonical_lowered_query_with_budget,
    emit_canonical_normal_form, emit_canonical_normalization,
};
pub use column_selectors::{
    ColumnSelector, ColumnSelectorName, ColumnSelectorOpaque, ColumnSelectorOpaqueReason,
    ColumnSelectorOutcome, ColumnSelectors, ResolvedColumnSelector, ResolvedColumnSelectors,
    extract_relation_column_selectors, resolve_relation_column_selectors,
};
pub use comparison::{
    ComparisonIndecision, ComparisonOutcome, OutputSchemaField, StructuralDifference,
    StructuralDifferenceKind, compare_lowered_queries, compare_lowered_queries_with_budget,
    compare_relational_queries, compare_relational_queries_with_budget,
};

pub use format::{FormatResult, format_query, format_query_with_width};
pub use lint::{MilestoningArityLintPass, NavigationLintPass};
pub use local::{LocalNavigationAnalysis, LocalResolution, LocalResolutionSite, analyze_m3_locals};
pub use lowering::lower_m3_query;
pub use normalizer::{
    DEFAULT_NORMALIZATION_STEP_LIMIT, EquivalenceKey, NormalizationBudget, NormalizationFailure,
    NormalizationOutcome, NormalizedQuery, StructuralKey, normalize_relational_query,
    normalize_relational_query_with_budget,
};
pub use pass::{
    AnalysisEngine, AnalysisInput, AnalysisPass, AnalysisResult, FindingPolicy, ModelAvailability,
};
pub use relational::{
    CandidateKey, Column, ColumnId, IrOrigin, JoinKind, Knowledge, ModelOrigin, ModelOriginKind,
    Nullability, OpaqueOutcome, Projection, ProjectionKind, RelationExpression,
    RelationExpressionError, RelationFacts, RelationOperator, RelationSchema, RelationSource,
    RelationalOutcome, RelationalQuery, ResolvedNavigation, RowSemantics, ScalarExpression,
    ScalarLiteral, ScalarOperator, SchemaError, SortDirection, SortKey, SourceSpan, Totality,
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
        // `env!` here is evaluated independently of the `version()` body
        // being tested, so this stays a real oracle instead of a tautology:
        // a mutant that swaps `version()`'s return value still fails this
        // assertion, and unlike a hardcoded literal it never goes stale on
        // a workspace version bump.
        assert_eq!(version(), env!("CARGO_PKG_VERSION"));
    }
}
