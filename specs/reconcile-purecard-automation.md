# Spec: reconcile-purecard-automation

- Status: complete
- Created: 2026-08-25
- Owner: agent (continuation of the user-approved PureCARD fold)

## Problem

The structural migration placed PureCARD at
`crates/pure-analyzer-purecard/`, but deliberately left its developer
automation behind for a second PR. The crate consequently documents commands
that do not exist, its opt-in Legend/tokenizer/Python/fuzz lanes have no
monorepo-aware entry points, and its dependency/doc anti-drift gates no longer
run. Copying the standalone repository's commands verbatim would target the
wrong package and paths, collide with pure-analyzer's own fuzz crate, and
restore a packaging assertion that no longer applies now that the migrated
crate is intentionally unpublished.

## Goals

- [x] Restore `just` entry points for PureCARD's hermetic checks, live-Legend
      lane, real-tokenizer oracles, Python boundary, differential labeler, and
      dedicated fuzz crate, all scoped to `pure-analyzer-purecard`.
- [x] Restore `cargo xtask check-core-deplight` and `check-doc-facts`, adapted
      to the nested crate. The dependency gate checks the shipped runtime graph;
      it does not reintroduce the standalone crate's obsolete package-content
      allowlist after `publish = false`.
- [x] Make the normal `just test` and `just ci` paths hermetic: optional
      `legend`, `qwen-oracle`, and `fused-extract` tests execute only through
      their explicit opt-in targets, while all features remain compile-checked.
- [x] Keep pure-analyzer's top-level fuzz target and PureCARD's three fuzz
      targets independently addressable, with no working-directory ambiguity.
- [x] Restore the pinned local Python/maturin toolchain only after checking the
      current stable versions from authoritative tool metadata.
- [x] Unit-test path/manifest parsing and anti-drift logic; run the restored
      offline gates plus `just ci` cleanly.

## Non-goals

- GitHub workflow, Dependabot, labeler, zizmor, and remaining policy
  reconciliation; those form PR 3 after these local entry points exist.
- Publishing a PureCARD wheel or Rust crate. A later workflow may build wheels,
  but an umbrella release must never implicitly publish PureCARD.
- Updating the umbrella constitution, domain model, ADR set, or README; that is
  the recorded PR 4.
- Sharing parser code or making PureCARD depend on `libpure`. That functional
  integration waits until the resilient parser exists and requires its own ADR.
- Repairing PureCARD's pre-existing, separately tooled Dylint crate. Its
  inherited workspace fields were already unusable while excluded in the
  standalone repository, and resolving its pinned nightly/`clippy_utils`
  contract is independent of restoring the working commands audited here.
- Deleting or archiving the old PureCARD repository.

## Design

`just` remains the only user-facing automation surface. Single-tool invocations
stay as recipes; orchestration with teardown or per-target loops lives in
`xtask`.

`xtask` owns constants for the nested PureCARD root, manifest, corpus, docs, and
fuzz manifest. The Legend task starts the checked-in Compose stack, runs only
the PureCARD `legend` feature lane, and always tears the stack down. The fuzz
task passes the nested fuzz manifest explicitly. The core dependency gate reads
Cargo metadata and admits only the three non-optional runtime dependencies
recorded by PureCARD ADR-0005. The doc-fact gate reads only PureCARD's copied
docs, sources, tests, and corpora, preventing similarly named analyzer facts
from contaminating its counts.

The real-tokenizer downloads remain opt-in and cache into `target/purecard/` at
immutable upstream revisions. Python recipes point maturin at the nested
manifest and pytest at the nested test directory. Rust-side PyO3 tests use a
test-only feature that embeds CPython, while maturin sets extension-module mode
only during packaging. Mutation testing runs the default workspace separately
from the feature-gated FFI file so neither surface can pass vacuously.

## API / contract impact

No shipped Rust or Python API changes. New contracts are developer commands:
`check-core-deplight`, `check-doc-facts`, `test-legend`, `purecard-fuzz`,
`purecard-fuzz-build`, `purecard-fuzz-ci`, `qwen-oracle`,
`fused-tokenizers`, `check-ffi`, `test-ffi`, `wheel`, `test-python`,
`lint-purecard-stale`, and `label-differential`.

## Testing plan

- Unit tests for dependency classification, nested path selection, source-tree
  comparison, and every doc-fact parser brought into `xtask`.
- Bun tests for the restored stale-self-description scanner.
- Rust-side mutation-sensitive tests for the feature-gated PyO3 boundary.
- `just check-core-deplight`, `just check-doc-facts`,
  `just lint-purecard-stale`, `just test-ffi`, and `just test-scripts`.
- `just ci`, followed by all locally reproducible full gates affected by the
  change. Live Legend, real-tokenizer downloads, Python environment mutation,
  and fuzz execution remain explicit environment-bound verification lanes.

## Verification

- `just ci`: 359/359 default-feature workspace tests plus all-feature doctests.
- `just test-ffi`: 175/175 PureCARD library tests with the embedded PyO3
  boundary enabled.
- `cargo test -p xtask`: 37/37 automation tests; `bun test scripts/`: 33/33
  script tests.
- `just purecard-fuzz-ci 1`: all three nested fuzz targets completed their
  bounded smoke runs; `just test-python`: 10/10 tests under Python 3.12.14 and
  pytest 8.4.2; `just wheel`: abi3-py39 wheel built successfully.
- Every locally reproducible `just ci-full` component passed. Mutation testing
  classified 499 default-workspace and 21 PyO3-boundary mutants with zero
  survivors or timeouts.
- Dependency, doc-fact, FFI, stale-description, release, deny, machete,
  postponed-marker, docs, structural, lockfile, Markdown, and diff checks pass.
- Live Legend and the network-fed Qwen/fused-tokenizer oracles were not run;
  their restored commands remain deliberately opt-in.

## Risks & rollout

The main risk is accidentally running an environment-bound feature in normal
CI. Default-feature test commands and package-scoped opt-in recipes keep that
boundary explicit. The second risk is path drift after future crate moves;
central xtask constants and tests make nested locations a single auditable
contract. Rollback is one cohort: remove the restored recipes/subcommands,
scripts, pins, and this spec together.
