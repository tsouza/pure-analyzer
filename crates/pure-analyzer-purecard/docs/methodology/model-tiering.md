# Methodology: Model Tiering

Judgment from a strong model is the most expensive thing this system spends. The
tiering strategy is about spending it well: **cheap to generate, strong to
review**, and escalate only when risk warrants it.

## The asymmetric cascade

Generation and review are *not* the same job and don't need the same model.

- **Generator — cheaper, faster model.** Writes code and tests against the spec.
  It's constrained on all sides — by the constitution, by L0/L1 deterministic
  gates, by the spec — so it doesn't need to be the strongest model available. It
  needs to be fast and cheap, because it does the high-volume work.
- **Reviewer — stronger model, as a subagent.** Gates the change. It reads the
  diff against its spec, hunts for gaming and gate-tampering, and enforces craft
  (DRY/KISS, comment economy). This is where judgment lives, so this is where the
  capability budget goes.

The asymmetry is the whole trick: you get strong-model rigor on the *decision that
matters* (does this merge?) without paying strong-model rates on every token of
*generation*.

## Escalation triggers

Review is not uniform. A trivial, low-risk diff gets a light pass; scrutiny
escalates when signals say it should:

- **Big or sensitive diff** — large surface area, or touching security-,
  concurrency-, or data-integrity-sensitive code.
- **Low generator confidence** — the generator flags uncertainty about its own
  output.
- **An L1 gate tripped** — a mechanical smell (complexity, duplication, a
  structural-rule hit) is a cue that the change deserves a closer, human-grade
  look, not just a fix-and-move-on.

Escalation can mean more reviewer effort, a stronger model, or a second pass.
Low-risk changes don't pay for scrutiny they don't need; risky ones can't dodge it.

## Why this saves money

Two forces compound in the right direction:

1. **Deterministic-first.** Everything L0/L1 can decide never reaches a model at
   all (see [quality-layers.md](quality-layers.md)). Models only see what genuinely
   needs judgment.
2. **The flywheel.** Recurring judgment findings graduate into free L1 rules
   (N=3 promotion), so the pool of "needs a model" shrinks over time. The audit
   cost per change trends **down** as the project matures.

So the strong model is reserved for novel risk, and the volume of novel risk falls
as the system learns. That's a cost curve that improves with age instead of
degrading — the opposite of "the agent slowly makes everything worse."

## The independent backstop

The homegrown reviewer subagent is the gate, but it isn't the only reader:
**CodeRabbit** (free for OSS) reviews in parallel as an independent second opinion.
It costs nothing on OSS and covers blind spots the primary reviewer might share
with the generator. Redundancy where it's free; asymmetry where it's expensive.
