use crate::{GreenNode, SyntaxKind};

/// A typed view over an immutable concrete syntax-tree node.
pub trait AstNode: Sized {
    /// Reports whether this wrapper accepts `kind`.
    #[must_use]
    fn can_cast(kind: SyntaxKind) -> bool;

    /// Wraps `syntax` when its kind matches this AST type.
    #[must_use]
    fn cast(syntax: GreenNode) -> Option<Self>;

    /// Returns the underlying concrete syntax-tree node.
    #[must_use]
    fn syntax(&self) -> &GreenNode;
}

macro_rules! ast_node {
    ($name:ident, $kind:ident, $docs:literal) => {
        #[doc = $docs]
        #[derive(Debug, Clone, PartialEq, Eq)]
        pub struct $name(GreenNode);

        impl AstNode for $name {
            fn can_cast(kind: SyntaxKind) -> bool {
                kind == SyntaxKind::$kind
            }

            fn cast(syntax: GreenNode) -> Option<Self> {
                Self::can_cast(syntax.kind()).then_some(Self(syntax))
            }

            fn syntax(&self) -> &GreenNode {
                &self.0
            }
        }
    };
}

ast_node!(Root, ROOT, "A typed view of the tree's root node.");
ast_node!(
    BinaryExpression,
    BINARY_EXPR,
    "A typed view of a binary-expression node."
);
