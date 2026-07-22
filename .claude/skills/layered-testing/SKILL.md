---
name: layered-testing
description: >-
  Choose the right test layer and build coverage up the pyramid — unit →
  integration → chaos/DST → mutation → fuzz → e2e, plus perf and API-stability.
  Use when the user says "how should I test this", "write tests", "add
  coverage", "test strategy", or when a spec's acceptance criteria need to become
  real tests. Includes anti-gaming rules (randomized seeds, held-out suite, never
  skip/weaken).
---

# Layered Testing

Testing is a pyramid, not a checkbox. Push each behavior to the **cheapest layer
that can actually catch its failure mode**, then add the higher layers for the
properties unit tests can't see (concurrency, resource faults, unknown inputs,
regressions in the public API). Every layer runs through `just`.

## The layers (bottom → top)

1. **Unit** — pure logic, one crate, in-module `#[cfg(test)]` or `tests/` in the
   crate. Fast, deterministic, the bulk of your tests.

   ```sh
   just test-unit
   ```

2. **Integration** — cross-crate and real-dependency wiring. Stand up real
   backing services with **testcontainers** (Postgres, Redis, etc.) rather than
   mocking them; mocks at this layer hide integration bugs.

   ```sh
   just test-integration
   ```

3. **Chaos / Deterministic Simulation Testing (DST)** — inject faults and
   reorderings under a deterministic scheduler using **turmoil** (network) and/or
   **madsim** (runtime). The point is to explore many interleavings.

   ```sh
   just test-chaos
   ```

   **On failure, capture and print the seed** and pin it as a regression test.
   The seed must be **randomized per run** (e.g. from time/entropy), never
   hardcoded — a hardcoded seed turns exploration into a single fixed path.
4. **Mutation** — measure whether the tests actually *detect* changes, using
   **cargo-mutants**. Surviving mutants = assertions that don't assert.

   ```sh
   just test-mutation
   ```

5. **Fuzz** — feed structured/unstructured random input via **bolero** (which can
   drive libFuzzer/AFL/property engines) to find inputs no human enumerated.

   ```sh
   just fuzz
   ```

6. **E2E** — the whole binary/server exercised from the outside on realistic
   flows. Fewest, slowest, highest-fidelity; reserve for critical paths.

## Cross-cutting layers

- **Performance** — **criterion** benchmarks for hot paths; fail the gate on
  regressions beyond a documented threshold, not on noise.

  ```sh
  just bench
  ```

- **API stability** — for public crates, guard the surface with
  **cargo-semver-checks** / **cargo-public-api**, and snapshot user-visible output
  with **insta**. A breaking change must be intentional and versioned.
- **Coverage** — a floor, not a target. Read what's *un*covered.

  ```sh
  just coverage
  ```

## Anti-gaming rules (the reviewer enforces these)

- **Randomized seeds, not hardcoded.** DST/fuzz/property runs explore fresh seeds
  each run; failing seeds are pinned *in addition to*, never *instead of*, random
  exploration.
- **Never skip, `#[ignore]`, or weaken.** No `todo!()`/`unimplemented!()` in test
  paths, no `assert!(true)`, no exact→range assertion softening, no deleting an
  assertion while keeping the test body. Zero-tolerance for flakes: a flaky test
  is a bug to fix, not a `retry` to add.
- **Hold-out suite.** Keep a set of tests the generator does not see while
  implementing, so coverage isn't overfit to known cases.
- **Test behavior, not implementation.** Assert observable outcomes; avoid
  over-mocking the unit under test into a tautology.
- **Fix the system, not the instance.** When a bug escaped, add the test *layer*
  or property that would have caught the whole class, not just this input.

## Choosing a layer (quick guide)

- Pure function / branch logic → **unit**.
- "Does it talk to Postgres correctly?" → **integration** (testcontainers).
- "Does it survive a dropped connection / reordered messages?" → **chaos/DST**.
- "Do my tests actually catch bugs?" → **mutation**.
- "What input breaks the parser?" → **fuzz**.
- "Is the user-visible flow intact?" → **e2e**.
- "Is it fast enough / did I break the API?" → **bench** / **semver-checks**.

## Full gate

```sh
just test        # runs the layers wired for CI
just ci          # everything, as CI runs it
```
