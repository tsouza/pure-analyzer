#![forbid(unsafe_code)]
#![deny(missing_docs)]

//! The `Pass`/visitor layer: `validate` (grammar fidelity + over-admission
//! guards, model-free) and `lint` (milestoning arity, unknown-property,
//! cardinality misuse; needs a model) (design doc §5.1, §5.2).
//!
//! This is the fact/policy boundary's policy half: `pure-analyzer-resolve`
//! emits structural facts, and the passes here compare a fact's `surface`
//! against its `accepted` set to decide Warning vs. Error vs. silence. `eq`
//! and `fmt` (v0.2) will be thin dispatchers from here into
//! `pure-analyzer-ir`/`pure-analyzer-eq`, kept in their own crates so the
//! sound core (`validate`+`lint`) never pulls in the heavier interpreter.
//!
//! **Scaffold status.** This crate is currently a placeholder: the workspace
//! was instantiated from a generic starter kit, and the real passes land
//! after the resolver. See `docs/design/pure-analyzer-design.md` §5 for the
//! target diagnostic roster.

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
