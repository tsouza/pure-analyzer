# Spec: pda-bounded-reachability

- Status: complete
- Created: 2026-08-26
- Owner: agent (autonomous PureCARD hardening)

## Problem

PureCARD registers every byte-PDA state and frame kind and has extensive corpus,
property, and fuzz coverage, but it has no single structural regression that
demonstrates a path from `State::Start` to every concrete state configuration or
detects a reached state-and-stack configuration with no bounded live
continuation. A mis-routed transition can therefore orphan a state without a
focused failure naming it.

Standalone PureCARD PR
[#42](https://github.com/tsouza/purecard/pull/42) contained a reusable bounded
exploration idea alongside an invalid source-dot rule. That PR was correctly
closed: Legend accepts `|X.'name'`, so the migrated grammar's shared
`State::AfterDot` behavior is sound. Issue
[#16](https://github.com/tsouza/pure-analyzer/issues/16) tracks adapting only
the general state-and-stack exploration.

## Goals

- [x] A bounded breadth-first exploration records a witness path from
      `State::Start` to every entry in `ALL_STATES`, including both
      `State::InStrLit` payloads.
- [x] Every frame kind in `ALL_FRAMES` is exercised by an in-bound push, and a
      pushed frame missing from that registry fails explicitly.
- [x] Every reached `(State, stack)` configuration has at least one non-dead
      successor that stays within the explored stack-depth bound.
- [x] The existing engine-verified source-dot regression remains green:
      `|X.'name'` is accepted, while `|X.5` and `|X.-y` die.

## Non-goals

- No PDA transition, grammar behavior, production API, or dependency changes.
- No `AfterSourceDot` state, source-lane admit-set transcription, or assertion
  that source and value dots differ; that was the disproven part of standalone
  PR #42.
- No claim of formal or unbounded reachability, grammar completeness, or Legend
  differential validation.

## Design

The change is test-only in `pure-analyzer-purecard`, an independent product
outside the analyzer crate DAG. A breadth-first search explores configurations
of `(State, Vec<frame-index>)` from `Start` with an empty stack. Frame indices
refer to `ALL_FRAMES`, avoiding a production `Hash` implementation solely for a
test. `Step::Next`, `Push`, and `Pop` update the bounded configuration exactly as
the live `Pda` driver does.

`MAX_STACK_DEPTH` is three. With 47 concrete states and four frame kinds, that
bounds the theoretical graph to
`47 * (1 + 4 + 16 + 64) = 3,995` configurations while still exercising nested
stacks. This is an exact search only inside that bound: push edges at the cap are
omitted, so a missing witness can mean either a real orphan or a path requiring
deeper nesting. Raising the cap is an explicit review decision, not evidence of
an unbounded proof.

Because `step` depends on only `(state, stack_top, byte)`, transition rows are
computed once for all 47 states, five possible tops, and 256 bytes. Identical
`Step` outcomes within a row are collapsed while retaining one byte for witness
reconstruction. This preserves bounded reachability and black-hole facts without
re-running the full byte table for every lower-stack combination.

## API / contract impact

None. The spec and `#[cfg(test)]` code are the only changes. PureCARD remains
unpublished and independent from analyzer crates.

## Testing plan

- Failing-first: add the structural assertion against an explorer that visits
  only the seed; `just test-unit` must fail by naming missing states and frames.
- Unit: bounded exploration reconstructs and replays one witness per state,
  observes every registered frame, and reports no bounded black-hole config.
- Existing `source_dots_admit_quoted_member_names` remains unchanged and green.
- Prove the new gate non-vacuous with temporary, reverted transition
  perturbations that orphan `SawTilde` and make a reached transient state dead;
  never use the invalid source-dot assumption as the mutation.
- Run `just fmt`, `just lint`, `just test-unit`, and `just ci`; mutation and the
  remaining full gates run before the eventual PR.

## Verification

- `just ci` passes on the final branch: all 397 Nextest cases and the
  all-features doctest lane are green with no skipped tests.
- `just test-mutation` classifies all 545 default-workspace mutants as 447
  caught and 98 unviable, with zero missed or timed-out mutants. Its separate
  FFI pass classifies all 23 mutants as seven caught and 16 unviable, again with
  zero misses or timeouts.
- The new structural test independently kills cargo-mutants perturbations that
  delete the transition into `SawTilde` and that leave reached
  `InMultiplicity` configurations without any in-bound live successor. The
  diagnostics name the orphan state and every black-hole stack witness.
- `git diff --check` passes, and the branch descends from current `origin/main`.

## Risks & rollout

- **Bounded-search interpretation:** truncation can omit a legitimate deeper
  witness. Diagnostics name the missing state and cap, and the spec makes the
  limitation explicit.
- **Test cost:** the capped graph and compressed transition rows keep the test
  small enough for repeated mutation runs.
- **False confidence:** this proves only bounded structural liveness, not that
  accepted queries compile. Existing corpus, fuzz, and live Legend lanes retain
  their distinct responsibilities.
- Rollback is a test/spec revert; shipped behavior is untouched.
