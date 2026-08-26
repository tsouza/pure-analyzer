#![forbid(unsafe_code)]
#![deny(missing_docs)]

//! A small, immutable, lossless concrete syntax tree.
//!
//! The crate translates lexer token kinds exhaustively, validates parser
//! events before constructing a tree, and exposes typed AST views without an
//! unchecked raw-kind conversion. Tree data is immutable and cheaply cloned
//! through [`std::sync::Arc`].

mod ast;
mod builder;
mod green;
mod kind;

pub use ast::{AstNode, BinaryExpression, Root};
pub use builder::{BuildError, Checkpoint, Event, GreenNodeBuilder};
pub use green::{GreenElement, GreenNode, GreenToken, TokenIter};
pub use kind::{InvalidRawSyntaxKind, RawSyntaxKind, SyntaxKind};
pub use text_size::{TextRange, TextSize};

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
