# Methodology: Overview

This kit is a way of working, not just a scaffold. The goal is simple to state and
hard to achieve: **let an AI agent build a real Rust server, fast, without letting
quality drift** — and do it without a human re-reading every line.

Three ideas make that possible. Each has its own page; this one ties them together.

1. **Push quality into deterministic gates.** A check a machine can run for free
   should never be a judgment call. See [quality-layers.md](quality-layers.md).
2. **Split the agent: cheap generator, strong reviewer.** Spend expensive
   judgment only where a machine can't decide. See
   [model-tiering.md](model-tiering.md).
3. **Self-learn, but ratchet.** The kit's understanding of the domain is free to
   evolve; its guardrails can only tighten. See
   [self-learning.md](self-learning.md).

## The change loop

Every unit of work is one change, and it runs the same loop:

```text
spec  →  worktree  →  implement  →  just ci  →  review  →  merge  →  reflect
```

1. **Spec.** The "what" is written down first — the constitution plus a
   per-feature spec. `just spec <name>` scaffolds it; the reviewer later checks the
   diff against it. See [spec-driven.md](spec-driven.md).
2. **Worktree.** `just new-feature <name>` creates a git worktree and branch. One
   change lives in one worktree, isolated from every other in-flight change.
3. **Implement.** The generator writes code and tests against the spec, obeying
   [`constitution.md`](../../constitution.md).
4. **`just ci`.** The fast local gate — the layering check, format, clippy
   (`-D warnings`), and the workspace test suite. Coverage, mutation, the
   structural sweep, and the supply-chain audits run as their own `just` targets
   and as separate CI jobs, not from `just ci`. Nothing proceeds red. See
   [testing.md](testing.md).
5. **Review.** The reviewer subagent is the gate; CodeRabbit backs it up on OSS
   PRs. They check the diff against the spec, hunt for gaming and gate-tampering,
   and enforce craft (DRY/KISS, comment economy).
6. **Merge.** One change, one PR, Conventional Commits, green.
7. **Reflect.** The loop feeds itself: what we learned updates the domain model,
   the lessons ledger, or the ADRs — and recurring findings graduate into new
   deterministic gates.

## The rules that hold it together

The full, authoritative list is [`constitution.md`](../../constitution.md). The
principles worth naming here, because everything else follows from them:

- **`just` is the frontend.** Humans and CI both go through `just`. A missing
  target is a bug in the frontend — build it, don't work around it.
- **Fix the system, not the instance.** Every bug fix closes its whole class with
  a new test, lint, hook, or rule. This is what makes the quality curve bend the
  right way over time instead of eroding.
- **Pre-existing issues → fold or branch, and justify it.** When the agent trips
  over an unrelated problem, it decides whether to fix it here (fold) or file and
  defer it (branch), and writes the reasoning in the PR. The reviewer checks the
  call. This keeps changes focused without letting rot accumulate silently.
- **Never self-lower a gate.** See [self-learning.md](self-learning.md).

## The dependency vetting rubric

"Library before writing" is a rule — prefer a good dependency over bespoke code —
but only after the candidate clears this rubric. The agent applies it before
adding any new crate, and records the outcome in the PR.

| Criterion              | Passes if…                                                                                                  |
| ---------------------- | ----------------------------------------------------------------------------------------------------------- |
| **License-compatible** | License is Apache-2.0-compatible; `cargo-deny` allows it.                                                   |
| **Reputable**          | Recognized authorship/org; not a typosquat; sane download and reverse-dep counts.                           |
| **Low rug-pull risk**  | Not a one-maintainer black box for a load-bearing role; no history of malicious or abandoned releases.      |
| **Maintained**         | Recent releases, issues triaged, security reports handled; compiles on our pinned toolchain.                |
| **Community**          | Real usage and docs; problems are searchable, not silent.                                                   |
| **Good fit**           | Solves *our* problem without dragging in a heavy or conflicting dependency tree; the API fits our layering. |

If a candidate clears every row, prefer it. If it fails any row and no alternative
clears the bar, **write our own** — small, owned, and tested — rather than take on
a liability. Either way, the decision and its reasoning go in the PR, and a
recurring gap becomes a lesson (and possibly a vetted default).

## Where to go next

- [spec-driven.md](spec-driven.md) — how the "what" enters and gets verified.
- [testing.md](testing.md) — the pyramid, from unit to DST to fuzz, and its gates.
- [quality-layers.md](quality-layers.md) — the L0–L4 defense-in-depth.
- [model-tiering.md](model-tiering.md) — the generator/reviewer cascade and its cost logic.
- [self-learning.md](self-learning.md) — how the kit adapts without weakening itself.
