#![forbid(unsafe_code)]
#![deny(missing_docs)]

//! The resolver: the proven core. Threads a small, bounded local type
//! environment through lambda bodies rooted at `Class.all()`, and computes
//! the `%latest`-arity *fact* for each navigation hop — fresh per hop, never
//! propagated globally (design doc §4.4, the corrected context-gate rule).
//!
//! This crate emits only structural facts (`Hop { surface, accepted, kind,
//! … }`, `PUR2002 unknown-property`, `PUR2101 unknown-source`); it emits no
//! policy. `pure-analyzer-analysis` compares the fact against the surface to
//! produce the `PUR2001 wrong-milestoning-arity` lint Warning/Error.
//!
//! **Scaffold status.** This crate is currently a placeholder: the workspace
//! was instantiated from a generic starter kit, and the real resolver lands
//! after the model loader. See `docs/design/pure-analyzer-design.md` §4.4 for
//! the target algorithm and the full milestoning-arity truth table.

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
