//! Deterministic, bounded lowering of a validated [`GrammarSpec`] into a
//! dense runtime transition table, plus [`RtnPda`] — the data-driven
//! automaton driver that plays the same role as `grammar::pda::Pda`, but over
//! a spec-declared state/frame alphabet instead of a hand-written one.
//!
//! Lowering here is table-building and validation, never grammar
//! interpretation: [`CompiledAutomaton::compile`] resolves every name to a
//! dense index once, up front, so the hot per-step path
//! ([`CompiledAutomaton::step`]) never touches a string.

use std::collections::BTreeMap;

use super::spec::{
    Action, ByteTest, GrammarSpec, GrammarSpecV1, Guard, MAX_FRAMES, MAX_RULES_PER_STATE,
    MAX_STATES, MAX_TOTAL_RULES, SpecError,
};

/// The outcome of feeding one byte to [`CompiledAutomaton::step`] — the
/// runtime analogue of `grammar::pda::Step`, generic over the compiled
/// automaton's own dense state/frame ids.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Stay within the current frame; move to the given state.
    Next(u32),
    /// Open a new delimiter: push the frame, move to the given state.
    Push(u32, u32),
    /// Close the current delimiter: pop the stack, move to the given state.
    Pop(u32),
    /// No valid continuation: the byte is rejected.
    Dead,
}

#[derive(Debug, Clone, Copy)]
enum CompiledGuard {
    Always,
    StackTopIs(u32),
    StackTopIsNot(u32),
    StackNonEmpty,
    StackEmpty,
}

impl CompiledGuard {
    fn matches(self, stack_top: Option<u32>) -> bool {
        match self {
            CompiledGuard::Always => true,
            CompiledGuard::StackTopIs(frame) => stack_top == Some(frame),
            CompiledGuard::StackTopIsNot(frame) => stack_top.is_some_and(|top| top != frame),
            CompiledGuard::StackNonEmpty => stack_top.is_some(),
            CompiledGuard::StackEmpty => stack_top.is_none(),
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum CompiledAction {
    Next(u32),
    Push(u32, u32),
    Pop(u32),
    Goto(u32),
    Dead,
}

#[derive(Debug, Clone)]
struct CompiledRule {
    byte_test: ByteTest,
    guard: CompiledGuard,
    action: CompiledAction,
}

#[derive(Debug, Clone)]
struct CompiledState {
    accepting: bool,
    rules: Vec<CompiledRule>,
}

/// A validated, dense, runtime-ready automaton table compiled from a
/// [`GrammarSpec`]. Immutable once built; cheap to share behind a shared
/// reference across every [`RtnPda`] instance it drives.
#[derive(Debug, Clone)]
pub struct CompiledAutomaton {
    states: Vec<CompiledState>,
    start: u32,
    frame_count: u32,
    boundary_byte: u8,
}

impl CompiledAutomaton {
    /// The automaton's declared start state.
    #[must_use]
    pub fn start(&self) -> u32 {
        self.start
    }

    /// The number of states this automaton declares — the bound a
    /// per-state cache (mirroring `grammar::compiled::CompiledGrammar`'s
    /// mask cache) must size itself to.
    #[must_use]
    pub fn state_count(&self) -> usize {
        self.states.len()
    }

    /// The pure transition function: `(state, stack_top, byte) -> Step`.
    ///
    /// Resolves any `Goto` chain internally (bounded by `state_count()`,
    /// since [`CompiledAutomaton::compile`] rejects a spec whose `Goto`
    /// edges cycle), so a caller only ever observes `Next`/`Push`/`Pop`/`Dead`.
    #[must_use]
    pub fn step(&self, state: u32, stack_top: Option<u32>, byte: u8) -> Step {
        let mut state = state;
        // Bounded by `state_count()`: `compile` proves the `Goto` graph is
        // acyclic, so a chain can revisit each state at most once.
        for _ in 0..=self.states.len() {
            let Some(compiled) = self.states.get(state as usize) else {
                return Step::Dead;
            };
            let selected = compiled
                .rules
                .iter()
                .find(|rule| rule.byte_test.matches(byte) && rule.guard.matches(stack_top));
            match selected.map(|rule| rule.action) {
                Some(CompiledAction::Next(next)) => return Step::Next(next),
                Some(CompiledAction::Push(frame, next)) => return Step::Push(frame, next),
                Some(CompiledAction::Pop(next)) => {
                    if stack_top.is_none() {
                        // The guard that selected a `Pop` action already
                        // proved the stack non-empty (`UnguardedPop`); this
                        // is defense in depth against a future validation
                        // gap, not a reachable path today.
                        return Step::Dead;
                    }
                    return Step::Pop(next);
                }
                Some(CompiledAction::Goto(next)) => state = next,
                Some(CompiledAction::Dead) | None => return Step::Dead,
            }
        }
        Step::Dead
    }

    /// Whether `state`, reached with an empty stack, is a complete query.
    ///
    /// True if `state` is itself marked `accepting`, or if feeding
    /// `boundary_byte` from `state` (empty stack) resolves — via `Next`
    /// only — to a state that is marked `accepting`. The second clause is
    /// what lets a mid-token state (an identifier body, an open number)
    /// complete without carrying its own `accepting` flag, mirroring the
    /// hand-written PDA's derivation of terminality from `step` itself.
    #[must_use]
    pub fn is_accepting_state(&self, state: u32) -> bool {
        let Some(compiled) = self.states.get(state as usize) else {
            return false;
        };
        if compiled.accepting {
            return true;
        }
        matches!(
            self.step(state, None, self.boundary_byte),
            Step::Next(landed) if self.states.get(landed as usize).is_some_and(|s| s.accepting)
        )
    }

    /// Deterministically compile `spec` into a dense, validated automaton.
    ///
    /// # Errors
    /// Returns [`SpecError`] for any malformed, unsupported, ambiguous, or
    /// explosive spec — see the individual [`SpecError`] variants. No
    /// [`RtnPda`] can be built from a spec this rejects.
    pub fn compile(spec: &GrammarSpec) -> Result<Self, SpecError> {
        let GrammarSpec::V1(v1) = spec;
        Self::compile_v1(v1)
    }

    fn compile_v1(v1: &GrammarSpecV1) -> Result<Self, SpecError> {
        if v1.frames.len() > MAX_FRAMES {
            return Err(SpecError::TooManyFrames {
                count: v1.frames.len(),
                max: MAX_FRAMES,
            });
        }
        let mut frame_ids = BTreeMap::new();
        for (index, frame) in v1.frames.iter().enumerate() {
            if frame_ids.insert(frame.as_str(), index as u32).is_some() {
                return Err(SpecError::DuplicateFrame {
                    frame: frame.clone(),
                });
            }
        }

        if v1.states.len() > MAX_STATES {
            return Err(SpecError::TooManyStates {
                count: v1.states.len(),
                max: MAX_STATES,
            });
        }
        let state_ids: BTreeMap<&str, u32> = v1
            .states
            .keys()
            .enumerate()
            .map(|(index, name)| (name.as_str(), index as u32))
            .collect();

        let start =
            *state_ids
                .get(v1.start.as_str())
                .ok_or_else(|| SpecError::UnknownStartState {
                    start: v1.start.clone(),
                })?;

        let total_rules: usize = v1.states.values().map(|s| s.rules.len()).sum();
        if total_rules > MAX_TOTAL_RULES {
            return Err(SpecError::TooManyTotalRules {
                count: total_rules,
                max: MAX_TOTAL_RULES,
            });
        }

        let mut states = Vec::with_capacity(v1.states.len());
        for (name, state_spec) in &v1.states {
            if state_spec.rules.len() > MAX_RULES_PER_STATE {
                return Err(SpecError::TooManyRules {
                    state: name.clone(),
                    count: state_spec.rules.len(),
                    max: MAX_RULES_PER_STATE,
                });
            }

            let mut rules = Vec::with_capacity(state_spec.rules.len());
            for (rule_index, rule) in state_spec.rules.iter().enumerate() {
                let guard = compile_guard(name, rule_index, &rule.guard, &frame_ids)?;
                let action = compile_action(
                    name,
                    rule_index,
                    &rule.action,
                    &state_ids,
                    &frame_ids,
                    guard,
                )?;
                check_shadowing(name, rule_index, &rule.byte_test, guard, &rules)?;
                rules.push(CompiledRule {
                    byte_test: rule.byte_test.clone(),
                    guard,
                    action,
                });
            }

            states.push((
                state_ids[name.as_str()],
                CompiledState {
                    accepting: state_spec.accepting,
                    rules,
                },
            ));
        }
        states.sort_by_key(|(id, _)| *id);
        let states = states.into_iter().map(|(_, state)| state).collect();

        let automaton = CompiledAutomaton {
            states,
            start,
            frame_count: frame_ids.len() as u32,
            boundary_byte: v1.boundary_byte,
        };

        check_goto_acyclic(v1, &state_ids)?;
        check_reachable_accept(&automaton, &v1.start)?;

        Ok(automaton)
    }
}

fn compile_guard(
    state: &str,
    rule_index: usize,
    guard: &Guard,
    frame_ids: &BTreeMap<&str, u32>,
) -> Result<CompiledGuard, SpecError> {
    let resolve = |frame: &str| {
        frame_ids
            .get(frame)
            .copied()
            .ok_or_else(|| SpecError::UnknownFrame {
                state: state.to_string(),
                rule_index,
                frame: frame.to_string(),
            })
    };
    Ok(match guard {
        Guard::Always => CompiledGuard::Always,
        Guard::StackTopIs { frame } => CompiledGuard::StackTopIs(resolve(frame)?),
        Guard::StackTopIsNot { frame } => CompiledGuard::StackTopIsNot(resolve(frame)?),
        Guard::StackNonEmpty => CompiledGuard::StackNonEmpty,
        Guard::StackEmpty => CompiledGuard::StackEmpty,
    })
}

fn compile_action(
    state: &str,
    rule_index: usize,
    action: &Action,
    state_ids: &BTreeMap<&str, u32>,
    frame_ids: &BTreeMap<&str, u32>,
    guard: CompiledGuard,
) -> Result<CompiledAction, SpecError> {
    let resolve_state = |target: &str| {
        state_ids
            .get(target)
            .copied()
            .ok_or_else(|| SpecError::UnknownTargetState {
                state: state.to_string(),
                rule_index,
                target: target.to_string(),
            })
    };
    let resolve_frame = |frame: &str| {
        frame_ids
            .get(frame)
            .copied()
            .ok_or_else(|| SpecError::UnknownFrame {
                state: state.to_string(),
                rule_index,
                frame: frame.to_string(),
            })
    };
    match action {
        Action::Next { state: target } => Ok(CompiledAction::Next(resolve_state(target)?)),
        Action::Push {
            frame,
            state: target,
        } => Ok(CompiledAction::Push(
            resolve_frame(frame)?,
            resolve_state(target)?,
        )),
        Action::Pop { state: target } => {
            if !matches!(
                guard,
                CompiledGuard::StackTopIs(_) | CompiledGuard::StackNonEmpty
            ) {
                return Err(SpecError::UnguardedPop {
                    state: state.to_string(),
                    rule_index,
                });
            }
            Ok(CompiledAction::Pop(resolve_state(target)?))
        }
        Action::Goto { state: target } => Ok(CompiledAction::Goto(resolve_state(target)?)),
        Action::Dead => Ok(CompiledAction::Dead),
    }
}

/// Reject a rule that is either an exact duplicate of an earlier rule in the
/// same state (ambiguous — which one "wins" was never a deliberate choice)
/// or wholly shadowed by one (unreachable — it can never fire).
fn check_shadowing(
    state: &str,
    rule_index: usize,
    byte_test: &ByteTest,
    guard: CompiledGuard,
    earlier: &[CompiledRule],
) -> Result<(), SpecError> {
    for (earlier_index, earlier_rule) in earlier.iter().enumerate() {
        if !same_guard(earlier_rule.guard, guard) {
            continue;
        }
        if *byte_test == earlier_rule.byte_test {
            return Err(SpecError::AmbiguousTransition {
                state: state.to_string(),
                first_index: earlier_index,
                rule_index,
            });
        }
        if byte_test.is_subsumed_by(&earlier_rule.byte_test) {
            return Err(SpecError::UnreachableRule {
                state: state.to_string(),
                rule_index,
                shadowed_by: earlier_index,
            });
        }
    }
    Ok(())
}

fn same_guard(a: CompiledGuard, b: CompiledGuard) -> bool {
    match (a, b) {
        (CompiledGuard::Always, CompiledGuard::Always)
        | (CompiledGuard::StackNonEmpty, CompiledGuard::StackNonEmpty)
        | (CompiledGuard::StackEmpty, CompiledGuard::StackEmpty) => true,
        (CompiledGuard::StackTopIs(x), CompiledGuard::StackTopIs(y))
        | (CompiledGuard::StackTopIsNot(x), CompiledGuard::StackTopIsNot(y)) => x == y,
        _ => false,
    }
}

/// Reject a spec whose `Goto` actions can cycle back to a state without ever
/// consuming a byte — such a chain could not terminate at runtime.
fn check_goto_acyclic(
    v1: &GrammarSpecV1,
    state_ids: &BTreeMap<&str, u32>,
) -> Result<(), SpecError> {
    let mut edges: Vec<Vec<u32>> = vec![Vec::new(); state_ids.len()];
    let mut owning_rule: BTreeMap<(u32, u32), (String, usize)> = BTreeMap::new();
    for (name, state_spec) in &v1.states {
        let from = state_ids[name.as_str()];
        for (rule_index, rule) in state_spec.rules.iter().enumerate() {
            if let Action::Goto { state: target } = &rule.action {
                let Some(&to) = state_ids.get(target.as_str()) else {
                    continue; // reported by `compile_action`'s own resolution pass
                };
                edges[from as usize].push(to);
                owning_rule
                    .entry((from, to))
                    .or_insert_with(|| (name.clone(), rule_index));
            }
        }
    }

    #[derive(Clone, Copy, PartialEq)]
    enum Mark {
        Unvisited,
        InProgress,
        Done,
    }
    let mut marks = vec![Mark::Unvisited; state_ids.len()];
    for start in 0..state_ids.len() as u32 {
        if marks[start as usize] != Mark::Unvisited {
            continue;
        }
        let mut stack = vec![(start, 0usize)];
        marks[start as usize] = Mark::InProgress;
        while let Some((node, next_edge)) = stack.pop() {
            if next_edge >= edges[node as usize].len() {
                marks[node as usize] = Mark::Done;
                continue;
            }
            stack.push((node, next_edge + 1));
            let neighbor = edges[node as usize][next_edge];
            match marks[neighbor as usize] {
                Mark::InProgress => {
                    let (state, rule_index) = owning_rule[&(node, neighbor)].clone();
                    return Err(SpecError::CyclicGoto { state, rule_index });
                }
                Mark::Unvisited => {
                    marks[neighbor as usize] = Mark::InProgress;
                    stack.push((neighbor, 0));
                }
                Mark::Done => {}
            }
        }
    }
    Ok(())
}

/// Reject a spec with no accepting state reachable from `start` at all — a
/// loose, guard-blind reachability check (every action edge is followed
/// regardless of guard), so it only ever catches a grammar that rejects
/// every input outright.
fn check_reachable_accept(
    automaton: &CompiledAutomaton,
    start_name: &str,
) -> Result<(), SpecError> {
    let mut seen = vec![false; automaton.states.len()];
    let mut stack = vec![automaton.start];
    seen[automaton.start as usize] = true;
    while let Some(state) = stack.pop() {
        if automaton.is_accepting_state(state) {
            return Ok(());
        }
        let Some(compiled) = automaton.states.get(state as usize) else {
            continue;
        };
        for rule in &compiled.rules {
            let next = match rule.action {
                CompiledAction::Next(s) | CompiledAction::Push(_, s) | CompiledAction::Pop(s) => {
                    Some(s)
                }
                CompiledAction::Goto(s) => Some(s),
                CompiledAction::Dead => None,
            };
            if let Some(next) = next
                && !seen[next as usize]
            {
                seen[next as usize] = true;
                stack.push(next);
            }
        }
    }
    Err(SpecError::NoReachableAccept {
        start: start_name.to_string(),
    })
}

/// A pushdown automaton instance driven by a [`CompiledAutomaton`] table —
/// the data-driven analogue of `grammar::pda::Pda`.
#[derive(Debug, Clone)]
pub struct RtnPda<'g> {
    automaton: &'g CompiledAutomaton,
    state: u32,
    stack: Vec<u32>,
}

/// The outcome of an [`RtnPda::probe`]: whether a candidate token's bytes
/// keep the automaton alive, and whether deciding that consulted the ambient
/// stack — the runtime analogue of `grammar::pda::Probe`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RtnProbe {
    /// Whether every byte was accepted (the automaton never died).
    pub alive: bool,
    /// Whether the verdict depended on the ambient (pre-existing) stack.
    pub consulted_ambient: bool,
}

impl<'g> RtnPda<'g> {
    /// A fresh automaton positioned at `automaton`'s start state with an
    /// empty stack.
    #[must_use]
    pub fn new(automaton: &'g CompiledAutomaton) -> Self {
        Self {
            automaton,
            state: automaton.start(),
            stack: Vec::new(),
        }
    }

    /// An automaton pinned at `state` with an **empty** stack — the base
    /// configuration a mask cache probes each candidate token from.
    #[must_use]
    pub fn at(automaton: &'g CompiledAutomaton, state: u32) -> Self {
        Self {
            automaton,
            state,
            stack: Vec::new(),
        }
    }

    /// The current state id.
    #[must_use]
    pub fn state(&self) -> u32 {
        self.state
    }

    /// Feed one `byte`, advancing the state and stack. Returns `false` — and
    /// leaves the automaton unchanged — iff `byte` has no valid continuation.
    #[must_use = "an unhandled dead-state return leaves the caller unaware the byte was rejected"]
    pub fn advance(&mut self, byte: u8) -> bool {
        let top = self.stack.last().copied();
        match self.automaton.step(self.state, top, byte) {
            Step::Next(next) => {
                self.state = next;
                true
            }
            Step::Push(frame, next) => {
                self.stack.push(frame);
                self.state = next;
                true
            }
            Step::Pop(next) => {
                self.stack.pop();
                self.state = next;
                true
            }
            Step::Dead => false,
        }
    }

    /// Whether the stream so far is a complete query: the stack is empty and
    /// the current state is marked accepting.
    #[must_use]
    pub fn is_accepting(&self) -> bool {
        self.stack.is_empty() && self.automaton.is_accepting_state(self.state)
    }

    /// Reset to the initial configuration, retaining the stack's allocation.
    pub fn reset(&mut self) {
        self.state = self.automaton.start();
        self.stack.clear();
    }

    /// Whether replaying `bytes` from the live configuration keeps the
    /// automaton alive, reusing `scratch` as the throwaway stack.
    #[must_use]
    pub fn admits(&self, bytes: &[u8], scratch: &mut Vec<u32>) -> bool {
        scratch.clear();
        scratch.extend_from_slice(&self.stack);
        let mut state = self.state;
        for &byte in bytes {
            let top = scratch.last().copied();
            match self.automaton.step(state, top, byte) {
                Step::Next(next) => state = next,
                Step::Push(frame, next) => {
                    scratch.push(frame);
                    state = next;
                }
                Step::Pop(next) => {
                    scratch.pop();
                    state = next;
                }
                Step::Dead => return false,
            }
        }
        true
    }

    /// Replay `bytes` over [`CompiledAutomaton::step`] without touching the
    /// live automaton, also classifying whether the verdict consulted the
    /// ambient stack.
    #[must_use]
    pub fn probe(&self, bytes: &[u8], scratch: &mut Vec<u32>) -> RtnProbe {
        scratch.clear();
        scratch.extend_from_slice(&self.stack);
        let mut state = self.state;
        for &byte in bytes {
            let top = scratch.last().copied();
            match self.automaton.step(state, top, byte) {
                Step::Next(next) => state = next,
                Step::Push(frame, next) => {
                    scratch.push(frame);
                    state = next;
                }
                Step::Pop(next) => {
                    scratch.pop();
                    state = next;
                }
                Step::Dead => {
                    let consulted_ambient = scratch.is_empty()
                        && (0..self.automaton.frame_count).any(|f| {
                            !matches!(self.automaton.step(state, Some(f), byte), Step::Dead)
                        });
                    return RtnProbe {
                        alive: false,
                        consulted_ambient,
                    };
                }
            }
        }
        RtnProbe {
            alive: true,
            consulted_ambient: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grammar::spec::GrammarSpec;

    /// Accepts exactly the literal "ok"; nothing else.
    const LITERAL_OK_SPEC: &str = r#"{
        "version": "1",
        "start": "start",
        "frames": [],
        "states": {
            "start": { "rules": [
                { "match": { "kind": "exact", "byte": 111 }, "action": { "kind": "next", "state": "saw_o" } }
            ] },
            "saw_o": { "rules": [
                { "match": { "kind": "exact", "byte": 107 }, "action": { "kind": "next", "state": "done" } }
            ] },
            "done": { "accepting": true, "rules": [] }
        }
    }"#;

    fn compile(text: &str) -> CompiledAutomaton {
        let spec = GrammarSpec::parse(text).expect("valid JSON spec");
        CompiledAutomaton::compile(&spec).expect("spec compiles")
    }

    fn accepts(automaton: &CompiledAutomaton, input: &[u8]) -> bool {
        let mut pda = RtnPda::new(automaton);
        for &byte in input {
            if !pda.advance(byte) {
                return false;
            }
        }
        pda.is_accepting()
    }

    #[test]
    fn accepts_the_literal_it_was_compiled_for() {
        let automaton = compile(LITERAL_OK_SPEC);
        assert!(accepts(&automaton, b"ok"));
    }

    #[test]
    fn rejects_a_non_matching_literal() {
        let automaton = compile(LITERAL_OK_SPEC);
        assert!(!accepts(&automaton, b"no"));
        assert!(!accepts(&automaton, b"okx"));
        assert!(!accepts(&automaton, b"o"));
    }

    #[test]
    fn editing_the_spec_changes_the_accepted_language() {
        // Same shape, different literal — proves the compiler actually reads
        // the spec rather than always producing one fixed automaton.
        let edited = LITERAL_OK_SPEC.replace("107", "112"); // 'k' (107) -> 'p' (112)
        let automaton = compile(&edited);
        assert!(accepts(&automaton, b"op"));
        assert!(!accepts(&automaton, b"ok"));
    }

    #[test]
    fn a_spec_with_balanced_parens_via_push_pop() {
        let text = r#"{
            "version": "1",
            "start": "value",
            "frames": ["paren"],
            "states": {
                "value": { "rules": [
                    { "match": { "kind": "exact", "byte": 40 }, "action": { "kind": "push", "frame": "paren", "state": "value" } },
                    { "match": { "kind": "ident_start" }, "action": { "kind": "next", "state": "after" } }
                ] },
                "after": {
                    "accepting": true,
                    "rules": [
                        { "match": { "kind": "exact", "byte": 41 }, "guard": { "kind": "stack_top_is", "frame": "paren" }, "action": { "kind": "pop", "state": "after" } }
                    ]
                }
            }
        }"#;
        let automaton = compile(text);
        assert!(accepts(&automaton, b"x"));
        assert!(accepts(&automaton, b"(x)"));
        assert!(accepts(&automaton, b"((x))"));
        assert!(!accepts(&automaton, b"(x"));
        assert!(!accepts(&automaton, b"x)"));
    }

    #[test]
    fn goto_delegates_the_same_byte_without_consuming_it() {
        // `in_ident` falls through to `after` for any non-ident-tail byte,
        // exactly like `pda::step_in_ident`'s delegation to `AfterValue`.
        // `in_ident` carries no `accepting` flag of its own — completion is
        // entirely derived from where the boundary byte resolves, mirroring
        // `Pda::is_accepting`'s space-probe trick.
        let text = r#"{
            "version": "1",
            "start": "in_ident",
            "frames": [],
            "states": {
                "in_ident": { "rules": [
                    { "match": { "kind": "ident_tail" }, "action": { "kind": "next", "state": "in_ident" } },
                    { "match": { "kind": "any" }, "action": { "kind": "goto", "state": "after" } }
                ] },
                "after": {
                    "accepting": true,
                    "rules": [
                        { "match": { "kind": "whitespace" }, "action": { "kind": "next", "state": "after" } },
                        { "match": { "kind": "exact", "byte": 46 }, "action": { "kind": "next", "state": "after_dot" } }
                    ]
                },
                "after_dot": { "rules": [
                    { "match": { "kind": "ident_start" }, "action": { "kind": "next", "state": "in_ident" } },
                    { "match": { "kind": "whitespace" }, "action": { "kind": "next", "state": "after_dot" } }
                ] }
            }
        }"#;
        let automaton = compile(text);
        assert!(accepts(&automaton, b"abc"));
        assert!(accepts(&automaton, b"abc.def"));
        // A trailing dot lands in `after_dot`, which never resolves back to
        // an `accepting` state — an identifier must follow the dot.
        assert!(!accepts(&automaton, b"abc."));
    }

    #[test]
    fn an_invalid_spec_fails_before_any_decoder_session_could_start() {
        let text = r#"{
            "version": "1",
            "start": "nowhere",
            "frames": [],
            "states": {}
        }"#;
        let spec = GrammarSpec::parse(text).expect("valid JSON spec");
        let error = CompiledAutomaton::compile(&spec).expect_err("start state is undeclared");
        assert!(matches!(error, SpecError::UnknownStartState { start } if start == "nowhere"));
    }

    #[test]
    fn rejects_an_unknown_target_state() {
        let text = r#"{
            "version": "1",
            "start": "start",
            "frames": [],
            "states": {
                "start": { "accepting": true, "rules": [
                    { "match": { "kind": "any" }, "action": { "kind": "next", "state": "ghost" } }
                ] }
            }
        }"#;
        let spec = GrammarSpec::parse(text).expect("valid JSON spec");
        let error = CompiledAutomaton::compile(&spec).expect_err("target state is undeclared");
        assert!(matches!(error, SpecError::UnknownTargetState { target, .. } if target == "ghost"));
    }

    #[test]
    fn rejects_an_unknown_frame() {
        let text = r#"{
            "version": "1",
            "start": "start",
            "frames": [],
            "states": {
                "start": { "accepting": true, "rules": [
                    { "match": { "kind": "any" }, "guard": { "kind": "stack_top_is", "frame": "ghost" }, "action": { "kind": "next", "state": "start" } }
                ] }
            }
        }"#;
        let spec = GrammarSpec::parse(text).expect("valid JSON spec");
        let error = CompiledAutomaton::compile(&spec).expect_err("frame is undeclared");
        assert!(matches!(error, SpecError::UnknownFrame { frame, .. } if frame == "ghost"));
    }

    #[test]
    fn rejects_an_ambiguous_duplicate_rule() {
        let text = r#"{
            "version": "1",
            "start": "start",
            "frames": [],
            "states": {
                "start": { "accepting": true, "rules": [
                    { "match": { "kind": "exact", "byte": 97 }, "action": { "kind": "next", "state": "start" } },
                    { "match": { "kind": "exact", "byte": 97 }, "action": { "kind": "dead" } }
                ] }
            }
        }"#;
        let spec = GrammarSpec::parse(text).expect("valid JSON spec");
        let error = CompiledAutomaton::compile(&spec).expect_err("duplicate (match, guard) pair");
        assert!(matches!(error, SpecError::AmbiguousTransition { .. }));
    }

    #[test]
    fn rejects_an_unreachable_rule_shadowed_by_any() {
        let text = r#"{
            "version": "1",
            "start": "start",
            "frames": [],
            "states": {
                "start": { "accepting": true, "rules": [
                    { "match": { "kind": "any" }, "action": { "kind": "next", "state": "start" } },
                    { "match": { "kind": "exact", "byte": 97 }, "action": { "kind": "dead" } }
                ] }
            }
        }"#;
        let spec = GrammarSpec::parse(text).expect("valid JSON spec");
        let error = CompiledAutomaton::compile(&spec).expect_err("second rule can never fire");
        assert!(matches!(error, SpecError::UnreachableRule { .. }));
    }

    #[test]
    fn rejects_a_pop_without_a_stack_guard() {
        let text = r#"{
            "version": "1",
            "start": "start",
            "frames": ["paren"],
            "states": {
                "start": { "accepting": true, "rules": [
                    { "match": { "kind": "any" }, "action": { "kind": "pop", "state": "start" } }
                ] }
            }
        }"#;
        let spec = GrammarSpec::parse(text).expect("valid JSON spec");
        let error = CompiledAutomaton::compile(&spec).expect_err("pop needs a stack guard");
        assert!(matches!(error, SpecError::UnguardedPop { .. }));
    }

    #[test]
    fn rejects_a_goto_cycle() {
        let text = r#"{
            "version": "1",
            "start": "a",
            "frames": [],
            "states": {
                "a": { "rules": [ { "match": { "kind": "any" }, "action": { "kind": "goto", "state": "b" } } ] },
                "b": { "rules": [ { "match": { "kind": "any" }, "action": { "kind": "goto", "state": "a" } } ] }
            }
        }"#;
        let spec = GrammarSpec::parse(text).expect("valid JSON spec");
        let error =
            CompiledAutomaton::compile(&spec).expect_err("goto cycles without consuming a byte");
        assert!(matches!(error, SpecError::CyclicGoto { .. }));
    }

    #[test]
    fn rejects_a_spec_with_no_reachable_accept() {
        let text = r#"{
            "version": "1",
            "start": "start",
            "frames": [],
            "states": {
                "start": { "rules": [
                    { "match": { "kind": "any" }, "action": { "kind": "dead" } }
                ] }
            }
        }"#;
        let spec = GrammarSpec::parse(text).expect("valid JSON spec");
        let error = CompiledAutomaton::compile(&spec).expect_err("no accepting state is reachable");
        assert!(matches!(error, SpecError::NoReachableAccept { .. }));
    }

    #[test]
    fn rejects_a_duplicate_frame_name() {
        let text = r#"{
            "version": "1",
            "start": "start",
            "frames": ["paren", "paren"],
            "states": {
                "start": { "accepting": true, "rules": [] }
            }
        }"#;
        let spec = GrammarSpec::parse(text).expect("valid JSON spec");
        let error = CompiledAutomaton::compile(&spec).expect_err("duplicate frame name");
        assert!(matches!(error, SpecError::DuplicateFrame { .. }));
    }

    #[test]
    fn malformed_json_reports_a_typed_span_aware_error() {
        let error = GrammarSpec::parse("{ not json").expect_err("malformed JSON");
        assert!(matches!(error, SpecError::Malformed { .. }));
    }

    #[test]
    fn probe_flags_context_dependence_for_a_bare_closer() {
        let text = r#"{
            "version": "1",
            "start": "value",
            "frames": ["paren"],
            "states": {
                "value": { "rules": [
                    { "match": { "kind": "exact", "byte": 40 }, "action": { "kind": "push", "frame": "paren", "state": "value" } },
                    { "match": { "kind": "ident_start" }, "action": { "kind": "next", "state": "after" } }
                ] },
                "after": {
                    "accepting": true,
                    "rules": [
                        { "match": { "kind": "exact", "byte": 41 }, "guard": { "kind": "stack_top_is", "frame": "paren" }, "action": { "kind": "pop", "state": "after" } }
                    ]
                }
            }
        }"#;
        let automaton = compile(text);
        let mut live = RtnPda::new(&automaton);
        assert!(live.advance(b'x')); // "value" -> "after", the state a ')' rule lives in
        let after = live.state();

        let mut scratch = Vec::new();
        // A bare `)` dies against an empty scratch stack, but would survive
        // under a `paren` frame — exactly the deferred/context-dependent case.
        let probe = RtnPda::at(&automaton, after).probe(b")", &mut scratch);
        assert!(!probe.alive);
        assert!(probe.consulted_ambient);
        // A `.`-free non-closer never depends on the stack: no frame could
        // make it alive at `after`, so it is unambiguously dead.
        let probe = RtnPda::at(&automaton, after).probe(b"!", &mut scratch);
        assert!(!probe.alive);
        assert!(!probe.consulted_ambient);
    }
}
