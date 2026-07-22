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

# Clippy with warnings denied across all targets and features.
lint:
    cargo clippy --all-targets --all-features -- -D warnings

# Lint + auto-fix markdown (aligns tables for MD060, then markdownlint --fix).
lint-md:
    bun scripts/lib/align-md-tables.mjs $(git ls-files '*.md')
    bunx markdownlint-cli2 --fix "**/*.md"

# Verify commit messages on this branch follow Conventional Commits.
lint-commits:
    bunx commitlint --from origin/main --to HEAD

# Lint GitHub Actions workflows.
lint-actions:
    actionlint

# ---------------------------------------------------------------------------
# Testing (layered: unit -> integration -> chaos -> mutation -> fuzz)
# ---------------------------------------------------------------------------

# Run the full test suite via nextest (all layers except mutation/fuzz).
test:
    cargo nextest run --workspace --all-features

# Fast inner-loop: unit tests only (lib targets).
test-unit:
    cargo nextest run --workspace --lib

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

# Run the Bun test suite for the .mjs automation under scripts/ (CI: test-scripts).
test-scripts:
    bun test scripts/

# Mutation testing — verifies the test suite actually catches regressions.
# Runs in-place (mutates the checked-out tree directly, reverting after each
# trial) for speed on both CI's disposable checkout and a developer's own tree.
test-mutation:
    cargo mutants --workspace --in-place

# ---------------------------------------------------------------------------
# Fuzzing & benchmarking
# ---------------------------------------------------------------------------

# Run cargo-fuzz targets for a bounded time (default 60s per target).
# Pass a target name to fuzz just one, e.g. `just fuzz diagnostics`. Uses the nightly
# toolchain cargo-fuzz needs for the sanitizers; CI's fuzz-smoke job calls this.
# `triple` forces the build target: CI passes the gnu triple because a
# musl-installed cargo-fuzz (taiki-e's static binary) otherwise defaults to a
# musl target, whose static libc is incompatible with the ASAN sanitizer. Local
# devs omit it and get their native (gnu/darwin) host.
fuzz target="" time="60" triple="":
    cargo +nightly fuzz run {{ if triple == "" { "" } else { "--target " + triple } }} {{ target }} -- -max_total_time={{ time }}

# Criterion benchmarks. On CI these run under CodSpeed (see ci.yml).
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

# Unused-dependency scan.
machete:
    cargo machete

# Validate release-plz.toml against the workspace, so config drift fails a PR
# instead of the post-merge trunk run. Delegates to xtask.
release-plz-check:
    cargo xtask release-plz-check

# Semantic-versioning check for the public API of the libraries.
semver:
    cargo semver-checks check-release --workspace

# Verify each public crate's API against its committed baseline under
# public-api/ (needs a nightly toolchain). Run `just public-api-bless` after an
# intended API change to refresh the baselines.
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
# Structural / hygiene checks
# ---------------------------------------------------------------------------

# ast-grep structural rules (banned constructs, architecture guardrails).
sweep:
    cargo xtask sweep

# Reject postponed-work markers (TODO/FIXME/XXX/#[ignore]) across all tracked
# Rust sources — the CI structural gate. The pre-commit hook scans staged lines.
postponed-markers:
    bun scripts/checks/postponed-markers.mjs --all

# Verify the workspace layering (constitution §1, ADR-0002): reject any layer
# that depends outward — onto a sibling or outer layer — in any dependency kind
# (normal/dev/build), the edge cargo-deny's global bans miss. Delegates to xtask
# (reads `cargo metadata`). Also runs inside `just ci`.
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
# Feature / spec scaffolding
# ---------------------------------------------------------------------------

# Create an isolated git worktree + branch `feature/<name>` for a change.
# One worktree per branch keeps parallel work from stepping on each other.
new-feature name:
    cargo xtask new-feature {{ name }}

# Scaffold a feature spec at specs/<name>.md from the template.
spec name:
    cargo xtask spec {{ name }}

# ---------------------------------------------------------------------------
# Aggregate / meta targets
# ---------------------------------------------------------------------------

# The fast inner-loop gate: layering + fmt-check + clippy + test. This is the
# necessary-but-not-sufficient pre-PR check; the full gate is CI (see ci-full).
ci:
    cargo xtask ci

# The full local gate: every PR-blocking CI gate, chained in CI's job order,
# fail-fast, reusing the same targets CI runs. Slow (coverage + mutation +
# supply-chain + docs + scripts). Four CI gates are environment-bound and cannot
# run faithfully here — the CodSpeed bench (needs the CodSpeed service), the
# no-warnings log sweep (reads the run's own Actions logs), the opt-in public-api
# snapshot (needs nightly + committed baselines), and the fuzz-smoke (needs
# nightly for cargo-fuzz's sanitizers) — so they are only enforced in CI. Run the
# fuzz-smoke directly with `just fuzz diagnostics 60` if you have nightly. Use
# before a PR when a change touches what the fast gate skips.
ci-full: ci coverage test-mutation deny audit machete release-plz-check semver sweep postponed-markers docs test-scripts
    @echo "ci-full: ran every locally reproducible PR gate; codspeed bench, the no-warnings log sweep, the opt-in public-api snapshot, and the fuzz-smoke are enforced only in CI"

# Install git hooks (managed by lefthook.yml). Also run automatically by the
# `install-cargo-tools` onboarding step, so a fresh clone is never left unwired;
# this target is the manual re-install.
hooks-install:
    lefthook install

# NOTE: there is deliberately no `setup` target. Environment bootstrap was a
# one-time, self-deleting agent runbook; the kit is already bootstrapped.
# Re-provisioning a tool is just: `mise install && mise run install-cargo-tools`.
