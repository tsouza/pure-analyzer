#![forbid(unsafe_code)]
#![deny(missing_docs)]

//! The model loader: builds a `ModelGraph` from PMCD JSON and/or a
//! Pure-model-file (Domain grammar), the single fact milestoning arity needs
//! — the target class's temporal stereotype (design doc §4.3, §7).
//!
//! The Pure-model-file path is first-class, not a fallback: pure-analyzer
//! delivers full-strength arity linting without ever running the Legend
//! engine. PMCD JSON is an optional coverage booster for associations and
//! richer qualified-property classification. Per-class provenance
//! (`Pmcd` vs `PureFile`) drives the closed-world/open-world split that keeps
//! `lint` honest under an unconfirmed stereotype (never a false Error).
//!
//! **Scaffold status.** This crate is currently a placeholder: the workspace
//! was instantiated from a generic starter kit, and the real model loader
//! lands after the parser's Domain-grammar path exists. See
//! `docs/design/pure-analyzer-design.md` §4.3 for the target `ModelGraph`
//! shape.

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
