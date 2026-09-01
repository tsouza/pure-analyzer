# justfile — the human (and agent) frontend for this workspace.
#
# `just` is the ONLY entry point you should need for day-to-day work. `just ci`
# is the fast inner-loop gate; `just ci-full` is the local mirror of the whole
# CI matrix. A green `just ci` is necessary but not sufficient — the full gate
# is CI (mirrored by `just ci-full`); "nothing merges red" is enforced by CI.
#
# Design rules for this file (see CLAUDE.md):
#   * The justfile is the frontend. If a workflow is missing a target, add it.
#   * Two tiers below a recipe, picked by whether there's real logic:
#       - Simple pass-through to one tool -> call it directly (`cargo deny check`).
#       - Real control flow (branching, loops, sequencing, templating) ->
#         `cargo xtask <subcommand>` (typed Rust), never inline shell here.
#     (See constitution.md §2: nested `cargo xtask` -> `cargo <plugin>` calls can
#     mangle the plugin's argv — reserve xtask for logic, not pass-throughs.)
#   * Every tool referenced in CI has a target here, and vice-versa.

set shell := ["bash", "-euo", "pipefail", "-c"]
set dotenv-load := false

# Default: list all available recipes.
default:
    @just --list

# ---------------------------------------------------------------------------
# Formatting & linting
# ---------------------------------------------------------------------------

# Format all code in place.
fmt:
    cargo fmt --all

# Verify formatting without modifying files (CI gate).
fmt-check:
    cargo fmt --all -- --check

# Clippy with warnings denied in both default and all-feature configurations.
# The first pass covers PureCARD's cfg(not(feature = "legend")) test binary.
lint:
    cargo clippy --workspace --all-targets -- -D warnings
    cargo clippy --workspace --all-targets --all-features -- -D warnings

# Lint + auto-fix markdown (aligns tables for MD060, then markdownlint --fix).
lint-md:
    bun scripts/lib/align-md-tables.mjs $(git ls-files '*.md' '*.markdown')
    bunx markdownlint-cli2 --fix "**/*.md" "**/*.markdown"
    just check-doc-links

# Verify commit messages on this branch follow Conventional Commits.
lint-commits:
    bunx commitlint --from origin/main --to HEAD

# Lint GitHub Actions workflows.
lint-actions:
    actionlint

# Audit GitHub Actions for unsafe triggers, permissions, and unpinned uses.
# Accepted findings, if any, are narrowly scoped in .github/zizmor.yml.
zizmor:
    zizmor --config .github/zizmor.yml .github/

# ---------------------------------------------------------------------------
# Testing (layered: unit -> integration -> chaos -> mutation -> fuzz)
# ---------------------------------------------------------------------------

# Run the hermetic default-feature test suite via nextest (all layers except
# mutation/fuzz). PureCARD's Legend and real-tokenizer features are exercised
# only through their explicit opt-in recipes below.
test:
    cargo nextest run --workspace

# Fast inner-loop: unit tests only (lib targets).
test-unit:
    cargo nextest run --workspace --lib

# Run the analyzer parser's focused lossless/recovery contract suite.
test-parser:
    cargo nextest run -p pure-analyzer-parser

# Run the analysis crate's focused relational and semantic contracts.
test-analysis:
    cargo nextest run -p pure-analyzer-analysis

# Run the CLI's focused process-boundary workflow suite.
test-cli:
    cargo nextest run -p pure-analyzer-cli

# Run the LSP's focused protocol transcript suite.
test-lsp:
    cargo nextest run -p pure-analyzer-lsp

# Run the libpure facade's focused driver contracts.
test-libpure:
    cargo nextest run -p libpure

# Replay the frozen Legend parser corpus without a running engine.
parser-differential-verify:
    cargo xtask parser-differential

# Validate the frozen corpus against an exactly pinned running Legend engine,
# then replay it locally. The refresher only updates an ignored response cache;
# committed verdicts remain immutable per corpus version.
parser-differential-refresh:
    cargo xtask parser-differential --refresh

# Run doctests explicitly: nextest does not execute them. All features remain
# compile-checked here; PureCARD's environment-bound feature tests are separate
# integration-test binaries, so they are not executed by this doc-only command.
doctest:
    cargo test --workspace --doc --all-features

# Integration tests (files under crates/*/tests/, compiled as separate
# `test`-kind binaries — `kind(test)` selects exactly those, unlike the
# `--test '*'` glob this used to use, which cargo takes as a literal binary
# name and fails to build when none exists). `--no-tests=warn` so this passes
# cleanly before any integration test exists yet; once real `tests/*.rs` files
# land, they run normally and this reverts to a hard gate.
test-integration:
    cargo nextest run --workspace -E 'kind(test)' --no-tests=warn

# Chaos / deterministic-simulation tests (turmoil/madsim), named `chaos_*`.
# Filtered by test-name substring so it works without a custom harness.
# `--no-tests=warn` so this passes cleanly before any chaos_* test exists yet.
test-chaos:
    cargo nextest run --workspace --all-features -E 'test(/chaos/)' --no-tests=warn

# Legend-backed PureCARD completeness lane (opt-in). xtask owns Compose startup,
# the package-scoped `legend` test invocation, and unconditional teardown.
test-legend:
    cargo xtask test-legend

# Run the Bun test suite for the .mjs automation under scripts/ (CI: test-scripts).
test-scripts:
    bun test scripts/

# Mutation testing verifies that the test suite actually catches regressions.
# The default workspace and feature-gated PureCARD FFI surface run separately
# so neither pass can succeed vacuously; xtask portably prepares their output.
# Full, unsharded — the local/ci-full entry point (CI itself shards the slow
# workspace pass across a matrix; see test-mutation-shard).
test-mutation:
    cargo xtask test-mutation

# One shard of the workspace-wide mutation pass (CI matrix only): `index` is
# zero-based, matching the mutation planner matrix's `index`/`total` fields.
test-mutation-shard index total:
    cargo xtask test-mutation-shard {{index}} {{total}}

# One verified merge-base-diff-scoped workspace mutation shard (CI only).
test-mutation-diff-shard index total diff:
    cargo xtask test-mutation-diff-shard {{index}} {{total}} {{quote(diff)}}

# Plan a fail-closed mutation matrix from the event-pinned pull-request diff.
plan-mutation:
    bun scripts/mutation-scope.mjs plan

# Recreate and verify the planner's exact diff in a mutation matrix worker.
prepare-mutation-diff:
    bun scripts/mutation-scope.mjs prepare

# Run the M3 parser's mutation pass in isolation while evolving parser
# contracts. This is an inner-loop aid only; the workspace-wide CI matrix
# remains the authoritative merge gate.
test-mutation-parser:
    cargo xtask test-mutation-parser

# The feature-gated PureCARD FFI-boundary mutation pass alone (fast; never
# sharded). Split out of `test-mutation` so CI can run it as its own job
# alongside the sharded workspace matrix.
test-mutation-ffi:
    cargo xtask test-mutation-ffi

# ---------------------------------------------------------------------------
# Fuzzing & benchmarking
# ---------------------------------------------------------------------------

# Run cargo-fuzz targets for a bounded time (default 60s per target).
# Pass a target name to fuzz just one, e.g. `just fuzz m3_parser`. Uses the nightly
# toolchain cargo-fuzz needs for the sanitizers; CI's fuzz-smoke job calls this.
# `triple` forces the build target: CI passes the gnu triple because a
# musl-installed cargo-fuzz (taiki-e's static binary) otherwise defaults to a
# musl target, whose static libc is incompatible with the ASAN sanitizer. Local
# devs omit it and get their native (gnu/darwin) host.
fuzz target="" time="60" triple="":
    cargo +nightly fuzz run {{ if triple == "" { "" } else { "--target " + triple } }} {{ target }} -- -max_total_time={{ time }}

# Run one target from PureCARD's separate, workspace-excluded fuzz project.
# The target is required so this can never accidentally select the analyzer's
# top-level fuzz crate. CI may pass the GNU triple explicitly for ASan.
purecard-fuzz target time="60" triple="":
    cargo +nightly fuzz run --fuzz-dir crates/pure-analyzer-purecard/fuzz {{ if triple == "" { "" } else { "--target " + triple } }} {{ target }} -- -max_total_time={{ time }}

# Compile every PureCARD fuzz target without executing it (bit-rot gate). CI
# supplies the GNU triple because the static installer otherwise selects musl,
# which is incompatible with ASan; local developers normally omit it.
purecard-fuzz-build triple="":
    cargo +nightly fuzz build --fuzz-dir crates/pure-analyzer-purecard/fuzz {{ if triple == "" { "" } else { "--target " + triple } }}

# Time-box all three PureCARD fuzz targets. The per-target loop and nested fuzz
# manifest selection live in xtask rather than shell control flow here.
purecard-fuzz-ci time="60":
    cargo xtask purecard-fuzz-ci {{ time }}

# Immutable real-tokenizer revisions and repo-root cache locations used by the
# opt-in PureCARD oracle recipes. Bump only deliberately and keep future CI in
# sync with these values.
purecard_qwen_revision := "c03e6d358207e414f1eca0bb1891e29f1db0e242"
purecard_qwen_tokenizer := justfile_directory() + "/target/purecard/qwen/tokenizer.json"
purecard_gpt4_revision := "1d9f1f1b1fae88c0e4df1dab0a397f8de6229075"
purecard_gpt4_tokenizer := justfile_directory() + "/target/purecard/gpt4/tokenizer.json"

# Fetch the pinned Qwen2.5-Coder tokenizer into the shared local/CI cache.
qwen-tokenizer-fetch:
    curl -sSL --fail --create-dirs -z {{ quote(purecard_qwen_tokenizer) }} -o {{ quote(purecard_qwen_tokenizer) }} "https://huggingface.co/Qwen/Qwen2.5-Coder-7B-Instruct/resolve/{{ purecard_qwen_revision }}/tokenizer.json"

# Fetch the pinned GPT-4 tokenizer used by the fused-precision fixture.
gpt4-tokenizer-fetch:
    curl -sSL --fail --create-dirs -z {{ quote(purecard_gpt4_tokenizer) }} -o {{ quote(purecard_gpt4_tokenizer) }} "https://huggingface.co/Xenova/gpt-4/resolve/{{ purecard_gpt4_revision }}/tokenizer.json"

# Fetch the pinned Qwen tokenizer and run PureCARD's real-tokenizer L2
# soundness oracle. Heavy and network-fed, so deliberately outside `test`/`ci`.
qwen-oracle: qwen-tokenizer-fetch
    just qwen-oracle-run

# Run the Qwen oracle from an already-populated cache (the CI-friendly entry
# point). `-p` prevents Cargo from applying the feature to another member.
qwen-oracle-run:
    QWEN_TOKENIZER_JSON={{ quote(purecard_qwen_tokenizer) }} cargo test -p pure-analyzer-purecard --features qwen-oracle --test qwen_soundness -- --nocapture

# Fetch both immutable byte-level BPE tokenizers used to verify the committed
# fused-navigation fixture. Each recipe uses `curl -z` to preserve fresh caches.
fused-tokenizers-fetch: qwen-tokenizer-fetch gpt4-tokenizer-fetch

# Re-extract the fixture from the real tokenizers and compare it with the
# committed hermetic replay data.
fused-tokenizers: fused-tokenizers-fetch
    just fused-tokenizers-run

# Run the fused-tokenizer comparison from already-populated caches.
fused-tokenizers-run:
    QWEN_TOKENIZER_JSON={{ quote(purecard_qwen_tokenizer) }} GPT4_TOKENIZER_JSON={{ quote(purecard_gpt4_tokenizer) }} cargo test -p pure-analyzer-purecard --features fused-extract --test fused_tokenizer_extract -- --nocapture

# Intentionally regenerate the committed fixture after a reviewed tokenizer or
# extractor change. The resulting diff must be inspected before commit.
fused-tokenizers-write: fused-tokenizers-fetch
    QWEN_TOKENIZER_JSON={{ quote(purecard_qwen_tokenizer) }} GPT4_TOKENIZER_JSON={{ quote(purecard_gpt4_tokenizer) }} WRITE_FUSED_FIXTURE=1 cargo test -p pure-analyzer-purecard --features fused-extract --test fused_tokenizer_extract -- --nocapture

# ---------------------------------------------------------------------------
# PureCARD real-model inference (issue #58)
# ---------------------------------------------------------------------------

# Pinned real-inference model (issue #58): Qwen2.5-Coder-0.5B-Instruct ships
# tokenizer.json byte-identical to the 7B revision already pinned above
# (verified at pin time), so every byte-vocab helper the Qwen tokenizer lanes
# already use carries over unchanged. Apache-2.0; the 0.5B size (~1 GiB) fits
# this project's CPU-only compute/CI budget — a larger sibling shares the same
# tokenizer but would need its own revision pin and budget review.
purecard_qwen_infer_model := "Qwen/Qwen2.5-Coder-0.5B-Instruct"
purecard_qwen_infer_revision := "ea3f2471cf1b1f0db85067f1ef93848e38e88c25"
purecard_qwen_infer_dir := justfile_directory() + "/target/purecard/qwen-infer"

# Fetch the pinned real-inference model weights + tokenizer into a local,
# gitignored cache (never committed — constitution §2's model-artifact rule).
[working-directory('crates/pure-analyzer-purecard')]
qwen-infer-model-fetch:
    uv run --locked --python 3.12 --no-managed-python --group real-model hf download {{ quote(purecard_qwen_infer_model) }} --revision {{ quote(purecard_qwen_infer_revision) }} --local-dir {{ quote(purecard_qwen_infer_dir) }}

# Drive real-model Python inference through PureCARD (issue #58): fetch the
# pinned weights, then run the harness alone (no Legend compile-check). Not
# part of `just test-python` / `just ci` — heavy and network-fed, like the
# Qwen tokenizer oracles above.
real-model-infer: qwen-infer-model-fetch
    just real-model-infer-run

# Run the real-model harness from an already-populated model cache (the
# CI-friendly entry point).
[working-directory('crates/pure-analyzer-purecard')]
real-model-infer-run:
    PURECARD_QWEN_INFER_DIR={{ quote(purecard_qwen_infer_dir) }} uv run --locked --python 3.12 --no-managed-python --group real-model python -m pytest python/tests/test_real_model_inference.py -v

# Full issue-#58 pipeline: real-model inference, then compile every completed
# constrained output through the live Legend stack. The harness runs first as
# a `just` dependency (xtask's shellouts are limited to the vetted
# cargo/git/buf/ast-grep/bun/docker allowlist — `just`/`uv` are not on it, so
# this step is sequenced here, not inside xtask); xtask then owns Compose
# startup, the compile-check invocation, and unconditional teardown (mirrors
# `test-legend`).
test-real-model: real-model-infer
    cargo xtask test-real-model

# Criterion benchmarks.
bench:
    cargo bench --workspace

# ---------------------------------------------------------------------------
# Coverage, supply-chain & API-stability gates
# ---------------------------------------------------------------------------

# Coverage report + floor enforcement (delegates the threshold logic to xtask).
coverage:
    cargo xtask coverage

# Advisory / vulnerability scan of the dependency tree.
audit:
    cargo audit

# License, ban, advisory and source policy enforcement.
deny:
    cargo deny check

# Verify committed dependency-review coverage without refreshing imports.
vet:
    cargo vet --locked

# Show why a third-party crate is present in the workspace graph.
tree crate:
    cargo tree -i {{ quote(crate) }}

# Unused-dependency scan.
machete:
    cargo machete

# Assert that PureCARD's non-optional shipped dependency set remains the three
# ADR-approved runtime crates. The migrated crate is unpublished, so this does
# not restore the standalone repository's obsolete package-content allowlist.
check-core-deplight:
    cargo xtask check-core-deplight

# Verify PureCARD's copied documentation facts against its nested sources,
# tests, and corpora so similarly named analyzer facts cannot contaminate them.
check-doc-facts:
    cargo xtask check-doc-facts

# Generate the user-facing diagnostic/reason reference from the shared explain
# catalog. Commit the resulting product documentation with catalog changes.
generate-explain-docs:
    cargo xtask generate-explain-docs

# Fail when a registered explain identifier lacks its reference page, content
# changes without its page, or an orphan page remains tracked.
check-explain-docs:
    cargo xtask check-explain-docs

# Check every tracked Markdown relative file and GitHub-style heading anchor.
check-doc-links:
    cargo xtask check-doc-links

# Validate release-plz.toml against the workspace, so config drift fails a PR
# instead of the post-merge trunk run. Delegates to xtask.
release-plz-check:
    cargo xtask release-plz-check

# Semantic-versioning check for the public API of the libraries.
semver:
    cargo semver-checks check-release --workspace

# Verify every public Rust crate's all-features API against its committed
# baseline under public-api/ (needs a nightly toolchain). The inventory is
# exact: missing or stale baselines fail closed. Run `just public-api-bless`
# after an intended API change, then review and commit the result.
public-api:
    cargo xtask public-api

# Refresh the committed public-API baselines after an intended change.
public-api-bless:
    cargo xtask public-api --bless

# ---------------------------------------------------------------------------
# Docs
# ---------------------------------------------------------------------------

# Build docs with warnings denied (missing-docs is a hard error in libs).
docs:
    RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features

# ---------------------------------------------------------------------------
# PureCARD Python boundary (PyO3 + maturin)
# ---------------------------------------------------------------------------

# Type-check only PureCARD's feature-gated PyO3 boundary.
check-ffi:
    cargo check -p pure-analyzer-purecard --features python

# Exercise the Rust side of the PyO3 boundary so mutation testing can observe
# its marshaling delegates without rebuilding a wheel for every mutant.
test-ffi:
    cargo test -p pure-analyzer-purecard --features python-test --lib

# Build PureCARD's abi3 wheel. Run from the nested crate so maturin discovers
# its pyproject.toml rather than looking for one at the workspace root.
[working-directory('crates/pure-analyzer-purecard')]
wheel:
    maturin build --release --features python

# Build/install the extension in uv's project-local environment, then run the
# pinned hermetic Python tests. No pre-activated virtualenv is required.
# `--ignore`s the real-model harness test (issue #58): it imports `torch`/
# `transformers`, which the `test` dependency group deliberately excludes to
# keep this lane network- and heavy-dependency-free, so collecting it here
# would fail every PR touching PureCARD's Python surface, not just skip it
# quietly. That file runs only via `just real-model-infer`/`test-real-model`.
[working-directory('crates/pure-analyzer-purecard')]
test-python:
    uv run --locked --python 3.12 --no-managed-python --group test python -m pytest python/tests --ignore=python/tests/test_real_model_inference.py

# ---------------------------------------------------------------------------
# Structural / hygiene checks
# ---------------------------------------------------------------------------

# ast-grep structural rules (banned constructs, architecture guardrails).
sweep:
    cargo xtask sweep

# Reject postponed-work markers (TODO/FIXME/XXX/#[ignore]) across all tracked
# Rust sources — the CI structural gate. The pre-commit hook scans staged lines.
postponed-markers:
    bun scripts/checks/postponed-markers.mjs --all

# Reject the retired checked-in work ledger. Change scope and progress belong in
# GitHub Issues; PRs record implementation evidence. Durable product references
# remain source material.
no-work-ledger:
    bun scripts/checks/no-work-ledger.mjs

# Reject tracked shell files and shell shebangs: repository automation is just, xtask, or Bun.
no-shell-scripts:
    bun scripts/checks/no-shell-scripts.mjs

# Reject stale milestone/scaffold self-description in shipped PureCARD source
# docs. The restored scanner is monorepo-aware and intentionally crate-scoped.
lint-purecard-stale:
    bun scripts/checks/stale-selfdescription.mjs --all

# Re-label PureCARD's frozen differential corpus against a running Legend
# engine. The script owns the nested paths and verifies the engine version pin.
label-differential:
    bun scripts/label-differential.mjs

# Verify analyzer layering (ADR-0003) and analyzer/PureCARD independence
# (ADR-0004 and PureCARD ADR-0009) across normal/dev/build dependencies.
# Delegates to xtask (reads `cargo metadata`). Also runs inside `just ci`.
verify-layering:
    cargo xtask verify-layering

# Verify the workspace lint contract (constitution §1.2/§1.3): the root declares
# `[workspace.lints.rust]` with forbid-unsafe / deny-missing-docs, and every
# member inherits it via `[lints] workspace = true`. Kills the class where a
# crate silently omits the attribute. Delegates to xtask. Also runs in `just ci`.
verify-lints:
    cargo xtask verify-lints

# Local pre-PR hygiene gate: structural rules + unused deps + secret scan.
# `review` runs the same underlying tools CI does, so it fails fast locally.
review: sweep
    cargo machete
    gitleaks detect --no-banner --redact

# ---------------------------------------------------------------------------
# Feature scaffolding
# ---------------------------------------------------------------------------

# Create an isolated git worktree + branch `feature/<name>` for a change.
# One worktree per branch keeps parallel work from stepping on each other.
new-feature name:
    cargo xtask new-feature {{ name }}

# ---------------------------------------------------------------------------
# Aggregate / meta targets
# ---------------------------------------------------------------------------

# The fast inner-loop gate: layering + fmt-check + clippy + test. This is the
# necessary-but-not-sufficient pre-PR check; the full gate is CI (see ci-full).
ci: no-work-ledger
    cargo xtask ci

# The full local gate: every PR-blocking CI gate, chained in CI's job order,
# fail-fast, reusing the same targets CI runs. Slow (coverage + mutation +
# supply-chain + API snapshots + docs + scripts). Two CI gates are
# environment-bound and cannot run faithfully here — the no-warnings log sweep
# (reads the run's own Actions logs) and the fuzz-smoke (needs nightly for
# cargo-fuzz's sanitizers) — so they are only enforced in CI. Run the relevant fuzz-smoke directly with `just fuzz <target>
# 60` if you have nightly. Use before a PR when a change touches what the fast
# gate skips.
ci-full: ci coverage test-mutation deny audit vet machete release-plz-check semver public-api sweep no-shell-scripts postponed-markers docs test-scripts lint-actions zizmor
    @echo "ci-full: ran every locally reproducible PR gate; the no-warnings log sweep and fuzz-smoke are enforced only in CI"

# Install git hooks (managed by lefthook.yml). Also run automatically by the
# `install-cargo-tools` onboarding step, so a fresh clone is never left unwired;
# this target is the manual re-install.
hooks-install:
    lefthook install

# NOTE: there is deliberately no `setup` target. Environment bootstrap was a
# one-time, self-deleting agent runbook; the kit is already bootstrapped.
# Re-provisioning a tool is just: `mise install && mise run install-cargo-tools`.
