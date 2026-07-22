#![forbid(unsafe_code)]
#![deny(missing_docs)]

//! The lexer: a `logos`-derived DFA token layer (design doc §4.1).
//!
//! Owns the `#[repr(u16)]` token enum shared by the M3 (query) and Domain
//! (model-definition) grammars — literals, the `%latest`/date family,
//! symbols, and the raw island tokens (`#>{`, `#{`, `}#`, …) that
//! `pure-analyzer-syntax` balances into `Island` nodes rather than a `logos`
//! mode stack (design doc §2.3).
//!
//! **Scaffold status.** This crate is currently a placeholder: the workspace
//! was instantiated from a generic starter kit, and real lexer work
//! (specs/lexer.md) lands as the first feature after bootstrap. See
//! `docs/design/pure-analyzer-design.md` §4.1 for the target token table.

/// The crate's semantic version, as declared in `Cargo.toml`.
///
/// A trivial, genuinely-used placeholder API: it lets `libpure`'s
/// `engine_crate_versions` report on this crate before the real lexer
/// lands, so `libpure`'s facade role is exercised from the first commit
/// rather than added later as an afterthought.
#[must_use]
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_matches_workspace_version() {
        // Asserting non-emptiness alone doesn't kill a mutant that replaces
        // the return value with a different non-empty string (`xyzzy`); pin
        // the exact value instead.
        assert_eq!(version(), "0.1.0");
    }
}
