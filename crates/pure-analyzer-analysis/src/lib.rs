#![forbid(unsafe_code)]
#![deny(missing_docs)]

//! Conservative analysis passes over Pure syntax and model facts.

mod format;
mod lint;
mod local;
mod pass;
mod validate;

pub use format::{FormatResult, format_query};
pub use lint::{MilestoningArityLintPass, NavigationLintPass};
pub use local::{LocalNavigationAnalysis, LocalResolution, LocalResolutionSite, analyze_m3_locals};
pub use pass::{
    AnalysisEngine, AnalysisInput, AnalysisPass, AnalysisResult, FindingPolicy, ModelAvailability,
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
