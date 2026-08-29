#![forbid(unsafe_code)]
#![deny(missing_docs)]

//! Resilient, lossless parsers for the analyzer's supported Pure surface.
//!
//! [`parse_query`] accepts a modern M3/Relation query and returns a concrete
//! syntax tree even when its source is incomplete or malformed. Ordinary
//! syntax failures are reported in [`Parse::diagnostics`]; the `Result` only
//! reports an infrastructure failure while constructing the validated tree.

mod domain;
mod m3;

pub use domain::{DomainCoverageGap, DomainCoverageGapKind, DomainParse, parse_domain};
pub use m3::{Parse, parse_query};

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
