use crate::{GreenNode, SyntaxKind, TextRange};

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

    /// Returns the exact source range covered by this node.
    #[must_use]
    fn text_range(&self) -> TextRange {
        self.syntax().text_range()
    }
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
ast_node!(
    QueryExpression,
    QUERY_EXPR,
    "A typed view of a complete query expression."
);
ast_node!(
    AllExpression,
    ALL_EXPR,
    "A typed view of an all-expression node."
);
ast_node!(
    QualifiedName,
    QUALIFIED_NAME,
    "A typed view of a qualified-name node."
);
ast_node!(
    VariableExpression,
    VARIABLE_EXPR,
    "A typed view of a variable-expression node."
);
ast_node!(
    LiteralExpression,
    LITERAL_EXPR,
    "A typed view of a literal-expression node."
);
ast_node!(
    ParenthesizedExpression,
    PAREN_EXPR,
    "A typed view of a parenthesized-expression node."
);
ast_node!(
    UnaryExpression,
    UNARY_EXPR,
    "A typed view of a unary-expression node."
);
ast_node!(ArrowCall, ARROW_CALL, "A typed view of an arrow-call node.");
ast_node!(
    PropertyNavigation,
    PROPERTY_NAV,
    "A typed view of a property-navigation node."
);
ast_node!(
    BracketIndex,
    BRACKET_INDEX,
    "A typed view of a bracket-index node."
);
ast_node!(CallArguments, CALL_ARGS, "A typed view of call arguments.");
ast_node!(
    FunctionCall,
    FUNCTION_CALL,
    "A typed view of a function-call node."
);
ast_node!(
    CollectionLiteral,
    COLLECTION_LITERAL,
    "A typed view of a collection-literal node."
);
ast_node!(
    LambdaExpression,
    LAMBDA_EXPR,
    "A typed view of a lambda-expression node."
);
ast_node!(
    LambdaParameters,
    LAMBDA_PARAMS,
    "A typed view of lambda parameters."
);
ast_node!(CodeBlock, CODE_BLOCK, "A typed view of a code-block node.");
ast_node!(
    LetStatement,
    LET_STMT,
    "A typed view of a let-statement node."
);
ast_node!(
    ColumnSpec,
    COLUMN_SPEC,
    "A typed view of a relation column specification."
);
ast_node!(
    ColumnSpecArray,
    COLUMN_SPEC_ARRAY,
    "A typed view of an array of relation column specifications."
);
ast_node!(
    NewInstanceExpression,
    NEW_INSTANCE_EXPR,
    "A typed view of a new-instance expression."
);
ast_node!(
    CastExpression,
    CAST_EXPR,
    "A typed view of a cast expression."
);
ast_node!(
    RelationType,
    RELATION_TYPE,
    "A typed view of a relation-type node."
);
ast_node!(
    ColumnInfo,
    COLUMN_INFO,
    "A typed view of relation column information."
);
ast_node!(Island, ISLAND, "A typed view of an island node.");
ast_node!(
    StoreTablePointer,
    STORE_TABLE_POINTER,
    "A typed view of a deeply parsed store-table pointer."
);
ast_node!(
    NavigationPathIsland,
    NAV_PATH_ISLAND,
    "A typed view of an opaque navigation-path island."
);
ast_node!(
    OpaqueIsland,
    OPAQUE_ISLAND,
    "A typed view of an opaque island."
);
ast_node!(TypeReference, TYPE_REF, "A typed view of a type reference.");
ast_node!(
    Multiplicity,
    MULTIPLICITY,
    "A typed view of a multiplicity node."
);
ast_node!(
    ErrorNode,
    ERROR_NODE,
    "A typed view of a recovered error region."
);
