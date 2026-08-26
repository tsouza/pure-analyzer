# Spec: reconcile-purecard-ci-policy

- Status: complete
- Created: 2026-08-26
- Owner: agent (continuation of the user-approved PureCARD fold)

## Problem

PureCARD's structural migration and local automation are restored, but the
umbrella repository lacks its specialized CI lanes. Existing workflows also
reference floating action tags, the policy toolchain has no zizmor gate, and
repository templates and labels describe only the analyzer product. Copying
the standalone workflows verbatim would target obsolete paths and, more
seriously, restore release-triggered crates.io and PyPI publication even though
the migrated crate and wheel are intentionally unpublished.

## Goals

- [x] Restore scheduled and on-demand real-tokenizer validation using immutable
  Qwen and GPT-4 revisions with cache-before-fetch behavior.
- [x] Restore PureCARD's separate three-target fuzz matrix for pull requests,
  merge queue entries, nightly runs, and manual runs, including nested-workspace
  caches and crash artifacts.
- [x] Build, test, and upload an abi3 Python wheel on relevant changes without
  any release trigger, publishing job, package token, or write permission.
- [x] Pin every external GitHub Action to an immutable commit and enforce that
  posture with zizmor locally and in CI.
- [x] Add seven-day Dependabot cooldowns and reconcile labels and issue/PR
  templates for PureCARD, corpus, and Python-boundary changes.
- [x] Keep every CI tool reachable through a matching `just` recipe and add
  Python setup wherever all-feature PyO3 compilation requires it.

## Non-goals

- Running the live Legend completeness lane in hosted CI. It remains an
  explicit local/environment lane because it requires a pinned engine stack.
- Publishing the PureCARD Rust crate or Python wheel. Artifact construction is
  verification, not release authorization.
- Changing branch protection, rulesets, repository secrets, or other
  repository-administration settings.
- Changing decoder behavior, parser integration, analyzer dependencies, or
  umbrella governance. Those product boundaries belong to PR4.
- Deleting or archiving PureCARD's old standalone repository.

## Design

Three PureCARD-prefixed workflows avoid colliding with the analyzer's existing
lanes. The tokenizer workflow runs only on a schedule or manual dispatch. Each
tokenizer cache key includes an immutable upstream revision, and downloads
occur only on cache misses through the same `just` recipes used locally.

The PureCARD fuzz workflow treats
`crates/pure-analyzer-purecard/fuzz/` as its own Cargo workspace. A one-time
build job compiles every target for the GNU triple, then a matrix time-boxes
`accept_token`, `allowed_mask`, and `schema_from_json` for 60 seconds on change
events or 900 seconds nightly. Per-target corpora are restored and saved under
fresh run keys; failures upload crash artifacts.

The wheel workflow path-filters relevant Rust and packaging files. One combined
job executes the locked Python tests, uses the pinned maturin action to build an
abi3-py39 wheel, and uploads it as a workflow artifact. There is no release event
or downstream publishing job.

All third-party actions use reviewed commit SHAs with version comments.
`.github/zizmor.yml` records only narrow, behavior-required exceptions. The
lint workflow runs zizmor 1.29.0, matching `.mise.toml` and `just zizmor`.

## API / contract impact

There are no shipped Rust or Python API changes. The new contracts are CI
checks, workflow artifacts, immutable action pins, the local `just zizmor`
command, and repository metadata that routes PureCARD, corpus, and Python
changes accurately.

## Verification

- `actionlint .github/workflows/*.yml`
- `mise exec zizmor@1.29.0 -- zizmor -qq --offline --strict-collection
  --no-progress --config .github/zizmor.yml .github/` (zero unaccepted findings)
- `bun test scripts/`
- `markdownlint-cli2 '**/*.md'`
- `just --list`
- `just ci`
- `git diff --check`

Manual policy inspection also confirmed that the wheel lane has no release
trigger, registry credential, publish command, or write permission; every
external `uses:` reference is SHA-pinned; and each new tool or lane is exposed
through `just`.

## Risks and rollout

The highest risk is accidentally broadening release authority. The wheel lane
therefore has no release trigger or publish job at all, and PureCARD remains
`publish = false`. Network-fed tokenizers are isolated from per-PR CI and
pinned by revision. Fuzz cost is bounded by path filters, event-specific
budgets, and concurrency cancellation. SHA comments keep automated update PRs
reviewable without weakening the immutable-use gate.
