# Lessons

The heuristics ledger. When the agent (or a reviewer) learns something that should
change future behavior but isn't yet worth a hard rule, it lands here — with
provenance, so we know why we believe it and how sure we are.

This file is **EVOLVABLE**. Entries flow in through the normal PR + reviewer loop.
The [self-learning methodology](methodology/self-learning.md) describes the loop;
this file is its memory. It is meant to stay **small**: lessons flow *out* as they
graduate into enforcement, so the ledger holds only what is still just judgment.

## How an entry works

Each lesson carries four things:

| Field          | Meaning                                                       |
| -------------- | ------------------------------------------------------------- |
| **Lesson**     | The heuristic, stated as an actionable rule of thumb.         |
| **Date**       | When it was first recorded (`YYYY-MM-DD`).                    |
| **Trigger**    | What made us learn it — the bug, review comment, or incident. |
| **Confidence** | `provisional` or `confirmed`.                                 |

## The N=3 promotion rule

A new lesson enters as **`provisional`**. Provisional lessons inform judgment but
are not treated as binding, and the reviewer applies them softly.

When the *same* lesson is independently re-triggered **three times** (N=3) —
three separate PRs or incidents where it would have prevented a problem — it is
promoted to **`confirmed`**. Update its row: bump the confidence, and append the
dates/triggers of the recurrences in the Trigger cell so the count is auditable.

**Graduation drains the lesson from this ledger.** A `confirmed` lesson does not
stay here as prose — its substance moves to the layer that can hold it best, and
its Active row is deleted:

- a **deterministic check** (a lint, an `ast-grep` rule, a test, a `just` gate)
  when the lesson is mechanically decidable — the flywheel in
  [quality-layers.md](methodology/quality-layers.md): judgment becomes free
  enforcement and the audit surface **shrinks**;
- a **constitution rule** ([constitution.md](../constitution.md)) when the lesson
  is a guardrail or craft principle no single check captures;
- a **methodology doc** when it's guidance about how we work.

All that remains here is a one-line pointer under "Graduated" so the flywheel
stays auditable; the full statement lives in its new home and this file's git
history. A clear-cut lesson may graduate straight to its home without waiting out
N=3 when a human calls it.

Retire a lesson (move it to "Retired") if it is contradicted by a later lesson or
made obsolete by a design change. Never silently delete — provenance is the point.

---

## Active lessons

| Lesson                                                                | Date | Trigger | Confidence |
| --------------------------------------------------------------------- | ---- | ------- | ---------- |
| *(none — every recorded lesson has graduated to its home; see below)* |      |         |            |

## Graduated

Drained into their enforcing home; kept only as one-line pointers so the flywheel
stays auditable. Full context lives in the linked home and this file's git history.

- **A one-tool recipe stays a plain `just` line; only real control flow moves to `cargo xtask`** (a nested `cargo xtask` → `cargo <plugin>` call can mangle the plugin's argv) → [constitution.md](../constitution.md) §2. (2026-07-05)
- **Verify a pin's current version at write time (`cargo add`), don't trust memory or lean on Dependabot as the currency mechanism** → [constitution.md](../constitution.md) §2 (Latest stable, verified). (2026-07-05)
- **The toolchain is declared once in `rust-toolchain.toml`; CI doesn't re-declare it with a `dtolnay/rust-toolchain` step** → [constitution.md](../constitution.md) §1. (2026-07-05)
- **commitlint `body-max-line-length` is unsatisfiable for Dependabot's generated body** → disabled in `commitlint.config.mjs`. (2026-07-05)
- **Gates run pre-merge and reproduce in CI** (post-merge-only automation needs a pre-merge counterpart; a gate mustn't depend on state a PR's detached-`HEAD` checkout lacks) → [constitution.md](../constitution.md) §2, enforced by `just release-plz-check`. (2026-07-05)
- **A branch must descend from current `origin/main` before a PR** → [constitution.md](../constitution.md) §2. (2026-07-05)
- **Prefer an in-tree crate over a new pin; bespoke code owns its format's edge cases** → [constitution.md](../constitution.md) §4. (2026-07-05)
- **Automation must not shell out to a platform-specific binary** (a `date` shell-out slipped in because no rule guarded the class) → deterministic `ast-grep-rules/no-platform-shellout.yml`, backing [constitution.md](../constitution.md) §2 (Portable automation). (2026-07-06, rot-sweep L2)
- **A figure a doc cites as evidence is machine-asserted against its source, never hand-copied** (a hand-copied number silently rots when its source changes and nobody notices) → a check regenerates the figure and fails until the doc and its source agree — the pattern behind the generated config reference's drift test, [methodology/twelve-factor.md](methodology/twelve-factor.md). (2026-07-06, rot-sweep L2)
- **A safety-critical library default is pinned in code, never inherited** (a default that a version bump or an env var can flip silently downgrades the guarantee you depend on) → set the safety option explicitly and lint any builder constructed without it, so the guarantee can't regress, backing [constitution.md](../constitution.md) §5. (2026-07-08, spec review)
- **A declared git hook the onboarding never installs is not a gate** (the `commit-msg` → commitlint hook lived in `lefthook.yml`, but `mise install && mise run install-cargo-tools` never ran `lefthook install`, so a fresh clone had no hook — commit-message and other violations surfaced only in CI, not at commit time) → the onboarding (`scripts/install-cargo-tools.mjs`, run by `mise run install-cargo-tools`) now runs `lefthook install`, so setup can't leave the declared hooks unwired. (2026-07-10, self-compliance audit)

## Retired

Lessons no longer in force, with the reason.

| Lesson       | Date retired | Reason |
| ------------ | ------------ | ------ |
| *(none yet)* |              |        |
