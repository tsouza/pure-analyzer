//! The shipped emitted-Pure grammar (§5), transcribed as a declarative
//! [`GrammarSpec`](super::spec::GrammarSpec) rather than hand-written `match`
//! arms.
//!
//! [`EMITTED_SUBSET_SPEC`] is a behavioral transcription of `grammar::pda`'s
//! automaton: every [`State`](super::pda::State) variant becomes one named spec
//! state, every [`Frame`](super::pda::Frame) variant one declared frame, and
//! every `step_*` function's `match` arms become that state's ordered
//! [`TransitionRule`](super::spec::TransitionRule)s, ready to compile with
//! [`CompiledAutomaton::compile`](super::compile::CompiledAutomaton::compile) or
//! lower end-to-end with
//! [`CompiledGrammar::from_spec`](super::compiled::CompiledGrammar::from_spec).
//! `tests/spec_equivalence.rs` proves the two automata agree on every gold,
//! precision-reject, and modern-dialect corpus entry.
//!
//! This is proof-of-equivalence data, not (yet) the production path:
//! [`CompiledGrammar::compile`](super::compiled::CompiledGrammar::compile)
//! still builds the hand-written `grammar::pda` automaton directly (issue #57).

/// The shipped emitted-subset grammar spec (JSON, schema version `"1"`).
pub const EMITTED_SUBSET_SPEC: &str = include_str!("emitted_subset.json");
