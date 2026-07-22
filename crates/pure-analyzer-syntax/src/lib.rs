#![forbid(unsafe_code)]
#![deny(missing_docs)]

//! `SyntaxKind` and the `rowan` `Language` implementation over the lossless
//! CST, plus `ungrammar`-codegen'd typed `AstNode` views (design doc §4.2).
//!
//! `SyntaxKind(u16) <-> enum` conversion uses `num-derive`'s `FromPrimitive`,
//! never `unsafe { transmute }` — the workspace forbids `unsafe_code`
//! entirely, and a green tree built from an out-of-range discriminant must
//! fail a checked conversion, not read undefined memory.
//!
//! **Scaffold status.** This crate is currently a placeholder: the workspace
//! was instantiated from a generic starter kit, and the real `SyntaxKind`/CST
//! work lands alongside the lexer/parser features. See
//! `docs/design/pure-analyzer-design.md` §4.2 for the target AST shape.

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
