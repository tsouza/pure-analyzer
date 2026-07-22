# 0007. Modern Legend Pure constructs are oracle'd by a separate seed corpus

- **Status:** Accepted
- **Date:** 2026-07-16
- **Deciders:** Thiago Souza; agent (Claude)

## Context

The L1 grammar's core principle (§5) is that *the verified corpus **is** the
spec*: a production must be motivated by a gold query, and a grammar that masks a
token a gold query emits is a soundness bug. The gold corpus that principle is
anchored on — `corpus/gold_queries.jsonl` — is 5,034 execution-verified,
Spider-derived queries, and its exact count is load-bearing: `ARM_A + ARM_C ==
EXPECTED_GOLD_RECORDS == wc -l gold_queries.jsonl` is asserted by
`just check-doc-facts`, and the number "5,034" is cited across ~15 docs, the
`selfcheck_corpus.rs` round-trip, and ADR-0006.

The fine-tuned NL→Legend-Pure model PureCARD constrains emits **modern Legend
Pure**, which the Spider-derived pilot never exercised: the `%latest` milestoning
literal (gap report §5/G2, this PR) and the `~` Relation/Function API family
(G1/arm-R, a follow-on PR). These constructs are real — the gap report measured
them over 3,908 target-dialect gold queries — but they come from a *different*
pipeline (pure-research) with different provenance than the Spider gold.

The forces in tension: the grammar must be widened to admit these constructs
(they are sound to admit — widening only relaxes an all-gold grammar), and the
widening must stay oracle-driven (seeded by real gold, not invented). But folding
the new seeds into `gold_queries.jsonl` would (a) mix two provenances in a file
whose README claims a single Spider lineage, and (b) force churning the 5,034
count in ~15 docs, the doc-facts gate, and the selfcheck round-trip — noise that
buries the actual grammar change and risks a stale-citation gate failure.

## Decision

We will seed modern-dialect constructs in a **separate** file,
`corpus/modern_dialect_seeds.jsonl`, with its own soundness lane
(`tests/modern_dialect_soundness.rs`) asserting the same killer property (never
dead, ends accepting) plus per-arm envelope classification against exact named
counts. `corpus/gold_queries.jsonl` and its 5,034-count citations stay frozen and
Spider-pure. A new grammar production requires a seed in one corpus or the other —
the oracle-driven principle is preserved, just across two provenance-separated
files.

## Alternatives considered

- **Append the seeds to `gold_queries.jsonl`.** Rejected. It conflates two
  provenances in one file, and every added row forces updating `ARM_C`,
  `EXPECTED_GOLD_RECORDS`, the doc-facts gate, `selfcheck_corpus.rs`, and ~15
  "5,034" citations — a large, error-prone diff orthogonal to the grammar change,
  with a real chance of reddening `check-doc-facts` on a missed citation.
- **Invent the productions from the gap report's EBNF without seeds.** Rejected —
  it violates §5's "do not invent productions the corpus does not exercise." The
  gap report supplies concrete gold strings precisely so the widening stays
  oracle-driven; those strings are the seeds.
- **A feature flag / dialect switch.** Rejected as premature. The constructs are
  additive and sound to always admit (they only relax the grammar); a dialect
  toggle adds configuration surface with no soundness benefit.

## Consequences

- **Easier:** the grammar can track the emitted modern dialect construct-by-
  construct, each seeded and replayed, without perturbing the frozen Spider gold
  or its citations. Provenance stays honest — the README of each corpus states
  its lineage.
- **Harder / follow-on obligations:** there are now **two** soundness corpora to
  keep green; a reviewer must check that a new production is seeded in one of
  them. `tests/modern_dialect_soundness.rs` holds exact per-arm counts
  (`SEED_ARM_A`, `SEED_ARM_C`, `SEED_ARM_R`) so a dropped or mislabelled seed
  reddens the gate. The arm-R Relation/Function API work (G1, ADR-0008) extended
  this file and its counts with `~`-bearing seeds, and added the
  `Envelope::RelationApi` variant the seed lane classifies against.
- **Relationship to prior ADRs:** complements ADR-0004 (both-arms M1 scope) — it
  does not change what `gold_queries.jsonl` encodes; it adds a second, separate
  oracle for constructs that corpus never held. It does not touch ADR-0002/0003
  (crate/packaging boundaries).
- **Revisit if:** the modern dialect becomes the *primary* target and the Spider
  pilot is retired, at which point the two corpora may merge under a single
  refreshed provenance and count.
