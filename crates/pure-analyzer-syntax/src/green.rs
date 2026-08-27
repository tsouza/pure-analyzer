use std::{fmt, slice, sync::Arc};

use crate::{SyntaxKind, TextRange};

/// An immutable terminal token in a concrete syntax tree.
#[derive(Clone)]
pub struct GreenToken {
    kind: SyntaxKind,
    source: Arc<str>,
    range: TextRange,
}

impl PartialEq for GreenToken {
    fn eq(&self, other: &Self) -> bool {
        self.kind == other.kind && self.range == other.range && self.text() == other.text()
    }
}

impl Eq for GreenToken {}

impl fmt::Debug for GreenToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GreenToken")
            .field("kind", &self.kind)
            .field("text", &self.text())
            .field("range", &self.range)
            .finish()
    }
}

impl GreenToken {
    pub(crate) fn new(kind: SyntaxKind, source: Arc<str>, range: TextRange) -> Self {
        Self {
            kind,
            source,
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
        self.source
            .get(usize::from(self.range.start())..usize::from(self.range.end()))
            .unwrap_or_default()
    }

    /// Returns this token's byte range in the source.
    #[must_use]
    pub const fn text_range(&self) -> TextRange {
        self.range
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use pure_analyzer_lexer::lex;

    use super::GreenToken;
    use crate::{GreenNodeBuilder, SyntaxKind};

    #[test]
    fn builder_tokens_share_one_source_allocation() {
        let source = "left right";
        let tokens = lex(source);
        let mut builder = GreenNodeBuilder::new(source, &tokens);
        builder.open(SyntaxKind::ROOT);
        for _ in &tokens {
            builder.advance();
        }
        builder.close();
        let tree = builder.finish().expect("flat token tree must build");
        let tokens = tree.tokens().collect::<Vec<_>>();

        assert_eq!(tokens.len(), 3);
        let left = tokens[0];
        let right = tokens[2];

        assert!(Arc::ptr_eq(&left.source, &right.source));
        assert_eq!(left.text(), "left");
        assert_eq!(right.text(), "right");
    }

    #[test]
    fn token_equality_does_not_depend_on_unrelated_source_text() {
        fn final_token(source: &str) -> GreenToken {
            let tokens = lex(source);
            let mut builder = GreenNodeBuilder::new(source, &tokens);
            builder.open(SyntaxKind::ROOT);
            for _ in &tokens {
                builder.advance();
            }
            builder.close();
            builder
                .finish()
                .expect("flat token tree must build")
                .tokens()
                .last()
                .expect("fixture must have a final token")
                .clone()
        }

        let left = final_token("a x");
        let right = final_token("b x");

        assert!(!Arc::ptr_eq(&left.source, &right.source));
        assert_eq!(left, right);
        let debug = format!("{left:?}");
        assert_eq!(debug, format!("{right:?}"));
        assert!(debug.contains("GreenToken"));
        assert!(debug.contains("kind: IDENT"));
        assert!(debug.contains("text: \"x\""));
        assert!(debug.contains("range: 2..3"));

        let different_text = final_token("a y");
        assert_ne!(left, different_text);
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
