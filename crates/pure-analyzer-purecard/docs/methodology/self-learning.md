# Methodology: Self-Learning

The kit is meant to get *better* as the agent works — to accumulate understanding
of the domain and to keep tightening its own quality bar. The danger is obvious: a
system that can rewrite its own rules can also rewrite them to make its life
easier. This page is how we get the learning without the erosion.

The whole scheme rests on one distinction.

## Two tiers

**EVOLVABLE** — the "what" and the heuristics. These flow through the normal
PR + reviewer loop with no special ceremony:

- the domain rules in [`constitution.md`](../../constitution.md),
- [`domain-model.md`](../domain-model.md),
- feature specs in `specs/`,
- [`lessons.md`](../lessons.md),
- ADRs in [`decisions/`](../decisions/),
- heuristic rules in `CLAUDE.md`.

The agent may add, refine, and retire these freely. They *should* change as
understanding grows.

**PROTECTED** — the guardrails. These the agent may only ever make **stricter**:

- test thresholds (mutation score, coverage floor),
- the forbid-skip / postponed-marker gates,
- `cargo-deny` policy,
- the anti-gaming suites and reviewer configuration.

**The ratchet:** an agent may *tighten* a PROTECTED value in a PR (raise a floor,
add a ban) and it merges normally. *Loosening* one requires a human — enforced by
a **machine-checkable ratchet**: CI recomputes each PROTECTED value independently
from config the agent cannot edit, and fails any PR where a gate moved the wrong
way without maintainer sign-off (see [`CODEOWNERS`](../../CODEOWNERS)). The agent
literally cannot lower its own bar to make a change pass.

## The learning loop

Learning happens at three cadences:

1. **Per-PR reflection.** After each change, the agent asks what it learned. A new
   heuristic becomes a `provisional` entry in `lessons.md`; a new domain fact
   updates `domain-model.md`; a significant decision becomes an ADR.
2. **Post-incident systematization.** When something breaks, the fix isn't done
   until the *class* is closed — "fix the system, not the instance." The bug
   yields a test, lint, hook, or rule, and a lesson recording the trigger.
3. **Periodic consolidation.** On a schedule (aligned with the L3 weekly sweep),
   the agent dedupes lessons, resolves contradictions, and checks that the ledger
   still tells one coherent story. Contradictions are surfaced, not silently
   overwritten.

## Provenance and promotion

Every learned rule carries **provenance**: the date it was recorded, the trigger
that produced it, and a confidence tier. New rules are **`provisional`** and
applied softly. When the same lesson is independently re-triggered **N=3** times,
it is promoted to **`confirmed`** and becomes a strong default.

A confirmed lesson then **graduates out of the ledger** into the layer that can
hold it best — an **L1 deterministic rule** when it's mechanically decidable (the
flywheel from [quality-layers.md](quality-layers.md)), a guardrail or craft rule
in the **constitution**, or a **methodology doc**. Graduation *drains* the lesson:
its substance moves to that home and its ledger entry collapses to a one-line
pointer, so the audit surface **shrinks** rather than grows. The ledger stays
small by design — it holds only what is still just judgment. The bookkeeping lives
in [`lessons.md`](../lessons.md).

## Progressive disclosure and the CLAUDE.md budget

The agent's per-session context is finite and precious, so `CLAUDE.md` has a
**size budget**. It holds the intro, the hard rules in brief, and links — nothing
more. Detail lives where it's read on demand: the constitution, the methodology
docs, skills, and the ledger. When a rule outgrows a one-liner, the one-liner
stays in `CLAUDE.md` and the depth moves into a doc or skill it links to. Growth
goes *outward* into progressive disclosure, never by fattening the file every
session reads.

## The eval harness is the fitness function

Self-adjustment can regress: a "tightening" might be wrong, a consolidation might
drop something load-bearing, a new lesson might contradict an old truth. The
eval-driven harness is the backstop — a fitness function over agent behavior that
catches self-adjustment regressions before they compound. It is the heaviest piece
of the system and lands last, but it's what makes trusting the loop safe: the kit
can change itself, and the harness verifies it changed for the better.

## In one sentence

Let the agent learn the domain freely; let it only ever tighten its guardrails;
make every learned rule carry its provenance; and verify the whole self-adjusting
loop against an independent fitness function.
