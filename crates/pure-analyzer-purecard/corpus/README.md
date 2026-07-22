# PureCARD test corpus

Distilled from the **pure-lingua** project (the NL→Legend-Pure training effort) — the correctness
oracle backbone for PureCARD. See [`../docs/spec/testing.md`](../docs/spec/testing.md) §13 for how these are used.

## Contents

- `gold_queries.jsonl` — 5,034 unique execution-verified gold Pure queries across 161 databases.
  One JSON object per line: `{db_id, source_id, arm, constructs[], pure_text}`.
  `arm`: "A" = relational/tableToTDS idiom, "C" = class-navigation idiom.
  **SOUNDNESS oracle**: replay each through the L1 decoder — any gold token the mask forbids is a
  grammar bug. Fully OFFLINE (no Legend engine needed). Also the empirical basis of the L1 grammar.
  Distilled from pure-lingua `data/phase2/{armA,armC}_*.jsonl` (accepted records; full dir 231MB → 4.8MB here).
- `modern_dialect_seeds.jsonl` — seed gold for **modern Legend Pure** constructs the Spider-derived
  pilot never exercised: the `%latest` milestoning seeds (gap report G2, `arm: "C"`) and the `~`
  Relation/Function API (arm-R, `arm: "R"`, gap report G1). Same record shape as `gold_queries.jsonl`.
  Distinct provenance (the pure-research gap report, not the Spider pipeline), kept separate so
  `gold_queries.jsonl` and its 5,034-count citations stay frozen. **SOUNDNESS oracle**, replayed by
  `tests/modern_dialect_soundness.rs`. See [ADR-0007](../docs/decisions/0007-modern-dialect-seed-corpus.md)
  and [ADR-0008](../docs/decisions/0008-arm-r-relation-function-api.md).
- `schemas/*.md` — 8 database schema context files (autogen Pure classes + associations + exec
  coords) for 5 pilot + 3 out-of-sample dbs. **L2 (schema-consistency) test inputs.** Workspace ids
  inside are stale/ephemeral — only the class/property/association structure matters.
- `legend-stack/` — the Legend engine docker-compose + configs (engine 4.113.0 + fs-SDLC 0.195.0,
  anonymous auth) for the COMPLETENESS oracle (compiling generated queries). See [`../docs/spec/testing.md`](../docs/spec/testing.md) §14.

## Provenance

Origin repo: pure-lingua (`data/phase2/`, `data/pilot/armC_ctx_*.md`, `data/pilot/oos_ctx_*.md`,
`infra/legend-stack/`). Faithfulness of the gold queries: execution-equivalence verified against
real data (see the pure-lingua Gate-2 report). CC BY-SA 4.0 (curated corpus lineage — Spider-derived).
