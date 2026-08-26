use std::{fmt, slice, sync::Arc};

use crate::{SyntaxKind, TextRange};

/// An immutable terminal token in a concrete syntax tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GreenToken {
    kind: SyntaxKind,
    text: Arc<str>,
    range: TextRange,
}

impl GreenToken {
    pub(crate) fn new(kind: SyntaxKind, text: &str, range: TextRange) -> Self {
        Self {
            kind,
            text: Arc::from(text),
            range,
        }
    }

    /// Returns this token's terminal kind.
    #[must_use]
    pub const fn kind(&self) -> SyntaxKind {
        self.kind
    }

    /// Returns this token's exact source text.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Returns this token's byte range in the source.
    #[must_use]
    pub const fn text_range(&self) -> TextRange {
        self.range
    }
}

/// An immutable node or token child.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GreenElement {
    /// A nested syntax node.
    Node(GreenNode),
    /// A terminal token.
    Token(GreenToken),
}

impl GreenElement {
    /// Returns this element's terminal or nonterminal kind.
    #[must_use]
    pub fn kind(&self) -> SyntaxKind {
        match self {
            Self::Node(node) => node.kind(),
            Self::Token(token) => token.kind(),
        }
    }

    /// Returns this element's byte range in the source.
    #[must_use]
    pub fn text_range(&self) -> TextRange {
        match self {
            Self::Node(node) => node.text_range(),
            Self::Token(token) => token.text_range(),
        }
    }

    /// Returns the nested node when this element is nonterminal.
    #[must_use]
    pub const fn as_node(&self) -> Option<&GreenNode> {
        match self {
            Self::Node(node) => Some(node),
            Self::Token(_) => None,
        }
    }

    /// Returns the token when this element is terminal.
    #[must_use]
    pub const fn as_token(&self) -> Option<&GreenToken> {
        match self {
            Self::Node(_) => None,
            Self::Token(token) => Some(token),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
struct GreenNodeData {
    kind: SyntaxKind,
    children: Arc<[GreenElement]>,
    range: TextRange,
}

/// An immutable, cheaply cloned concrete syntax-tree node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GreenNode(Arc<GreenNodeData>);

impl GreenNode {
    pub(crate) fn new(kind: SyntaxKind, children: Vec<GreenElement>, range: TextRange) -> Self {
        Self(Arc::new(GreenNodeData {
            kind,
            children: children.into(),
            range,
        }))
    }

    /// Returns this node's nonterminal kind.
    #[must_use]
    pub fn kind(&self) -> SyntaxKind {
        self.0.kind
    }

    /// Returns this node's direct children in source order.
    #[must_use]
    pub fn children(&self) -> &[GreenElement] {
        &self.0.children
    }

    /// Returns this node's byte range in the source.
    #[must_use]
    pub fn text_range(&self) -> TextRange {
        self.0.range
    }

    /// Traverses every descendant token in source order.
    #[must_use]
    pub fn tokens(&self) -> TokenIter<'_> {
        TokenIter {
            stack: vec![self.children().iter()],
        }
    }

    /// Re-emits the exact source text covered by this node.
    #[must_use]
    pub fn text(&self) -> String {
        self.tokens().map(GreenToken::text).collect()
    }
}

impl fmt::Display for GreenNode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for token in self.tokens() {
            formatter.write_str(token.text())?;
        }
        Ok(())
    }
}

/// A depth-first iterator over a node's descendant tokens.
#[derive(Debug)]
pub struct TokenIter<'tree> {
    stack: Vec<slice::Iter<'tree, GreenElement>>,
}

impl<'tree> Iterator for TokenIter<'tree> {
    type Item = &'tree GreenToken;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let children = self.stack.last_mut()?;
            match children.next() {
                Some(GreenElement::Node(node)) => self.stack.push(node.children().iter()),
                Some(GreenElement::Token(token)) => return Some(token),
                None => {
                    self.stack.pop();
                }
            }
        }
    }
}
