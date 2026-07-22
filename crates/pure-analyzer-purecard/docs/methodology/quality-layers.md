# Methodology: Quality Layers

The expensive way to keep AI-authored code clean is to have a smart reviewer read
everything, every time. That doesn't scale and it burns tokens. So we arrange
quality as **defense in depth, cheapest first**: each layer catches what it can
deterministically, and only what genuinely needs judgment reaches a model.

The layers, L0 through L4:

## L0 — Standard linters (free, instant)

The baseline every Rust project should have, wired as hard gates:

- **clippy** with warnings-as-errors.
- **rustfmt** — formatting is not a debate.
- **cargo-deny** — license, ban, and advisory policy.
- **cargo-audit** — known-vulnerability check on dependencies.

If L0 is red, nothing else runs. It's the cheapest possible feedback.

## L1 — Deterministic project gates (cheap, mechanical)

Project-specific checks that encode *our* rules as machine-decidable pass/fail. No
model is involved, so they cost nothing per run and never disagree with themselves:

- **Unused dependencies** — [`cargo-machete`](https://github.com/bnjbvr/cargo-machete)
  / `cargo-shear`.
- **Dead code** — the compiler's `dead_code` as an error.
- **Postponed-marker gate** — `TODO`, `FIXME`, `todo!()`, `unimplemented!()`,
  `#[ignore]`, `dbg!` fail the build. Unfinished work doesn't merge disguised as
  finished.
- **Structural rules** — [`ast-grep`](https://ast-grep.github.io/) (or Semgrep)
  patterns encode conventions, banned constructs, and architecture guardrails as
  syntax-tree matches.
- **Semantic lints** — custom [`dylint`](https://github.com/trailofbits/dylint)
  lints in the `lints` crate: compiler-grade checks like "no `unwrap` in library
  crates" that need type information a text matcher can't see.
- **Complexity budgets** — a copy-paste/duplication detector plus clippy's
  `cognitive_complexity`, so DRY and KISS are measured, not just preached.
- **Coverage floor** — `cargo-llvm-cov` below the PROTECTED minimum fails.

L1 is where lessons go to die a good death: a recurring judgment finding that can
be made mechanical becomes an L1 rule (see the flywheel below).

## L2 — Generator → reviewer cascade (judgment, tiered)

What deterministic checks can't decide — is this the *right* design? does the diff
match the spec? is this comment litter or genuine explanation? — goes to models,
but **asymmetrically**: a **cheap generator** writes, a **stronger reviewer
subagent** gates. Review is escalated, not applied uniformly, by risk and
uncertainty:

- a large or sensitive diff,
- low generator confidence,
- or an L1 gate that tripped (a mechanical smell warrants a closer human-grade look).

This keeps strong-model spend proportional to risk. The economics are in
[model-tiering.md](model-tiering.md).

## L3 — Scheduled rot sweeps (periodic judgment)

Rot accumulates between changes. Two sweeps catch it:

- **Per-PR light sweep** — a quick pass for smells introduced by the change.
- **Weekly deep sweep** — a thorough review of the codebase as a whole:
  duplication creeping across modules, abstractions going stale, drift from the
  domain model.

Findings feed [`lessons.md`](../lessons.md). And crucially: a finding that recurs
**N=3** times is **promoted into a new L1 deterministic rule**. The judgment that
caught it three times becomes a free check that catches it forever after.

## L4 — Reviewer as gate, with an independent backstop

- The **homegrown reviewer subagent is the gate** — the change does not merge
  without its approval. It's tuned to this project's rules and reads the diff
  against its spec.
- **CodeRabbit** (free for OSS) runs alongside as an *independent* second opinion.
  A backstop, not the primary gate — so a blind spot in one reviewer isn't a blind
  spot in the system.

## The flywheel

The whole system tightens itself. Every layer feeds the one below it:

```text
L4/L3 judgment finds an issue
   → recurs N=3 times → promoted to an L1 deterministic rule
      → now caught for free, forever → audit surface shrinks
         → strong-model attention freed for genuinely novel risk
```

Over time the fraction of quality enforced by *free, deterministic* checks rises,
and the per-change cost of keeping the bar high trends **down**. That is the
point: judgment is expensive and should be spent buying permanent, cheap
enforcement — not re-spent on the same finding every week. The promotion mechanism
is governed by [self-learning.md](self-learning.md); the thresholds it can and
can't touch are PROTECTED.
