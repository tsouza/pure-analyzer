//! A versioned, declarative grammar-spec schema for
//! [`CompiledGrammar::from_spec`](crate::grammar::compiled::CompiledGrammar::from_spec).
//!
//! A [`GrammarSpec`] describes a byte-level pushdown automaton the same way
//! `grammar/pda.rs`'s hand-written `step` function does — named states, each
//! with an ordered list of byte-guarded transitions, plus a small set of named
//! stack-frame kinds for delimiter nesting — but as `serde`-deserializable
//! data rather than Rust `match` arms. Lowering a spec is therefore
//! validation and table-building, not grammar interpretation: there is no
//! EBNF text parser here, and none is needed. That is a deliberate choice,
//! not an oversight — see `docs/decisions/0010-declarative-transition-table-spec.md`.
//!
//! Rules within a state are tried **in order**; the first whose [`ByteTest`]
//! and [`Guard`] both match wins, exactly like the hand-written `match` arms
//! it replaces. [`Action::Goto`] re-evaluates the *same* byte against another
//! state's rules without consuming input — the declarative form of the
//! hand-written PDA's "delegate to another state's arm" pattern used for
//! multi-byte operators and shared literal-closing logic.

use std::collections::BTreeMap;
use std::fmt;

use serde::Deserialize;

/// Upper bound on the number of states a single spec may declare.
pub const MAX_STATES: usize = 512;
/// Upper bound on the number of transition rules a single state may declare.
///
/// Validation checks every rule pair within a state for ambiguity/shadowing
/// (`O(rules^2)`, each comparison scanning the 256-byte domain), so this
/// bound also keeps compilation itself from becoming the explosive input —
/// with [`MAX_STATES`] and [`MAX_TOTAL_RULES`], worst-case validation stays
/// under a billion byte-comparisons.
pub const MAX_RULES_PER_STATE: usize = 64;
/// Upper bound on the total transition rules across a whole spec.
pub const MAX_TOTAL_RULES: usize = 8_192;
/// Upper bound on the number of named stack-frame kinds a spec may declare.
pub const MAX_FRAMES: usize = 64;

fn default_boundary_byte() -> u8 {
    b' '
}

/// A versioned grammar specification.
///
/// New capabilities are added under a new `version` tag, never by silently
/// widening an existing one — a consumer pinned to `"1"` must never observe a
/// behavior change from a spec it already accepted.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "version")]
pub enum GrammarSpec {
    /// Version 1: an ordered, named-state transition table.
    #[serde(rename = "1")]
    V1(GrammarSpecV1),
}

/// Version 1 of the grammar-spec schema.
#[derive(Debug, Clone, Deserialize)]
pub struct GrammarSpecV1 {
    /// The name of the state the automaton starts in.
    pub start: String,
    /// The byte a state's completion is probed with: a state that is not
    /// itself marked `accepting` is nonetheless a complete query (with an
    /// empty stack) if feeding this byte from it lands — via `Next` only,
    /// never `Push`/`Pop` — in a state that *is* marked `accepting`. This is
    /// the declarative form of the hand-written PDA's "derive terminality
    /// from `step` itself" trick: a mid-token state like an identifier body
    /// need not duplicate the hub state's `accepting` flag, only the byte
    /// that would resolve to it (typically a space). Defaults to `b' '`.
    #[serde(default = "default_boundary_byte")]
    pub boundary_byte: u8,
    /// The named stack-frame kinds this grammar's [`Action::Push`]/[`Guard`]
    /// variants may reference.
    #[serde(default)]
    pub frames: Vec<String>,
    /// Every state, keyed by name. A `BTreeMap` so a spec's textual
    /// (de)serialization is order-independent and diff-stable.
    pub states: BTreeMap<String, StateSpec>,
}

/// One automaton state: whether it accepts (with an empty stack) and its
/// ordered transition rules.
#[derive(Debug, Clone, Deserialize)]
pub struct StateSpec {
    /// Whether this state, reached with an empty stack, is a complete query.
    #[serde(default)]
    pub accepting: bool,
    /// Transition rules, tried in order; the first match wins.
    #[serde(default)]
    pub rules: Vec<TransitionRule>,
}

/// One ordered transition rule: `match` and `guard` both gate applicability;
/// `action` is what happens once a rule is selected.
#[derive(Debug, Clone, Deserialize)]
pub struct TransitionRule {
    /// The byte test this rule requires.
    #[serde(rename = "match")]
    pub byte_test: ByteTest,
    /// The stack condition this rule additionally requires.
    #[serde(default)]
    pub guard: Guard,
    /// What happens when both `byte_test` and `guard` match.
    pub action: Action,
}

/// A predicate over the current input byte.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ByteTest {
    /// A single space, tab, newline, or carriage return.
    Whitespace,
    /// An ASCII letter or `_` — legal at the start of an identifier.
    IdentStart,
    /// An ASCII letter, digit, or `_` — legal inside an identifier.
    IdentTail,
    /// An ASCII digit `0`-`9`.
    Digit,
    /// Exactly one byte value.
    Exact {
        /// The required byte.
        byte: u8,
    },
    /// Any one of the listed byte values.
    OneOf {
        /// The admitted byte values.
        bytes: Vec<u8>,
    },
    /// Any byte other than one of the listed values.
    NoneOf {
        /// The excluded byte values.
        bytes: Vec<u8>,
    },
    /// Any byte at all — the universal fallback, valid only as a state's last
    /// rule (see [`super::compile::CompileError::UnreachableRule`]).
    Any,
}

impl ByteTest {
    pub(super) fn matches(&self, byte: u8) -> bool {
        match self {
            ByteTest::Whitespace => matches!(byte, b' ' | b'\t' | b'\n' | b'\r'),
            ByteTest::IdentStart => byte.is_ascii_alphabetic() || byte == b'_',
            ByteTest::IdentTail => byte.is_ascii_alphanumeric() || byte == b'_',
            ByteTest::Digit => byte.is_ascii_digit(),
            ByteTest::Exact { byte: want } => byte == *want,
            ByteTest::OneOf { bytes } => bytes.contains(&byte),
            ByteTest::NoneOf { bytes } => !bytes.contains(&byte),
            ByteTest::Any => true,
        }
    }

    /// Whether every byte this test admits is also admitted by `other` —
    /// used to detect a rule that can never fire because an earlier rule in
    /// the same state already covers its whole domain. Conservative: a
    /// `false` result does not prove the tests are disjoint, only that this
    /// particular pair could not be proven to be a total shadow.
    pub(super) fn is_subsumed_by(&self, other: &ByteTest) -> bool {
        (0..=u8::MAX).all(|b| !self.matches(b) || other.matches(b))
    }
}

/// A predicate over the automaton's stack top.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Guard {
    /// Always applies.
    #[default]
    Always,
    /// Applies iff the stack is non-empty and its top frame is `frame`.
    StackTopIs {
        /// The required top frame kind.
        frame: String,
    },
    /// Applies iff the stack is non-empty and its top frame is not `frame`.
    StackTopIsNot {
        /// The excluded top frame kind.
        frame: String,
    },
    /// Applies iff the stack is non-empty (of any frame kind).
    StackNonEmpty,
    /// Applies iff the stack is empty.
    StackEmpty,
}

/// What a matched rule does to the automaton's state and stack.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Action {
    /// Consume the byte and move to `state`, stack unchanged.
    Next {
        /// The target state name.
        state: String,
    },
    /// Consume the byte, push `frame`, and move to `state`.
    Push {
        /// The frame kind to push.
        frame: String,
        /// The target state name.
        state: String,
    },
    /// Consume the byte, pop the stack, and move to `state`. Only reachable
    /// behind a [`Guard::StackTopIs`] rule — popping an empty or
    /// wrong-topped stack is a spec validation error, not a runtime dead
    /// state, so the two never need to be told apart at decode time.
    Pop {
        /// The target state name.
        state: String,
    },
    /// Re-evaluate the *same* byte against `state`'s rules, without
    /// consuming input or changing the stack — the declarative form of the
    /// hand-written PDA's "delegate to another state's arm" fallthrough.
    Goto {
        /// The state whose rules re-evaluate this byte.
        state: String,
    },
    /// No valid continuation: the byte is rejected.
    Dead,
}

/// A malformed, unsupported, or explosive grammar spec.
///
/// Every variant names the exact location (state name and, where
/// applicable, the rule's index within that state) so a spec author can find
/// the offending line without re-deriving it from a generic message.
#[derive(Debug, thiserror::Error)]
pub enum SpecError {
    /// The spec text is not valid JSON, or does not match the versioned
    /// schema. `serde_json`'s own line/column locate the problem.
    #[error("malformed grammar spec at line {line}, column {column}: {message}")]
    Malformed {
        /// 1-based line number of the syntax/schema error.
        line: usize,
        /// 1-based column number of the syntax/schema error.
        column: usize,
        /// `serde_json`'s description of the problem.
        message: String,
    },
    /// `start` names a state that is not declared in `states`.
    #[error("start state {start:?} is not declared")]
    UnknownStartState {
        /// The undeclared start-state name.
        start: String,
    },
    /// A rule's action names a target state that is not declared.
    #[error("state {state:?} rule {rule_index}: target state {target:?} is not declared")]
    UnknownTargetState {
        /// The state the offending rule belongs to.
        state: String,
        /// The rule's 0-based index within that state.
        rule_index: usize,
        /// The undeclared target state name.
        target: String,
    },
    /// A rule's guard or push action names a frame kind that is not declared
    /// in `frames`.
    #[error("state {state:?} rule {rule_index}: frame {frame:?} is not declared")]
    UnknownFrame {
        /// The state the offending rule belongs to.
        state: String,
        /// The rule's 0-based index within that state.
        rule_index: usize,
        /// The undeclared frame name.
        frame: String,
    },
    /// The spec declares more states than [`MAX_STATES`].
    #[error("grammar spec declares {count} states, exceeding the bound of {max}")]
    TooManyStates {
        /// The declared state count.
        count: usize,
        /// The enforced bound.
        max: usize,
    },
    /// A state declares more rules than [`MAX_RULES_PER_STATE`].
    #[error("state {state:?} declares {count} rules, exceeding the bound of {max}")]
    TooManyRules {
        /// The offending state's name.
        state: String,
        /// The declared rule count.
        count: usize,
        /// The enforced bound.
        max: usize,
    },
    /// The spec declares more transition rules in total than
    /// [`MAX_TOTAL_RULES`].
    #[error("grammar spec declares {count} total transition rules, exceeding the bound of {max}")]
    TooManyTotalRules {
        /// The declared total rule count.
        count: usize,
        /// The enforced bound.
        max: usize,
    },
    /// The spec declares more frame kinds than [`MAX_FRAMES`].
    #[error("grammar spec declares {count} frame kinds, exceeding the bound of {max}")]
    TooManyFrames {
        /// The declared frame count.
        count: usize,
        /// The enforced bound.
        max: usize,
    },
    /// `frames` lists the same frame name more than once.
    #[error("frame {frame:?} is declared more than once")]
    DuplicateFrame {
        /// The repeated frame name.
        frame: String,
    },
    /// Two rules in the same state test the identical (byte test, guard)
    /// pair — the second can never be distinguished from the first, so
    /// which one "wins" is not something a spec author chose on purpose.
    #[error(
        "state {state:?} rules {first_index} and {rule_index} are ambiguous: both test the same byte and guard"
    )]
    AmbiguousTransition {
        /// The state the offending rules belong to.
        state: String,
        /// The earlier rule's 0-based index.
        first_index: usize,
        /// The later, shadowed rule's 0-based index.
        rule_index: usize,
    },
    /// A rule can never fire because an earlier rule in the same state
    /// already matches every byte this rule's test admits.
    #[error(
        "state {state:?} rule {rule_index} is unreachable: rule {shadowed_by} already matches every byte it tests for"
    )]
    UnreachableRule {
        /// The state the offending rule belongs to.
        state: String,
        /// The unreachable rule's 0-based index.
        rule_index: usize,
        /// The earlier rule's 0-based index that shadows it.
        shadowed_by: usize,
    },
    /// A `Pop` action is reachable without a `StackTopIs`/`StackNonEmpty`
    /// guard ruling out an empty stack — popping is only ever well-defined
    /// behind a guard that already proved the stack is non-empty.
    #[error(
        "state {state:?} rule {rule_index}: a pop action requires a stack_top_is or stack_non_empty guard"
    )]
    UnguardedPop {
        /// The state the offending rule belongs to.
        state: String,
        /// The offending rule's 0-based index.
        rule_index: usize,
    },
    /// A chain of `Goto` actions revisits a state without ever consuming a
    /// byte, so it can never terminate.
    #[error(
        "state {state:?} rule {rule_index}: goto chain cycles back to {state:?} without consuming a byte"
    )]
    CyclicGoto {
        /// The state where the cycle was detected.
        state: String,
        /// The rule's 0-based index that closes the cycle.
        rule_index: usize,
    },
    /// No accepting state is reachable from `start` at all — every walk of
    /// this grammar is rejected, which is never an intentional spec.
    #[error("no accepting state is reachable from start state {start:?}")]
    NoReachableAccept {
        /// The spec's start-state name.
        start: String,
    },
}

impl From<serde_json::Error> for SpecError {
    fn from(error: serde_json::Error) -> Self {
        SpecError::Malformed {
            line: error.line(),
            column: error.column(),
            message: error.to_string(),
        }
    }
}

impl fmt::Display for GrammarSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GrammarSpec::V1(_) => write!(f, "grammar spec v1"),
        }
    }
}

impl GrammarSpec {
    /// Parse `text` as JSON into a versioned [`GrammarSpec`].
    ///
    /// # Errors
    /// Returns [`SpecError::Malformed`] if `text` is not valid JSON or does
    /// not match the versioned schema.
    pub fn parse(text: &str) -> Result<Self, SpecError> {
        Ok(serde_json::from_str(text)?)
    }
}
