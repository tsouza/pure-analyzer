#![forbid(unsafe_code)]
#![deny(missing_docs)]

//! The resilient parser: hand-written recursive-descent + Pratt/TDOP over the
//! event model (`Open`/`Advance`/`Close`), folded into a `rowan` green tree
//! by a builder (design doc §4.2).
//!
//! Parses two grammar layers from the same event machinery: M3 (query
//! expressions) via `parse_query`, and Domain (`Class`/`Association`/
//! `Profile` definitions) via `parse_model`. Total parsing: the parser never
//! panics or bails, every node and diagnostic carries a byte-accurate span,
//! and every recovery loop is fuel-bounded (guards the classic recursive-
//! descent infinite-loop pitfall; enforced by `cargo-fuzz` once the real
//! parser lands).
//!
//! **Scaffold status.** This crate is currently a placeholder: the workspace
//! was instantiated from a generic starter kit, and the real parser lands
//! after the lexer/syntax layers. See `docs/design/pure-analyzer-design.md`
//! §4.2 for the target grammar and AST shape.

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
