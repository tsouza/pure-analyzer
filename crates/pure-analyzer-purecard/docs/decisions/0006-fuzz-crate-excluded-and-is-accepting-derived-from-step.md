# 0006. Excluded fuzz crate; EOS acceptance derived from `step`

- **Status:** Accepted
- **Date:** 2026-07-11
- **Deciders:** PureCARD decoder engineer (M5 hardening)

## Context

M5 hardens the shipped decoder without adding capability. Two structural choices
arose:

1. **Fuzzing must not compromise the core's invariants.** `libfuzzer-sys`'s
   `fuzz_target!` macro generates `unsafe` and requires a nightly toolchain. The
   published `purecard` crate is `#![forbid(unsafe_code)]` (constitution §1) and
   stable-pinned via `rust-toolchain.toml` (§1). A fuzz harness compiled *inside*
   the crate or workspace would drag `unsafe` and a nightly requirement into the
   shipped surface.

2. **EOS acceptance was a hand-maintained singleton.** `Pda::is_accepting`
   accepted exactly `State::AfterValue`, so a stream ending on a completed but
   un-boundary-terminated lexical token (a trailing top-level identifier,
   `|X.all()->name`) reported *incomplete* — a latent precision wart (old issue
   #8). Widening it by hand-listing the value-terminal states
   (`InIdent`/`InNumberInt`/…) would duplicate knowledge already encoded in
   `step`, and drift the moment a new terminal state is added.

## Decision

We will keep the cargo-fuzz crate in a **separate `fuzz/` package excluded from
the root workspace** (alongside `lints`), built out-of-band with `cargo +nightly
fuzz`, so the core stays `forbid(unsafe)` and stable-pinned.

We will **derive EOS acceptance from `step` itself** —
`stack.is_empty() && matches!(step(state, None, VALUE_BOUNDARY), Step::Next(AfterValue))`
— rather than maintain a list of accepting states, keeping a single source of
truth for terminality (constitution §4).

## Alternatives considered

- **Fuzz crate as a workspace member / `#[cfg(fuzzing)]` module in the core.**
  Rejected: it would either break `forbid(unsafe_code)` or force the whole
  workspace onto nightly. The `lints` crate already established the
  excluded-crate pattern for exactly this class of out-of-band tool.
- **A `finalize()` state machine / new EOS-signal `State`.** Rejected as
  over-engineering for a non-corpus-reachable wart: it forks the recognizer
  contract and adds a subsystem where a one-line gate widening suffices.
- **Hand-listing the accepting states in `is_accepting`.** Rejected: duplicates
  `step`'s knowledge and rots — a DRY defect (constitution §4).

## Consequences

- **Easier:** the core keeps its two load-bearing invariants (`forbid(unsafe)`,
  stable pin) while gaining a real no-panic backstop; any future value-terminal
  state added to `step` is covered by `is_accepting` for free.
- **Harder / obligations:** the fuzz crate carries its own lockfile and nightly
  requirement, and needs its own CI job (`fuzz.yml`) — it is *not* covered by
  `just ci`. New fuzz targets must be registered in both `fuzz/Cargo.toml` and
  the `FUZZ_TARGETS` list in `xtask` (and the `fuzz.yml` matrix).
- **Soundness argument (why gold stays 5034/5034):** the `is_accepting` change
  reads `step` but never mutates it, so it **strictly adds** accepting
  configurations — it never turns a live byte dead or clears a mask bit. Every
  gold query ends in `)` → `AfterValue`, still accepting; the only newly-reachable
  completion is a trailing top-level identifier, an accepted L1 over-acceptance
  residual (§5.6), caught downstream — never a soundness regression.
- **Revisit if:** cargo-fuzz gains stable-toolchain support (the exclusion could
  relax), or a `finalize()`/EOS-signal contract becomes genuinely needed (e.g. an
  L3 tier).
