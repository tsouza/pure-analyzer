#![forbid(unsafe_code)]
#![deny(missing_docs)]

//! Analyzer model package boundary.
//!
//! The crate currently exposes its package version as its complete public API.

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
