# 0010. A declarative transition-table schema for `CompiledGrammar::from_spec`

- **Status:** Accepted
- **Date:** 2026-08-26
- **Deciders:** Agent (issue #57), reviewer gate

## Context

`CompiledGrammar::from_spec` (`src/grammar/compiled.rs`) accepted a `spec: &str`
argument but ignored it, always compiling the single fixed, hand-written §5
grammar (`src/grammar/pda.rs`). Issue #57 requires `from_spec` to actually lower
a supplied grammar spec into the runtime PDA/mask structure, so a host can load
a grammar PureCARD did not ship with.

The standalone predecessor of this crate considered — and explicitly rejected —
a closely related design. Its M1 milestone doc (`specs/m1-l1-grammar.md` in the
pre-migration `purecard` repository) records:

> **Compile-time / runtime EBNF-string parsing into transition tables.**
> Rejected: a hand-written match is the live automaton; the EBNF string is a
> test oracle only... a runtime EBNF interpreter would add a parser-combinator
> dependency (fails vetting) plus a lowering pass — two new untested soundness
> surfaces — for zero soundness gain over an explicit match.

That decision was scoped to the *fixed* shipped grammar: it never needed to
vary, so interpreting a grammar text at runtime bought nothing. Issue #57's
requirement — a *supplied*, host-varying grammar — is a different problem the
prior decision did not have to solve, so it does not, by itself, forbid this
one. But its concern is still live: whatever "spec" means here must not
reintroduce a parser-combinator dependency or an undertested interpretation
layer just to reproduce what a `match` already does safely.

## Decision

We define the grammar spec as **data, not text-to-be-parsed**: a versioned,
`serde`-deserializable transition table (`src/grammar/spec.rs`) — named states,
each an *ordered* list of byte-guarded rules (`match` + `guard` + `action`),
plus a small set of named stack-frame kinds. `serde`/`serde_json` are already
this crate's core-dependency allowlist (ADR-0005, for `Schema::from_json`), so
this adds no new dependency and no new parsing surface: `GrammarSpec::parse`
is a direct `serde_json::from_str`, and lowering
(`src/grammar/compile.rs::CompiledAutomaton::compile`) is validation and
dense-table construction, never grammar interpretation. There is no EBNF text
parser anywhere in this crate.

Rules within a state are tried in order, first match wins — the same semantics
as the `match` arms it replaces. `Action::Goto` re-evaluates the same byte
against another state's rules without consuming input, the declarative form of
the hand-written PDA's "delegate to another state's arm" fallthrough (used for
multi-byte operators and shared literal-closing logic). A state's completion
is derived the same way `Pda::is_accepting` derives it — feeding a designated
boundary byte through `step` and checking whether it lands in a state marked
`accepting` — so a mid-token state (an identifier body, an open number) need
not duplicate a hub state's `accepting` flag.

Compilation is bounded (`MAX_STATES`, `MAX_RULES_PER_STATE`, `MAX_TOTAL_RULES`,
`MAX_FRAMES`) and validated before any `RtnPda` can run: unknown start/target
states, unknown frames, ambiguous or unreachable rules, an unguarded `Pop`, a
cyclic `Goto` chain, and an automaton with no reachable accepting state are all
typed `SpecError` variants naming the offending state and rule index.

## Alternatives considered

- **Parse the existing §5 EBNF text at runtime.** Rejected for the same reason
  the standalone repo rejected it: a general EBNF interpreter is a second,
  under-tested soundness surface, and would need either a parser-combinator
  dependency (fails the vetting rubric — `docs/methodology/overview.md`) or a
  hand-rolled EBNF parser that owns its own edge cases for no gain over `serde`
  deserializing a schema we already control.
- **A hand-rolled bespoke text format instead of JSON.** Rejected: it would be
  a second parser this crate owns and tests from scratch, when `serde_json` is
  already an approved core dependency and gives span-aware malformed-input
  errors for free.
- **Unordered pattern rules with a general overlap/ambiguity solver.** Rejected
  as unnecessary complexity: ordered, first-match-wins rules mirror the
  hand-written `match` arms exactly, and the only "ambiguity" worth surfacing
  as an error is an exact duplicate rule — a real overlap solver would need to
  reason about guards that depend on the runtime stack, which ordering already
  sidesteps.

## Consequences

- Encoding the full shipped §5 grammar as a `GrammarSpec` (so
  `CompiledGrammar::compile`'s built-in grammar also compiles through
  `from_spec`, closing out issue #57's remaining acceptance criteria) is
  mechanical transcription against this schema, not new design.
- The hand-written `pda.rs` automaton remains the equivalence oracle the
  transcription is checked against (differentially, over the existing gold /
  precision / property / fuzz corpora) before any default switches to the
  spec-compiled path.
- A future capability this schema cannot express (e.g. lookahead) requires a
  new `version` tag, never widening `V1` — a consumer pinned to `"1"` must
  never observe a behavior change from a spec it already accepted.
