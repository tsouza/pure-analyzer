use thiserror::Error;

macro_rules! define_syntax_kinds {
    (
        tokens { $( $token:ident = $token_value:literal ),+ $(,)? }
        nodes { $( $node:ident = $node_value:literal ),+ $(,)? }
    ) => {
        /// A terminal or nonterminal kind in a concrete syntax tree.
        ///
        /// The stable token-ID namespace is `0x0000..=0x7fff`; current
        /// assignments are contiguous at `0x0000..=0x0031`. The stable node-ID
        /// namespace is the disjoint `0x8000..=0xffff`; current assignments are
        /// `0x8000..=0x8002`. Existing assignments do not change.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        #[repr(u16)]
        #[allow(non_camel_case_types, missing_docs)]
        pub enum SyntaxKind {
            $( $token = $token_value, )+
            $( $node = $node_value, )+
        }

        impl SyntaxKind {
            /// Returns every currently defined kind in discriminant order.
            #[must_use]
            pub const fn all() -> &'static [Self] {
                &[$( Self::$token, )+ $( Self::$node, )+]
            }

            /// Reports whether this kind represents a lexer token.
            #[must_use]
            pub const fn is_token(self) -> bool {
                matches!(self, $( Self::$token )|+)
            }

            /// Reports whether this kind represents a syntax-tree node.
            #[must_use]
            pub const fn is_node(self) -> bool {
                matches!(self, $( Self::$node )|+)
            }
        }

        impl From<pure_analyzer_lexer::SyntaxKind> for SyntaxKind {
            fn from(kind: pure_analyzer_lexer::SyntaxKind) -> Self {
                match kind {
                    $( pure_analyzer_lexer::SyntaxKind::$token => Self::$token, )+
                }
            }
        }
    };
}

define_syntax_kinds! {
    tokens {
        DATE_TIME = 0x0000,
        STRICT_DATE = 0x0001,
        LATEST_DATE = 0x0002,
        PERCENT = 0x0003,
        TILDE = 0x0004,
        DOLLAR = 0x0005,
        ARROW = 0x0006,
        PIPE = 0x0007,
        AT = 0x0008,
        NEW_SYMBOL = 0x0009,
        DOT = 0x000a,
        COMMA = 0x000b,
        PATH_SEPARATOR = 0x000c,
        COLON = 0x000d,
        PAREN_OPEN = 0x000e,
        PAREN_CLOSE = 0x000f,
        BRACKET_OPEN = 0x0010,
        BRACKET_CLOSE = 0x0011,
        EQ = 0x0012,
        NEQ = 0x0013,
        PLUS = 0x0014,
        MINUS = 0x0015,
        STAR = 0x0016,
        SLASH = 0x0017,
        LE = 0x0018,
        LT = 0x0019,
        GE = 0x001a,
        GT = 0x001b,
        SEMICOLON = 0x001c,
        BRACE_OPEN = 0x001d,
        BRACE_CLOSE = 0x001e,
        ALL_KW = 0x001f,
        LET_KW = 0x0020,
        ALL_VERSIONS_KW = 0x0021,
        ALL_VERSIONS_IN_RANGE_KW = 0x0022,
        TO_BYTES_KW = 0x0023,
        IDENT = 0x0024,
        INTEGER = 0x0025,
        BOOLEAN = 0x0026,
        STRING = 0x0027,
        HASH_STORE_OPEN = 0x0028,
        HASH_ISLAND_OPEN = 0x0029,
        NAV_PATH_BLOCK = 0x002a,
        ISLAND_END = 0x002b,
        HASH = 0x002c,
        WHITESPACE = 0x002d,
        LINE_COMMENT = 0x002e,
        BLOCK_COMMENT = 0x002f,
        ERROR = 0x0030,
        ASSIGN = 0x0031,
    }
    nodes {
        ROOT = 0x8000,
        ERROR_NODE = 0x8001,
        BINARY_EXPR = 0x8002,
    }
}

/// The serialized representation of a [`SyntaxKind`].
///
/// Its field is private so reading a raw value always goes through the checked
/// [`TryFrom`] implementation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RawSyntaxKind(u16);

impl RawSyntaxKind {
    /// Creates a raw kind for decoding at a trust boundary.
    #[must_use]
    pub const fn new(value: u16) -> Self {
        Self(value)
    }

    /// Returns the underlying integer for serialization.
    #[must_use]
    pub const fn get(self) -> u16 {
        self.0
    }
}

impl From<SyntaxKind> for RawSyntaxKind {
    fn from(kind: SyntaxKind) -> Self {
        Self(kind as u16)
    }
}

/// The error returned when a raw integer does not name a syntax kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("{value} is not a valid syntax kind")]
pub struct InvalidRawSyntaxKind {
    value: u16,
}

impl InvalidRawSyntaxKind {
    /// Returns the rejected integer.
    #[must_use]
    pub const fn value(self) -> u16 {
        self.value
    }
}

impl TryFrom<RawSyntaxKind> for SyntaxKind {
    type Error = InvalidRawSyntaxKind;

    fn try_from(raw: RawSyntaxKind) -> Result<Self, Self::Error> {
        Self::all()
            .iter()
            .copied()
            .find(|kind| RawSyntaxKind::from(*kind).get() == raw.0)
            .ok_or(InvalidRawSyntaxKind { value: raw.0 })
    }
}

impl From<RawSyntaxKind> for u16 {
    fn from(kind: RawSyntaxKind) -> Self {
        kind.0
    }
}
