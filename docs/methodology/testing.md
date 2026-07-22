# Methodology: Testing

Testing here is first-class and layered. Each layer catches a different class of
defect, gets progressively more expensive, and feeds a hard gate. The rules are
PROTECTED: **no test is skipped, and flakes are fixed, never silenced.**

The test runner is [`cargo-nextest`](https://nexte.st/) throughout.

## The pyramid

From cheapest and most numerous at the base to slowest and fewest at the top.

### 1. Unit tests

Pure, fast, in-crate. The engine crates (`pure-analyzer-lexer`, `-syntax`,
`-parser`, `-model`, `-resolve`, `-analysis`, `-diagnostics`) — no I/O, no async —
should be almost entirely covered here. Push logic down into them precisely so it
can be tested this cheaply. Make illegal states unrepresentable so there's less to
test.

### 2. Integration tests — CLI/LSP over fixture inputs

pure-analyzer has no databases, queues, or other backing services to spin up —
its only "real dependency" is its own input surface: `.pure` source files and a
model (PMCD JSON or a Pure model file, design doc §7). Integration tests invoke
the real `pure-analyzer` binary (or the LSP session, once it exists in v0.2)
against fixture `.pure` files + models under `tests/corpus/` and `tests/golden/`
(design doc §3) and assert on the emitted `Diagnostic`s/exit code — no mocking the
parser or resolver.

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
targets in the `fuzz/` crate, hammering untrusted input — the parsers and decoders
at the edges — with inputs no human would think to write. Each target asserts the
invariants the input must never break (see `fuzz/fuzz_targets/`). Run one locally
with `just fuzz <target>` (a nightly toolchain is required for the sanitizers). CI
runs a **bounded fuzz-smoke** job — `cargo fuzz run <target> -- -max_total_time=60`
over a corpus restored from the Actions cache — on every code-changing PR, so a
newly reachable crash reddens the PR instead of waiting for a periodic run.
Long-horizon continuous fuzzing (e.g. OSS-Fuzz) is a future addition, not
something this template ships today.

### 6. End-to-end

A thin top layer: the compiled `pure-analyzer` binary invoked as a subprocess over
its real CLI surface (all five subcommands, `--format json`/`--format human`,
exit codes) and — once it exists in v0.2 — the LSP server driven over real
stdio JSON-RPC. Few, slow, high-value — the smoke that proves the wiring.

## The gates around the pyramid

Testing isn't only "does it pass." Several gates run alongside:

- **Coverage floor** — [`cargo-llvm-cov`](https://github.com/taiki-e/cargo-llvm-cov).
  A PROTECTED minimum line/branch coverage.
- **Mutation floor** — `cargo-mutants` score must stay above a PROTECTED threshold.
- **Performance regression** — [`criterion`](https://docs.rs/criterion) benchmarks
  gated by [CodSpeed](https://codspeed.io/) (free for OSS), which reports per-PR
  deltas. This protects the high-performance goal from silent erosion. **Off by
  default:** the CodSpeed job is gated on the repo variable `CODSPEED_ENABLED`, so
  until a deriver installs the CodSpeed app and sets it, perf regressions ship
  **unprotected** — see the [optional-gates checklist](../../README.md#optional-gates-off-by-default).
- **API stability** — [`cargo-semver-checks`](https://github.com/obi1kenobi/cargo-semver-checks)
  runs on every PR; [`cargo-public-api`](https://github.com/enselic/cargo-public-api)
  snapshots the exact public surface and [`insta`](https://insta.rs/) pins
  serialized outputs so changes to them are deliberate and reviewed. **Off by
  default:** the `cargo-public-api` snapshot is gated on the repo variable
  `PUBLIC_API_ENABLED` (it needs a nightly toolchain and committed baselines from
  `just public-api-bless`), so until a deriver enables it the public API surface
  ships **unprotected** — see the [optional-gates checklist](../../README.md#optional-gates-off-by-default).
- **Diagnostic-code stability** — a `PUR<nnnn>` code, once shipped, is a contract
  with editor/CI integrations that key off it; retiring or repurposing one is a
  breaking change and belongs in a spec, not a drive-by rename.

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
- **Independent recomputation in CI.** Every threshold and gate value is
  recomputed by CI from PROTECTED config the agent can't edit. The agent may raise
  a floor; it may never lower one. See [self-learning.md](self-learning.md).

## Running it

```sh
just test        # nextest across the workspace
just ci          # the fast gate: layering + fmt + clippy + test
just ci-full     # the full local mirror of the CI matrix
```

`just ci` is the fast pre-PR gate — necessary but not sufficient. The heavier
gates — coverage (`just coverage`), mutation (`just test-mutation`), the
structural sweep (`just sweep`), and the supply-chain audits (`just deny` /
`just audit` / `just machete`) — are separate targets that run as their own
parallel CI jobs. Run the one that guards what you changed, or run them all at
once with `just ci-full`, which chains every PR-blocking gate in CI's order
(reporting the few environment-bound gates it can't reproduce locally).

If you need a finer-grained target than exists, add it to the `justfile` — `just`
is the frontend, and a missing target is a bug in the frontend.
