use std::sync::Arc;

use pure_analyzer_lexer::SyntaxKind as LexerSyntaxKind;
use thiserror::Error;

use crate::{GreenElement, GreenNode, GreenToken, SyntaxKind, TextRange, TextSize};

/// A parser action consumed by [`GreenNodeBuilder`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// Starts a node of the supplied nonterminal kind.
    Open(SyntaxKind),
    /// Consumes the next lexer token.
    Advance,
    /// Finishes the most recently opened node.
    Close,
}

/// A stable insertion point in one builder's event stream.
#[derive(Debug, Clone)]
pub struct Checkpoint {
    owner: Arc<()>,
    marker: Arc<()>,
}

/// A validation failure while constructing a concrete syntax tree.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum BuildError {
    /// The source length cannot be represented by `TextSize`.
    #[error("source length {length} exceeds the u32 text-size limit")]
    SourceTooLong {
        /// The unrepresentable source length in bytes.
        length: usize,
    },
    /// A lexer token has an empty byte range.
    #[error("token {token_index} has an empty range at {offset:?}")]
    EmptyTokenRange {
        /// The token's zero-based index.
        token_index: usize,
        /// The zero-length range's byte offset.
        offset: TextSize,
    },
    /// A lexer range does not start where its predecessor ended.
    #[error("token {token_index} starts at {actual:?}, expected {expected:?}")]
    NonContiguousTokenRange {
        /// The token's zero-based index.
        token_index: usize,
        /// The required start offset.
        expected: TextSize,
        /// The supplied start offset.
        actual: TextSize,
    },
    /// A lexer range ends beyond the supplied source.
    #[error("token {token_index} range {range:?} lies outside source ending at {source_end:?}")]
    TokenRangeOutsideSource {
        /// The token's zero-based index.
        token_index: usize,
        /// The invalid token range.
        range: TextRange,
        /// The source's byte length.
        source_end: TextSize,
    },
    /// A lexer range boundary splits a UTF-8 code point.
    #[error("token {token_index} boundary {offset:?} is not a UTF-8 boundary")]
    InvalidUtf8Boundary {
        /// The token's zero-based index.
        token_index: usize,
        /// The invalid byte offset.
        offset: TextSize,
    },
    /// The token stream does not cover the complete source.
    #[error("token stream ends at {actual:?}, expected source end {expected:?}")]
    IncompleteTokenCoverage {
        /// The source's byte length.
        expected: TextSize,
        /// The final token's end, or zero for no tokens.
        actual: TextSize,
    },
    /// An `Open` event used a terminal kind.
    #[error("event {event_index} opens terminal kind {kind:?}")]
    ExpectedNodeKind {
        /// The event's zero-based index.
        event_index: usize,
        /// The supplied terminal kind.
        kind: SyntaxKind,
    },
    /// A [`SyntaxKind::ROOT`] node was opened inside another node.
    #[error("event {event_index} opens ROOT inside another node")]
    NestedRoot {
        /// The event's zero-based index.
        event_index: usize,
    },
    /// A checkpoint belongs to a different builder.
    #[error("checkpoint belongs to a different builder")]
    ForeignCheckpoint,
    /// A builder-owned checkpoint marker is missing from the event stream.
    ///
    /// Safe callers cannot remove markers; this defensively reports an
    /// internal builder-invariant violation without panicking.
    #[error("checkpoint marker is missing from its builder")]
    InvalidCheckpoint,
    /// An `Advance` event appeared without an open node.
    #[error("event {event_index} advances outside any open node")]
    AdvanceOutsideNode {
        /// The event's zero-based index.
        event_index: usize,
    },
    /// An `Advance` event exceeded the lexer token stream.
    #[error("event {event_index} advances beyond {token_count} lexer tokens")]
    AdvancePastTokens {
        /// The event's zero-based index.
        event_index: usize,
        /// The lexer token count.
        token_count: usize,
    },
    /// A `Close` event had no matching `Open` event.
    #[error("event {event_index} closes without a matching open")]
    CloseWithoutOpen {
        /// The event's zero-based index.
        event_index: usize,
    },
    /// The event stream ended with open nodes.
    #[error("event stream ended with {count} unclosed node(s)")]
    UnclosedNodes {
        /// The number of nodes still open.
        count: usize,
    },
    /// The event stream left lexer tokens unconsumed.
    #[error("event stream consumed {consumed} of {total} lexer tokens")]
    UnconsumedTokens {
        /// The number of consumed lexer tokens.
        consumed: usize,
        /// The total number of lexer tokens.
        total: usize,
    },
    /// The event stream did not produce a node.
    #[error("event stream did not produce a root node")]
    MissingRoot,
    /// The event stream produced more than one top-level node.
    #[error("event stream produced {count} top-level nodes")]
    MultipleRoots {
        /// The number of top-level nodes.
        count: usize,
    },
    /// The sole top-level node was not [`SyntaxKind::ROOT`].
    #[error("top-level node has kind {actual:?}, expected ROOT")]
    ExpectedRootKind {
        /// The supplied top-level kind.
        actual: SyntaxKind,
    },
    /// A validated range could not be sliced from the supplied source.
    #[error("token {token_index} range {range:?} cannot be sliced from source")]
    InvalidSourceSlice {
        /// The token's zero-based index.
        token_index: usize,
        /// The range that could not be sliced.
        range: TextRange,
    },
}

/// Collects parser events and folds them into an immutable green tree.
#[derive(Debug)]
pub struct GreenNodeBuilder<'source> {
    source: &'source str,
    tokens: &'source [(LexerSyntaxKind, TextRange)],
    events: Vec<BuilderEvent>,
    owner: Arc<()>,
}

#[derive(Debug)]
enum BuilderEvent {
    Event(Event),
    Checkpoint(Arc<()>),
}

impl BuilderEvent {
    const fn event(&self) -> Option<Event> {
        match self {
            Self::Event(event) => Some(*event),
            Self::Checkpoint(_) => None,
        }
    }
}

impl<'source> GreenNodeBuilder<'source> {
    /// Creates a builder over `source` and its lexer token stream.
    #[must_use]
    pub fn new(source: &'source str, tokens: &'source [(LexerSyntaxKind, TextRange)]) -> Self {
        Self {
            source,
            tokens,
            events: Vec::new(),
            owner: Arc::new(()),
        }
    }

    /// Appends a parser event.
    pub fn push(&mut self, event: Event) {
        self.events.push(BuilderEvent::Event(event));
    }

    /// Appends an [`Event::Open`] event.
    pub fn open(&mut self, kind: SyntaxKind) {
        self.push(Event::Open(kind));
    }

    /// Appends an [`Event::Advance`] event.
    pub fn advance(&mut self) {
        self.push(Event::Advance);
    }

    /// Appends an [`Event::Close`] event.
    pub fn close(&mut self) {
        self.push(Event::Close);
    }

    /// Captures the current event-stream position for later wrapping.
    #[must_use]
    pub fn checkpoint(&mut self) -> Checkpoint {
        let checkpoint = Checkpoint {
            owner: Arc::clone(&self.owner),
            marker: Arc::new(()),
        };
        self.events
            .push(BuilderEvent::Checkpoint(Arc::clone(&checkpoint.marker)));
        checkpoint
    }

    /// Inserts an `Open` event at an earlier checkpoint.
    ///
    /// This supports Pratt-style retroactive wrapping after the parser learns
    /// that an already-consumed left-hand side begins a binary expression.
    pub fn open_at(&mut self, checkpoint: &Checkpoint, kind: SyntaxKind) -> Result<(), BuildError> {
        if !Arc::ptr_eq(&self.owner, &checkpoint.owner) {
            return Err(BuildError::ForeignCheckpoint);
        }
        let marker_index = self
            .events
            .iter()
            .position(|event| {
                matches!(
                    event,
                    BuilderEvent::Checkpoint(marker)
                        if Arc::ptr_eq(marker, &checkpoint.marker)
                )
            })
            .ok_or(BuildError::InvalidCheckpoint)?;
        let event_index = self.events[..marker_index]
            .iter()
            .filter(|event| event.event().is_some())
            .count();
        if !kind.is_node() {
            return Err(BuildError::ExpectedNodeKind { event_index, kind });
        }
        self.events
            .insert(marker_index, BuilderEvent::Event(Event::Open(kind)));
        Ok(())
    }

    /// Validates all inputs and constructs exactly one [`SyntaxKind::ROOT`].
    pub fn finish(self) -> Result<GreenNode, BuildError> {
        let source_end = validate_tokens(self.source, self.tokens)?;
        fold_events(
            self.source,
            self.tokens,
            self.events.iter().filter_map(BuilderEvent::event),
            source_end,
        )
    }
}

#[derive(Debug)]
struct PendingNode {
    kind: SyntaxKind,
    children: Vec<GreenElement>,
    start: TextSize,
}

fn source_end(source: &str) -> Result<TextSize, BuildError> {
    TextSize::try_from(source.len()).map_err(|_| BuildError::SourceTooLong {
        length: source.len(),
    })
}

fn validate_tokens(
    source: &str,
    tokens: &[(LexerSyntaxKind, TextRange)],
) -> Result<TextSize, BuildError> {
    let source_end = source_end(source)?;
    let mut expected = TextSize::from(0);

    for (token_index, (_, range)) in tokens.iter().enumerate() {
        validate_token_range(source, token_index, *range, expected, source_end)?;
        expected = range.end();
    }

    if expected != source_end {
        return Err(BuildError::IncompleteTokenCoverage {
            expected: source_end,
            actual: expected,
        });
    }
    Ok(source_end)
}

fn validate_token_range(
    source: &str,
    token_index: usize,
    range: TextRange,
    expected: TextSize,
    source_end: TextSize,
) -> Result<(), BuildError> {
    if range.start() != expected {
        return Err(BuildError::NonContiguousTokenRange {
            token_index,
            expected,
            actual: range.start(),
        });
    }
    if range.is_empty() {
        return Err(BuildError::EmptyTokenRange {
            token_index,
            offset: range.start(),
        });
    }
    if range.end() > source_end {
        return Err(BuildError::TokenRangeOutsideSource {
            token_index,
            range,
            source_end,
        });
    }
    for offset in [range.start(), range.end()] {
        if !source.is_char_boundary(usize::from(offset)) {
            return Err(BuildError::InvalidUtf8Boundary {
                token_index,
                offset,
            });
        }
    }
    Ok(())
}

fn fold_events(
    source: &str,
    tokens: &[(LexerSyntaxKind, TextRange)],
    events: impl Iterator<Item = Event>,
    source_end: TextSize,
) -> Result<GreenNode, BuildError> {
    let mut stack = Vec::<PendingNode>::new();
    let mut roots = Vec::<GreenNode>::new();
    let mut token_index = 0;
    let mut offset = TextSize::from(0);

    for (event_index, event) in events.enumerate() {
        match event {
            Event::Open(kind) => open_node(&mut stack, event_index, kind, offset)?,
            Event::Advance => {
                offset = advance_token(source, tokens, &mut stack, event_index, token_index)?;
                token_index += 1;
            }
            Event::Close => close_node(&mut stack, &mut roots, event_index, offset)?,
        }
    }

    finish_root(stack, roots, token_index, tokens.len(), source_end)
}

fn open_node(
    stack: &mut Vec<PendingNode>,
    event_index: usize,
    kind: SyntaxKind,
    offset: TextSize,
) -> Result<(), BuildError> {
    if !kind.is_node() {
        return Err(BuildError::ExpectedNodeKind { event_index, kind });
    }
    if kind == SyntaxKind::ROOT && !stack.is_empty() {
        return Err(BuildError::NestedRoot { event_index });
    }
    stack.push(PendingNode {
        kind,
        children: Vec::new(),
        start: offset,
    });
    Ok(())
}

fn advance_token(
    source: &str,
    tokens: &[(LexerSyntaxKind, TextRange)],
    stack: &mut [PendingNode],
    event_index: usize,
    token_index: usize,
) -> Result<TextSize, BuildError> {
    let parent = stack
        .last_mut()
        .ok_or(BuildError::AdvanceOutsideNode { event_index })?;
    let (kind, range) = tokens
        .get(token_index)
        .copied()
        .ok_or(BuildError::AdvancePastTokens {
            event_index,
            token_count: tokens.len(),
        })?;
    let text = source
        .get(usize::from(range.start())..usize::from(range.end()))
        .ok_or(BuildError::InvalidSourceSlice { token_index, range })?;
    parent.children.push(GreenElement::Token(GreenToken::new(
        kind.into(),
        text,
        range,
    )));
    Ok(range.end())
}

fn close_node(
    stack: &mut Vec<PendingNode>,
    roots: &mut Vec<GreenNode>,
    event_index: usize,
    offset: TextSize,
) -> Result<(), BuildError> {
    let pending = stack
        .pop()
        .ok_or(BuildError::CloseWithoutOpen { event_index })?;
    let node = GreenNode::new(
        pending.kind,
        pending.children,
        TextRange::new(pending.start, offset),
    );
    if let Some(parent) = stack.last_mut() {
        parent.children.push(GreenElement::Node(node));
    } else {
        roots.push(node);
    }
    Ok(())
}

fn finish_root(
    stack: Vec<PendingNode>,
    mut roots: Vec<GreenNode>,
    consumed: usize,
    total: usize,
    source_end: TextSize,
) -> Result<GreenNode, BuildError> {
    if !stack.is_empty() {
        return Err(BuildError::UnclosedNodes { count: stack.len() });
    }
    if consumed != total {
        return Err(BuildError::UnconsumedTokens { consumed, total });
    }
    if roots.is_empty() {
        return Err(BuildError::MissingRoot);
    }
    if roots.len() != 1 {
        return Err(BuildError::MultipleRoots { count: roots.len() });
    }
    let root = roots.pop().ok_or(BuildError::MissingRoot)?;
    if root.kind() != SyntaxKind::ROOT {
        return Err(BuildError::ExpectedRootKind {
            actual: root.kind(),
        });
    }
    if root.text_range() != TextRange::new(TextSize::from(0), source_end) {
        return Err(BuildError::IncompleteTokenCoverage {
            expected: source_end,
            actual: root.text_range().end(),
        });
    }
    Ok(root)
}
