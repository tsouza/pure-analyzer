# 0008. Arm-R — admit the modern Relation/Function API (`~`-column constructs)

- **Status:** Accepted
- **Date:** 2026-07-16
- **Deciders:** Thiago Souza; agent (Claude)

## Context

ADR-0004 scoped the M1 grammar to the two Spider-corpus idioms (arm-A relational,
arm-C class-navigation) and anticipated: *"Revisit if a future corpus adds a third
top-level idiom."* The gap report (`PURECARD_GRAMMAR_GAP_REPORT.md`, §4/G1) is that
trigger: the fine-tuned NL→Legend-Pure model emits the **modern Legend Pure
Relation/Function API** (`meta::pure::functions::relation::*`) — every construct
distinguished by the `~` column sigil: `project(~[…])`, `groupBy(~[…], ~'agg':…)`,
`sort([ascending(~col)])`, `extend(over(~col), ~[…])`, `rename(~old, ~new)`. The
report measured 61 `project(~[…])`, 24 window `extend`, and more over 3,908
target-dialect gold queries. The L1 byte-PDA dead-stated on the first `~`.

The forces: the productions are additive and sound to admit (widening an all-gold
grammar cannot introduce a soundness regression); they must stay oracle-driven
(seeded by real gold, per §5); and the arm is class-nav-sourced in the seeds
(`Class.all()->project(~[…])`), so it overlaps the arm-C `.all(` marker — the `~`
is the true discriminator.

## Decision

We will admit arm-R as an **additive** widening of the same PDA, distinguished by
the `~` sigil. Concretely: one new automaton state `SawTilde` (reached from the
value hubs on `~`) opens a relation column-set `~[…]` or a column reference
`~ident`/`~'…'`, and a `:` may open a `{`-brace frame lambda; every other arm-R
element reuses the existing value-hub/lambda/bracket machinery. A new
`Envelope::RelationApi` classifies any `~`-bearing query (checked before the
`.all(`/`tableReference` markers). The arm-R gold is seeded in the separate
modern-dialect corpus (ADR-0007), not the frozen Spider gold. `docs/spec/grammar.md`
gains §5.9 documenting the productions and the residual over-approximation.

## Alternatives considered

- **A second PDA / a distinct arm-R engine.** Rejected for the same reason
  ADR-0004 rejected it for arm-A: the spine (whitespace, ident/strlit lexis,
  bracket + pipeline stack, lambda frames, dead-state tuple) is identical. Arm-R
  adds *one* state on that shared machinery; a second engine would duplicate the
  spine for no soundness gain (a DRY defect).
- **Encode each arm-R production exactly (a tight grammar).** Rejected. The
  byte-PDA is deliberately a residual over-approximation (§5.6): encoding the
  precise `relAggSpec`-vs-`winAggSpec` colon asymmetry, or the exact
  `frameLambda` binder count, needs per-frame phase tracking the machine omits by
  design. The compiler oracle re-catches the residue, exactly as it does for the
  arm-A typed-binder multiplicity and the `join` brace-lambda body.
- **Fold arm-R into `Envelope::ClassNav`** (bump the arm-C seed count). Rejected —
  it conflates the arm-R construct family into the class-nav count and loses the
  report's "third arm" framing. A distinct `RelationApi` envelope gives a clean,
  disjoint 3-way partition the seed lane asserts per-arm.

## Consequences

- **Easier:** the grammar now admits the modern Relation/Function API the model
  actually emits; the fix is one state plus additive transitions, and every arm-R
  seed streams to acceptance. `Envelope::classify` returns a clean 3-way partition.
- **Harder / follow-on obligations:** `Envelope` grows a variant, so every `match`
  over it must handle `RelationApi` (the Spider soundness lane asserts arm-R never
  appears in `gold_queries.jsonl`; the seed lane counts it explicitly). The
  completeness walker's alphabet gains `~` so it exercises `SawTilde`. Arm-R is an
  L2 pass-through — a `~`-column name is never narrowed — so no new N/T rule is
  claimed for it; if a future seed compares or resolves a `~`-column, L2 revisits.
- **Relationship to prior ADRs:** this is the ADR-0004 "third idiom" revisit,
  resolved the same both-arms way (additive on the shared PDA). It builds on
  ADR-0007 (the seed corpus that oracle's it). It does not touch ADR-0002/0003.
- **Revisit if:** arm-R appears on a *relational* (arm-A) source in a future seed
  (the `~` API works over TDS too), at which point the `RelationApi`-before-
  `Relational` classify order and the seed provenance are re-examined.
