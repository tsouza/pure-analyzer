# 0004. M1 L1 grammar covers both corpus arms (relational + class-navigation)

- **Status:** Accepted
- **Date:** 2026-07-11
- **Deciders:** Thiago Souza; agent (Claude)

## Context

M1 brings the first real decoder into `src/`: a byte-level pushdown automaton
whose done-criterion is **100% gold-corpus soundness** — the PDA must never reach
a dead state on any byte a gold query actually emits, and must be in an accepting
state at end-of-stream, for *every* in-scope gold query (`specs/m1-l1-grammar.md`,
G2).

The committed corpus (`corpus/gold_queries.jsonl`, verified this session) holds
**5,034** execution-verified gold queries in two idioms, discriminated by their
`source` production:

| Arm                      | Idiom                                                | Discriminator     | Count | Share |
| ------------------------ | ---------------------------------------------------- | ----------------- | ----: | ----: |
| **A** — relational       | `Db->tableReference('default','T')->tableToTDS()->…` | `tableReference(` | 4,639 | 92.2% |
| **C** — class-navigation | `Class.all()->…`                                     | `.all()`          | 395   | 7.8%  |

The two arms are exhaustive and disjoint (0 records match neither, 0 match both).
`docs/spec/grammar.md` §5 as drafted grammared **only arm-C** — it was written
against an earlier ~1,791-query pilot in which the class-navigation form
dominated. The relational envelope (`tableReference`/`tableToTDS`, 3-arg
string-named `agg`, typed-multiplicity `TDSRow[1]`/`[*]` binders, the brace
multi-binder `join` predicate, `JoinType.INNER`/`LEFT_OUTER`, `renameColumns`/
`pair`, `extend`/`col`, `limit`, string-or-list `restrict`/`groupBy`, and
`between`) had no productions at all.

The forces in tension:

- The grammar's **core principle** (§5) is that *the verified corpus **is** the
  spec*: a grammar that masks a token appearing in a gold query is a soundness
  bug, and the standing instruction is "do not invent productions the corpus
  doesn't exercise, and do not omit ones it does."
- The M0 harness slice was arm-C-shaped, so shipping arm-C first *looked* like a
  natural staging step.
- But arm-C is 7.8% of gold. An arm-C-only M1 would leave the tokens that 92.2%
  of gold queries emit off the grammar — a soundness defect over the real corpus,
  not a deferral.

Widening §5 to admit arm-A only ever *relaxes* the grammar (it accepts strictly
more strings); since the corpus is all-gold, relaxing to admit it cannot
introduce a soundness regression — it can only affect completeness/precision,
which the differential-compile lane (not L1) governs. The authoritative grammar
is nonetheless a human-gated surface, so the widening plus this record is a
deliberate decision, not an incidental code change.

## Decision

We will grammar **both arms in M1**, as a single PDA that branches at the
`source` production, and widen `docs/spec/grammar.md` §5 to document the arm-A
relational envelope and its construct inventory alongside the existing arm-C
productions. The M1 soundness gate's scope is the **full 5,034-query corpus**,
asserted per-envelope partition (4,639 arm-A + 395 arm-C) against exact named
record counts. The arm-C productions are kept intact; arm-A is additive.

## Alternatives considered

- **Arm-C first, quarantine arm-A to a later milestone (M1.5).** Rejected. This
  would ship a decoder that is sound over 7.8% of gold and silently dead-states
  on the other 92.2%. Meeting the "100% gold soundness" done-criterion under this
  plan would require *renegotiating M1's corpus down to the 395 arm-C queries* —
  a scope change only a human may authorize, and one that ships the minority
  idiom as if it were the product. The staging benefit it seemed to offer is
  recovered without the scope cut (see next point).
- **Two separate engines / a second PDA for arm-A.** Rejected. The hard part —
  the whitespace-skip layer, ident/classpath/`strlit` (`''`-doubling) lexis, the
  bracket + pipeline-continuation stack, lambda frames, nested sub-pipeline
  recursion, and the dead-state error tuple — is identical across arms. Arm-A
  adds ~10 leaf productions on that shared machinery; a second engine would
  duplicate the spine (a DRY defect) for no soundness gain.
- **Sequence the work but not the scope.** Adopted as the *implementation*
  tactic, not an alternative to both-arms: the shared spine + arm-C land first at
  an internally-green checkpoint (the 395-query partition passes and exercises
  the full harness end-to-end), then the arm-A productions widen the *same* PDA
  to the full 5,034. This retires the big-bang risk without dropping scope or
  renegotiating the corpus.

## Consequences

- **Easier:** the M1 soundness gate is honest — it runs over the real corpus, so
  a green gate means the decoder is sound over what the model actually emits, not
  over a hand-picked slice. `docs/spec/grammar.md` §5 now matches corpus reality.
- **Harder / follow-on obligations:** §5's construct inventory (§5.7) is now
  locked against the full corpus and must be re-verified whenever the corpus
  changes; the PDA carries both arms' leaf productions and the join
  sub-pipeline recursion. The soundness test asserts exact per-partition counts
  (`ARM_A = 4639`, `ARM_C = 395`, `EXPECTED_GOLD_RECORDS = 5034`) so corpus
  shrinkage or mis-partition reddens the gate.
- **Relationship to prior ADRs:** this does **not** supersede ADR-0002
  (single published crate) or ADR-0003 (`src/` = dep-light core, harness in
  `tests/`). It is a scope decision about *what grammar M1 encodes*, orthogonal
  to the crate/packaging boundary those ADRs fix.
- **Revisit if:** a future corpus adds a third top-level idiom (a new `source`
  form), at which point the same both-arms reasoning applies to the new arm and
  §5 is widened again through this same human-gated flow.
