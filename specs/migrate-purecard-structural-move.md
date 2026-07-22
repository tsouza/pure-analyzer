# Spec: migrate-purecard-structural-move

- Status: draft
- Created: 2026-07-22
- Owner: agent (user decision: fold purecard into pure-analyzer as the
  umbrella for all Pure-analysis tooling)

## Problem

purecard (a grammar/schema-constrained decoder for Legend Pure) and
pure-analyzer were both built from the same starter kit and both work on
Legend Pure. purecard already has a working differential-testing oracle
against the real Legend engine (`corpus/legend-stack/`, a docker-compose'd
engine + harness) and a verified gold-query corpus — infrastructure
pure-analyzer's own design doc (§8) says it will need once
`pure-analyzer-parser` exists. Per the user's decision (discussed and
plan-approved this session), purecard moves under pure-analyzer as a
workspace member rather than staying a separate repo, so this and other
Pure-analysis infrastructure isn't duplicated across two repos. This PR
(PR 1 of a 5-PR sequence, see `/home/thiago/.claude/plans/polymorphic-booping-axolotl.md`)
is the structural move only.

## Goals

- [x] purecard's crate (`src/`, `tests/`, `benches/`, `fuzz/`, `corpus/`,
      `docs/`, `python/`, `lints/`) lives at `crates/pure-analyzer-purecard/`,
      a real workspace member — not a nested `[workspace]` (Cargo forbids
      workspace-in-workspace; this is the one hard structural blocker).
- [x] Package name `pure-analyzer-purecard` (this workspace's naming
      convention); library/PyO3/Python identifier stays `purecard` via an
      explicit `[lib] name = "purecard"` override, so no internal
      `use purecard::...` / `import purecard` reference breaks.
- [x] `publish = false` — per the user's decision, crates.io identity doesn't
      matter; the standalone `purecard` v0.1.0 publish (191 downloads) is
      retired, not continued from the new location.
- [x] Every real dependency-version conflict resolved by adopting
      pure-analyzer's newer, already-verified pin (`thiserror`, `serde`,
      `serde_json`); genuinely new dependencies (`pyo3`, `self_cell`,
      `tokenizers`, `ureq`, `proptest`) each vetted
      (`docs/dependencies/<crate>.md`) and pinned current, not carried over
      stale (`self_cell` was bumped 1.2.2 -> 1.3.0 as part of this).
- [x] `just ci` genuinely green with purecard in the tree — including fixing
      what surfaced only once it was: `xtask ci()`'s test step no longer uses
      `--all-features` for actual test *execution* (purecard's
      `legend`/`qwen-oracle`/`fused-extract` features gate heavy,
      network-/env-dependent tests that must stay out of the hermetic per-PR
      gate — matching the separation purecard's own pre-migration CI already
      established, which pure-analyzer hadn't needed until now); a `--doc
      --all-features` step added instead, since nextest never runs doctests.

## Non-goals

- Not the automation reconciliation (`xtask` subcommands, `justfile` recipes,
  `.mise.toml` tool additions for `check_core_deplight`/`qwen-oracle`/etc.) —
  PR 2.
- Not CI workflow files (`qwen-oracle.yml`, `wheels.yml`) or the remaining
  `deny.toml`/`ast-grep-rules` reconciliation beyond what PR 1 needed to be
  green — PR 3.
- Not the constitution/domain-model/ADR updates — PR 4.
- Not deleting the old `purecard` GitHub repo — the final, separately
  confirmed step after PR 5.
- Not refactoring `grammar/pda.rs::step`'s complexity (75/15 cognitive
  complexity, 299/120 lines) — real, pre-existing debt exposed (not
  introduced) by this migration, unblocked here with a scoped, documented
  `#[allow(...)]`, tracked as a dedicated follow-up. See the function's own
  doc comment for the full explanation.

## Design

Plain file copy (git history not preserved, per the user's decision) from
`../purecard` into `crates/pure-analyzer-purecard/`. `lints/` and `fuzz/`
stay excluded from the Cargo workspace (nightly-toolchain/dylint
requirements), same reasoning as pure-analyzer's own top-level `fuzz/`
exclude, just now also covering purecard's copies at their new nested paths.

Two classes of bug surfaced by inheriting this workspace's stricter/different
lint config, both real and both fixed here (not silenced):

1. Three test files had `#![cfg(feature = "...")]` as their literal first
   line, *before* their crate-level `//!` doc comment. An inner attribute
   cfg's out everything textually after it in the same file when the
   condition is false — including a doc comment written later — so with the
   feature off (the default), the crate-level doc vanished, tripping
   `#![deny(missing_docs)]` (inherited via `[lints] workspace = true`, wider
   in scope than purecard's own pre-migration lint setup). Fixed by moving
   the doc comment above the cfg in each file
   (`tests/legend_completeness.rs`, `tests/classify_oracle.rs` — the third,
   `tests/qwen_soundness.rs`, already had the correct order).
2. `tests/spider_corpus_replay.rs` had two `_ => unreachable!()` arms in
   already-narrowed exhaustive matches. `no-postponed-markers.yml`'s pattern
   (bans `unreachable!()`) is identical in both repos, but its `files:` scope
   differs: purecard's only ever covered `src/**/*.rs` (never `tests/`), so
   this was invisible to its own sweep; pure-analyzer's broader
   `crates/**/*.rs` (which nests `tests/` under it) now catches it. Fixed by
   replacing both with a graceful `Err(...)` describing the actually-
   impossible state, rather than excluding tests from the sweep's scope.

## API / contract impact

None beyond the workspace's own dependency graph (new optional/dev deps,
each vetted). `pure-analyzer-purecard` is not part of the
`lexer -> syntax -> parser -> ... -> libpure -> cli` DAG — it's an
independent decoder, not a front-end over `libpure` (that would be a real,
separate future integration, not this migration).

## Testing plan

- `cargo test -p pure-analyzer-purecard --all-features`: 345 tests pass
  (purecard's own 280+ plus doctests), matching purecard's own pre-migration
  test count (nothing silently stopped running during the copy).
- `just ci` green: fmt, clippy `--all-features -D warnings`, nextest
  (default features — hermetic), `cargo test --doc --all-features`.
- `just deny`, `just machete`, `just sweep`, `just lint-md`: all clean.
- One test (`fused_fixture_matches_the_real_tokenizers`, under the
  `fused-extract` feature) fails without `QWEN_TOKENIZER_JSON` set — expected
  and by design (its own doc comment: "heavy and network-fed... NOT a per-PR
  gate"), not part of `just ci`'s scope.

## Risks & rollout

Structural-only change, no consumers of `pure-analyzer-purecard` elsewhere in
the workspace yet, so blast radius is contained to the new crate itself. The
`#[allow(clippy::cognitive_complexity, clippy::too_many_lines)]` on
`grammar/pda.rs::step` is a deliberate, documented, narrowly-scoped exception
(not a threshold change in `clippy.toml`, which stays PROTECTED and
untouched) — tracked as real follow-up work, not swept under the rug.
