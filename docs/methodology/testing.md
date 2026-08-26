# Methodology: Testing

Testing here is first-class and layered. Each layer catches a different class of
defect, gets progressively more expensive, and feeds a hard gate. The rules are
PROTECTED: **no test is skipped, and flakes are fixed, never silenced.** The
root policy governs both independent products; each product keeps its own test
assets and guarantee boundary.

Rust tests run through [`cargo-nextest`](https://nexte.st/). PureCARD also has
specialized Python, corpus, tokenizer, Legend-oracle, fuzz, and wheel lanes,
reached through root `just` recipes without becoming analyzer tests.

## The pyramid

From cheapest and most numerous at the base to slowest and fewest at the top.

### 1. Unit tests

Pure, fast, in-crate. Analyzer crates follow `lexer → syntax → parser →
model → resolve → analysis`, with diagnostics as a leaf; core logic should be
almost entirely covered here. PureCARD independently tests its PDA, grammar,
schema overlay, cache, and boundary types in its own crate. Push logic down so it
can be tested this cheaply. Make illegal states unrepresentable so there is less
to test.

### 2. Integration tests — CLI/LSP over fixture inputs

The analyzer has no databases, queues, or other backing services. Its integration
surface is `.pure` source plus a model (PMCD JSON or a Pure model file). Tests
invoke implemented analyzer interfaces against analyzer-owned fixtures and assert
on emitted diagnostics and exit codes without mocking parser or resolver.

PureCARD's shipped integration tests replay its own offline gold corpus, schemas,
tokenizer fixtures, and Python boundary. Its optional live Legend oracle remains
a PureCARD lane. Those assets are not an analyzer corpus merely because both
products live in one repository.

### 3. Chaos / deterministic simulation testing (DST)

Concurrency and failure-injection testing under a **deterministic** scheduler —
[`turmoil`](https://docs.rs/turmoil) or [`madsim`](https://docs.rs/madsim). The
determinism is the point: on a failure, **capture the seed**, so any failure
reproduces exactly. CI runs a **multi-seed** sweep to explore the schedule space;
a failing seed is committed as a regression test.

### 4. Mutation testing — cargo-mutants

[`cargo-mutants`](https://mutants.rs/) perturbs the code and checks the tests
notice. This measures whether tests actually *assert*, not just execute. The
mutation score is a **PROTECTED floor** (see anti-gaming below).

### 5. Fuzzing — cargo-fuzz / libFuzzer

[`cargo-fuzz`](https://github.com/rust-fuzz/cargo-fuzz) drives
[`libFuzzer`](https://llvm.org/docs/LibFuzzer.html) (via `libfuzzer-sys`) over the
targets in the analyzer and nested PureCARD fuzz workspaces, hammering untrusted
inputs at their respective edges with inputs no human would think to write. Each
target asserts invariants the input must never break. Root `just` recipes select
the correct workspace and CI runs bounded, cached fuzz jobs. Long-horizon
continuous fuzzing such as OSS-Fuzz is a future option, not a current service.

### 6. End-to-end

A thin top layer exercises each product through its externally exposed interface.
PureCARD exercises its Rust decoder and optional PyO3 boundary end to end in its
own suites. Keep product tests independent even when root CI runs them in one
matrix.

## The gates around the pyramid

Testing isn't only "does it pass." Several gates run alongside:

- **Coverage floor** — [`cargo-llvm-cov`](https://github.com/taiki-e/cargo-llvm-cov).
  A PROTECTED minimum line/branch coverage.
- **Mutation floor** — `cargo-mutants` score must stay above a PROTECTED threshold.
- **Performance regression** — [`criterion`](https://docs.rs/criterion) benchmarks
  gated by [CodSpeed](https://codspeed.io/) (free for OSS), which reports per-PR
  deltas. This protects the high-performance goal from silent erosion. **Off by
  default:** the CodSpeed job is gated on the repo variable `CODSPEED_ENABLED`, so
  until a repository maintainer installs the CodSpeed app and sets it,
  performance regressions ship
  **unprotected** — see the [optional performance gate](../../README.md#optional-performance-gate-off-by-default).
- **API stability** — [`cargo-semver-checks`](https://github.com/obi1kenobi/cargo-semver-checks)
  runs on every PR; [`cargo-public-api`](https://github.com/enselic/cargo-public-api)
  snapshots every public Rust crate's all-features surface. The committed
  baseline inventory is exact: a missing or stale file fails the PR. Intended
  changes use `just public-api-bless`; the resulting diff is reviewed and
  committed with the implementation change.
- **Diagnostic-code stability** — a `PUR<nnnn>` code, once shipped, is a contract
  with editor/CI integrations that key off it; retiring or repurposing one is a
  breaking change and belongs in a linked issue, not a drive-by rename.

## Zero tolerance for flakes and skips

- **No skipping.** `#[ignore]`, disabled tests, and skip markers are forbidden by
  an L1 gate. Removing a test to make CI pass is the same offense.
- **Flakes are bugs.** A test that fails intermittently is fixed *immediately* —
  by fixing the test or the code, never by weakening or deleting the assertion,
  and never by adding a retry that hides the nondeterminism. For DST failures, the
  captured seed makes "it's flaky" reproducible, so there's no excuse to defer.

## Anti-gaming

An agent optimizing to pass tests will, unchecked, learn to defeat them. The
countermeasures:

- **Randomized-seed property tests** the agent can't hardcode: the harness draws
  seeds it doesn't control, so "memorize the expected output" doesn't work.
- **A reviewer-authored held-out suite** the generator never sees during
  implementation — an independent check on whether the code really works.
- **A capped-score suspicion signal**: a suite that scores suspiciously *perfectly*
  is itself a flag for the reviewer, on top of `cargo-mutants` catching
  assertion-free tests.
- **Recomputation in CI.** CI evaluates each threshold and gate from the submitted
  branch and runs anti-gaming tests for mechanically detectable weakening. The
  agent may raise a floor; lowering one requires an explicit maintainer decision.
  See [self-learning.md](self-learning.md).

## Running it

```sh
just test        # nextest across the workspace
just ci          # the fast gate: layering + fmt + clippy + test
just ci-full     # the full local mirror of the CI matrix
```

`just ci` is the fast pre-PR gate — necessary but not sufficient. The heavier
gates — coverage (`just coverage`), mutation (`just test-mutation`), the
structural sweep (`just sweep`), and the supply-chain audits (`just deny` /
`just audit` / `just vet` / `just machete`) — are separate targets that run as
their own parallel CI jobs. Run the one that guards what you changed, or run
them all at once with `just ci-full`, which chains every PR-blocking gate in
CI's order (reporting the few environment-bound gates it can't reproduce
locally).

If you need a finer-grained target than exists, add it to the `justfile` — `just`
is the frontend, and a missing target is a bug in the frontend.
