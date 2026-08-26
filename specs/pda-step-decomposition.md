# Spec: pda-step-decomposition

- Status: complete
- Created: 2026-08-26
- Owner: agent (GitHub issue #6)

## Problem

PureCARD's byte-level PDA transition function dispatches all 47 automaton
states from one large nested match. Its behavior is covered by the existing
decoder, corpus, and oracle-oriented tests, but the function exceeds the
workspace's protected cognitive-complexity and line-count budgets. A temporary,
documented lint suppression made that pre-existing debt visible during the
PureCARD migration; issue #6 tracks removing it as a dedicated refactor.

## Goals

- [x] Make `grammar::pda::step` a small, exhaustive state dispatcher whose
      transition logic lives in private state-specific helpers.
- [x] Remove the `cognitive_complexity` and `too_many_lines` suppressions from
      `step` without changing any transition result.
- [x] Remove the three cognitive-complexity suppressions from the affected PDA
      and scope tests while retaining their assertions and coverage.
- [x] Keep all existing workspace tests and the full pre-merge gate green.

## Non-goals

- Changing, widening, or narrowing the emitted-Pure grammar.
- Rewriting the PDA, changing its public API, or altering state/frame layout.
- Adding the bounded reachability exploration from closed standalone PureCARD
  PR #42; that independent test idea is not required to prove this refactor.
- Changing analyzer crates or creating dependencies across the analyzer/PureCARD
  product boundary.

## Design

Keep `step(state, stack_top, byte)` as the only public transition entry point
and preserve its exhaustive match over `State`. Each arm delegates to a private
helper for that state's existing byte-transition body. Helpers that need
context receive only `stack_top` and/or `byte`; `InStrLit` additionally receives
its `escaped` flag. Existing byte re-dispatch remains routed through `step`, so
multi-byte operator and terminal-lexeme behavior is unchanged.

The two already-shared hubs (`value_position` and `block_stmt`) remain shared
instead of being duplicated. Small states with identical shapes may share a
parameterized helper when the state-specific dispatcher still makes every
variant explicit. The three lint-suppressed tests are divided into focused
tests (or data-driven assertions) without deleting or weakening an assertion.

Only the independent `pure-analyzer-purecard` product and this spec are touched;
the analyzer dependency DAG and zero-edge product boundary remain unchanged.

## API / contract impact

None. `State`, `Frame`, `Step`, and public `step` retain their signatures and
semantics. The extracted helpers are private implementation details.

## Dependencies

None.

## Testing plan

- Failing-first: remove the four scoped lint suppressions and run `just lint`;
  current Clippy must reject the oversized function/tests for the tracked
  reasons before implementation begins.
- Run `just test-unit` after extraction; every pre-existing unit assertion must
  pass unchanged in meaning.
- Run `just ci`, `just review`, and `just ci-full` before opening the PR.
- Have an independent reviewer compare the diff with this spec and issue #6,
  paying particular attention to transition equivalence and gate weakening.

## Verification

- Failing-first `just lint`: `step` failed at cognitive complexity 75/15 and
  299/120 lines after the suppressions were removed.
- `just lint`, `just test-unit` (216/216), `just ci` (394/394 plus doctests),
  and `just review` passed without skips or warnings.
- Coverage passed at 97.81% lines. Mutation testing caught 447 default-workspace
  and seven FFI mutants, with zero missed or timed-out mutants.
- Every `just ci-full` component passed. The aggregate run reached `just deny`
  after coverage and mutation, then sandbox DNS blocked its crate downloads;
  `just deny` and `just audit` passed with approved network access, and every
  remaining component passed individually.
- Two independent reviews found no issues. A focused reviewer compared all
  60,160 combinations of concrete state, stack top, and byte with `main`; the
  transition tables were byte-identical, and no original assertion changed.

## Risks & rollout

The primary risk is a dropped or subtly changed transition while moving match
arms. Mitigation is a mechanical extraction that retains the exhaustive public
dispatcher, followed by the existing unit, corpus, mutation, and full CI gates.
No feature flag or staged rollout is appropriate for an internal structural
refactor; rollback is a single commit/PR revert.
