#![forbid(unsafe_code)]
#![deny(missing_docs)]

//! A small, immutable, lossless concrete syntax tree.
//!
//! The crate translates lexer token kinds exhaustively, validates parser
//! events before constructing a tree, and exposes typed AST views without an
//! unchecked raw-kind conversion. Tree data is immutable and cheaply cloned
//! through [`std::sync::Arc`]; every token references its one shared source
//! allocation by range instead of copying its source text.

mod ast;
mod builder;
mod green;
mod kind;

pub use ast::{
    AllExpression, ArrowCall, AstNode, BinaryExpression, BracketIndex, CallArguments,
    CastExpression, CodeBlock, CollectionLiteral, ColumnInfo, ColumnName, ColumnSpec,
    ColumnSpecArray, DomainAssociationDeclaration, DomainClassDeclaration, DomainExtendsClause,
    DomainFile, DomainMultiplicity, DomainOpaqueBody, DomainOpaqueNode, DomainParameterDeclaration,
    DomainProfileDeclaration, DomainProfileSection, DomainPropertyDeclaration, DomainQualifiedName,
    DomainQualifiedPropertyDeclaration, DomainStereotypeApplications, DomainStereotypeDeclaration,
    DomainTypeReference, ErrorNode, FunctionCall, Island, LambdaExpression, LambdaParameters,
    LetStatement, LiteralExpression, Multiplicity, NavigationPathIsland, NewInstanceExpression,
    OpaqueIsland, ParenthesizedExpression, PropertyNavigation, QualifiedName, QueryExpression,
    RelationType, Root, StoreTablePointer, TypeReference, UnaryExpression, VariableExpression,
};
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
        // `env!` here is evaluated independently of the `version()` body
        // being tested, so this stays a real oracle instead of a tautology:
        // a mutant that swaps `version()`'s return value still fails this
        // assertion, and unlike a hardcoded literal it never goes stale on
        // a workspace version bump.
        assert_eq!(version(), env!("CARGO_PKG_VERSION"));
    }
}
