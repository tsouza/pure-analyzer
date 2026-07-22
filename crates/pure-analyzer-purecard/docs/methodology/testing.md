# Methodology: Testing

Testing here is first-class and layered. Each layer catches a different class of
defect, gets progressively more expensive, and feeds a hard gate. The rules are
PROTECTED: **no test is skipped, and flakes are fixed, never silenced.**

The test runner is [`cargo-nextest`](https://nexte.st/) throughout.

## The pyramid

From cheapest and most numerous at the base to slowest and fewest at the top.

### 1. Unit tests

Pure, fast, in-crate. The `domain` crate — no I/O, no async — should be almost
entirely covered here. Push logic down into `domain` precisely so it can be tested
this cheaply. Make illegal states unrepresentable so there's less to test.

### 2. Integration tests — testcontainers

Exercise real adapters against real dependencies (databases, brokers) spun up in
throwaway containers via [`testcontainers`](https://docs.rs/testcontainers). No
mocks standing in for infrastructure whose behavior we depend on.

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

### 5. Fuzzing — bolero + OSS-Fuzz

[`bolero`](https://docs.rs/bolero) unifies property and fuzz testing; OSS-Fuzz
runs continuous fuzzing for free on OSS. This is the layer that hammers untrusted
HTTP/gRPC input — the parsers and decoders at the edges — with inputs no human
would think to write.

### 6. End-to-end

A thin top layer: the assembled server exercised over its real HTTP and gRPC
surfaces. Few, slow, high-value — the smoke that proves the wiring.

## The gates around the pyramid

Testing isn't only "does it pass." Several gates run alongside:

- **Coverage floor** — [`cargo-llvm-cov`](https://github.com/taiki-e/cargo-llvm-cov).
  A PROTECTED minimum line/branch coverage.
- **Mutation floor** — `cargo-mutants` score must stay above a PROTECTED threshold.
- **Performance regression** — [`criterion`](https://docs.rs/criterion) benchmarks
  gated by [CodSpeed](https://codspeed.io/) (free for OSS), which reports per-PR
  deltas. This protects the high-performance goal from silent erosion.
- **API stability** — [`cargo-semver-checks`](https://github.com/obi1kenobi/cargo-semver-checks)
  and [`cargo-public-api`](https://github.com/enselic/cargo-public-api) catch
  unintended public-API breaks; [`insta`](https://insta.rs/) snapshots pin
  serialized outputs so changes to them are deliberate and reviewed.
- **Proto governance** — [`buf lint`](https://buf.build/) and `buf breaking` guard
  the `.proto` contracts against style drift and breaking changes.

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
just ci          # the fast gate: fmt + clippy + test
```

`just ci` is the fast pre-PR gate. The heavier gates — coverage (`just
coverage`), mutation (`just test-mutation`), the structural sweep (`just
sweep`), and the supply-chain audits (`just deny` / `just audit` / `just
machete`) — are separate targets that run as their own parallel CI jobs. Run
them locally before a PR when the change touches what they guard.

If you need a finer-grained target than exists, add it to the `justfile` — `just`
is the frontend, and a missing target is a bug in the frontend.
