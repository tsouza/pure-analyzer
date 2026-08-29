# Testing strategy (PureCARD decoder)

PureCARD is a byte-level grammar/schema-constrained decoder, not a web server.
The root [testing methodology](../../../../docs/methodology/testing.md) governs
repository-wide policy; this page maps that policy onto the decoder-specific
oracles in [the correctness spec](../spec/testing.md).

The failure modes are asymmetric. A **soundness** bug masks a continuation a
valid query needs. A **completeness** bug admits a path that the real Legend
compiler rejects. Both can stay invisible in ordinary unit tests, so each claim
below names its input, oracle, and execution lane.

## Test map

- **Unit, hermetic:** hand-authored assertions prove local PDA, mask, schema,
  error, and FFI contracts.
- **Properties, hermetic:** generated cases and committed regressions prove
  mask/accept agreement, rollback, and L2 ⊆ L1.
- **Corpus replay, hermetic:** 5,034 execution-verified queries and the
  provenance-distinct modern seeds prove membership in the fixed PDA.
- **L2 replay and precision, hermetic:** committed schemas, BPE regressions, and
  Spider-derived cases exercise the covered narrowing rules.
- **Real tokenizer, scheduled/on-demand:** the pinned Qwen tokenizer and
  `tests/qwen_soundness.rs` prove actual token-ID replay.
- **Accepting walks, hermetic:** seeded PDA walks prove the deterministic walker
  emits non-trivial strings accepted by an independent session.
- **Live Legend, local/on-demand:** protocol fixtures and annotated walks verify
  managed-stack reachability and response classification.
- **Python/wheel, CI verification:** Rust FFI tests, Python tests, and built
  wheels prove that the PyO3 surface builds, imports, and delegates.

## Why end to end is not a unit test

A unit test can prove one transition or mask operation. It cannot prove that a
real tokenizer's token boundaries match the supplied vocabulary, that applying a
mask to a model's logits produces a complete query, or that Legend accepts the
result. Those are integration properties across independently implemented
systems.

The real-Qwen lane loads the pinned tokenizer, builds `Vocab` in token-ID order,
tokenizes the corpus, and replays the actual IDs. The live Legend lane verifies
engine reachability and response classification. These are distinct test
boundaries, not a claim about host model behavior or general compiler validity.

## Differential Legend oracle

Do not fork Legend. PureCARD is a recognizer and mask generator, not an AST
producer, and the stock engine exposes two relevant signals:

1. `POST /pure/v1/grammar/grammarToJson/lambda` tests parse membership and
   produces protocol JSON.
2. `POST /pure/v1/compilation/lambdaReturnType` tests name/type resolution
   against a supplied PMCD.

The pinned stack under `corpus/legend-stack/` is therefore the oracle. The test
client exercises health and `lambdaReturnType` classification with protocol
fixtures; the decoder itself remains independent of the engine.

## How test inputs are derived

### Frozen and modern corpora

`corpus/gold_queries.jsonl` contains 5,034 execution-verified queries across
161 databases. `soundness_replay.rs` streams every query through the real PDA
byte by byte and asserts that it never dead-states and ends accepting. The exact
record count is asserted so an emptied loop cannot pass vacuously.

The frozen corpus includes `map` (6 gold records). Their normal replay coverage
keeps the resolved lambda-argument branch in the fixed PDA from regressing.

Newer Legend constructs that are absent from the frozen Spider-derived corpus
live in `corpus/modern_dialect_seeds.jsonl`.
`modern_dialect_soundness.rs` applies the same acceptance property and checks
the per-envelope counts. A production widening needs a provenance-bearing seed
in one of these corpora.

### Schema and structural cases

`l2_soundness.rs` replays every in-scope gold query with its committed schema.
`l2_properties.rs` asserts that L2 never widens L1. `l2_precision.rs`,
`bpe_split_soundness.rs`, and `fused_tokenizer_precision.rs` pin boundary and
counterfactual cases where byte-level BPE tokens cross Pure lexeme boundaries.

Every fixture in `l2_precision.rs` that freezes a **kill** — a walk the decoder
must never emit, or a decision point where a phantom must be cleared — lives in
one table, `FROZEN_KILLS`, and records the `L2Position` rule that closes it. A
new fixture is a row in that table, never a new list, because
`every_rule_kind_has_a_frozen_walk_that_it_closes` reads the whole table and
fails when a shipped rule is no longer the recorded closer of anything. It
exists because a newly landed rule silently taking over an earlier rule's kill —
leaving that rule with no walk-level evidence at all, while every fixture still
passed — happened four times across issue #55's phases, and neither the
per-fixture closing-token pins nor `--in-diff` mutation can see it: the code
that lost its coverage is untouched by the diff. Soundness *edges* — directed
contrasts pinned beside a rule's precision so it cannot pass by masking
everything — stay outside the table, having no closing mechanism to record.

`spider_corpus_replay.rs` mechanically derives a much larger structural case
set from the Spider schemas. Soundness stays strict; known precision leaks are
counted and tagged so fixing a gap requires removing its allowlist entry rather
than weakening the corpus.

### Tokenizer cases

`qwen_soundness.rs` uses the actual pinned Qwen tokenizer rather than a
lex-then-split proxy. It checks full L1 replay, in-scope L2 replay, and special
token behavior. The workflow restores the revision-keyed tokenizer cache and
fetches only on a miss. This lane proves real-tokenizer token-ID replay, not
real-model inference.

The smaller committed fused-tokenizer fixture keeps relevant Qwen/GPT boundary
shapes in the hermetic lane. The scheduled extractor re-derives that fixture
against the real tokenizers and fails on drift.

### Generated walks, properties, and fuzzing

`completeness_walks.rs` asks the seeded walker for a fixed non-trivial corpus,
then replays each string through a fresh decoder session. It proves the generator
and recognizer agree; it cannot prove Legend compilation.

`mask_properties.rs` and `l2_properties.rs` cover stateful invariants under
generated inputs. The workspace-excluded nightly fuzz crate has three targets:
`accept_token`, `allowed_mask`, and `schema_from_json`. Failing fuzz and
property inputs become committed regressions.

## Implemented test locations

- Local unit tests live under `src/**` and `tests/support/**`.
- Offline language membership:
  `tests/soundness_replay.rs`,
  `tests/modern_dialect_soundness.rs`, and
  `tests/differential_l1.rs`.
- Mask and L2 behavior:
  `tests/mask_oracle.rs`,
  `tests/mask_properties.rs`,
  `tests/l2_soundness.rs`,
  `tests/l2_properties.rs`,
  `tests/l2_precision.rs`,
  `tests/precision_reject.rs`, and
  `tests/spider_corpus_replay.rs`.
- Token-boundary evidence:
  `tests/bpe_split_soundness.rs`,
  `tests/fused_tokenizer_precision.rs`, and
  `tests/qwen_soundness.rs`.
- Generation/live engine:
  `tests/completeness_walks.rs` and
  `tests/legend_completeness.rs`.
- Self-check and Python boundary:
  `tests/selfcheck_corpus.rs`, unit tests in `src/ffi.rs`, and
  `python/tests/test_session.py`.

## Execution lanes

- `just ci` runs the hermetic workspace checks and default/all-feature Rust
  tests without Docker or tokenizer downloads.
- `just coverage` and `just test-mutation` run the root workspace coverage
  and mutation jobs separately from the fast loop.
- `just purecard-fuzz-ci` time-boxes the three decoder fuzz targets; the
  dedicated workflow also guards build rot and scheduled fuzzing.
- `just qwen-oracle` runs actual Qwen token-ID replay. Its workflow is scheduled
  and manually dispatchable, not a per-PR network dependency.
- `just wheel` builds the unpublished verification wheel; the wheel workflow
  smoke-tests supported Python/platform combinations.
- `just test-legend` owns compose startup, health wait, package-scoped Legend
  tests, and teardown. It runs nightly (`purecard-legend.yml`, plus dispatch)
  and on demand locally, never per PR — the stack is too heavy for that lane.
  It carries the schema-walk compile-rate floors: per database, and per walk
  partition (`recipe` vs `exploration`), since recipe walks compile by
  construction and only the exploration partition is evidence about mask
  precision.

All commands run from the monorepo root through the shared `just` frontend.

## Decoder boundary

The test suite covers the oracle/corpus harness, hand-written emitted-subset
PDA, lazy mask cache, selected schema overlay, PyO3/wheel boundary, self-check,
error handling, fuzz targets, and benchmarks. The decoder's behavioral limits
are defined in the [product reference](../spec/overview.md#10-operating-limits),
not in a checked-in task ledger.
