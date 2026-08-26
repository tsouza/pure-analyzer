# Methodology: Overview

This repository carries a way of working, not just Rust code. The goal is simple
to state and hard to achieve: **let agents build and maintain real products
quickly without letting quality drift** — and do it without requiring a human to
re-read every line. It now governs two independent products, `pure-analyzer` and
PureCARD, plus the infrastructure that checks both.

Three ideas make that possible. Each has its own page; this one ties them together.

1. **Push quality into deterministic gates.** A check a machine can run for free
   should never be a judgment call. See [quality-layers.md](quality-layers.md).
2. **Split the agent: cheap generator, strong reviewer.** Spend expensive
   judgment only where a machine can't decide. See
   [model-tiering.md](model-tiering.md).
3. **Self-learn, but ratchet.** The repository's understanding of each product
   is free to evolve; its guardrails can only tighten. See
   [self-learning.md](self-learning.md).

## The change loop

Every unit of work is one change, and it runs the same loop:

```text
issue  →  worktree  →  implement  →  just ci  →  review  →  merge  →  reflect
```

1. **Issue.** The change's goal, non-goals, acceptance criteria, and dependencies
   live in its GitHub issue. The PR links it and records only implementation
   evidence and decisions, so the repository stores present product truth rather
   than a second work ledger.
2. **Worktree.** `just new-feature <name>` creates a git worktree and branch. One
   change lives in one worktree, isolated from every other in-flight change.
3. **Implement.** The generator writes code and tests against the issue, obeying
   [`constitution.md`](../../constitution.md).
4. **`just ci`.** The fast local gate — the layering check, format, clippy
   (`-D warnings`), and the workspace test suite. Coverage, mutation, the
   structural sweep, and the supply-chain audits run as their own `just` targets
   and as separate CI jobs, not from `just ci`. Nothing proceeds red. See
   [testing.md](testing.md).
5. **Review.** A reviewer subagent checks the diff against the issue and uses the
   PR's evidence to validate it, hunts for gaming and gate-tampering, and enforces
   craft (DRY/KISS, comment economy).
   Risky changes receive a separate reviewer pass; deterministic aggregate
   checks remain independent required gates.
6. **Merge.** One change, one PR, Conventional Commits, green.
7. **Reflect.** The loop feeds itself: durable product truth updates the domain
   model or an ADR, while a mechanically decidable failure becomes a new
   deterministic gate. Mutable work state stays in GitHub.

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
- **Keep product boundaries explicit.** Analyzer and PureCARD have no Cargo
  dependency edges in either direction. Shared parser or corpus work requires a
  dedicated issue and ADR; root automation may orchestrate both products without
  becoming part of either.

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
a liability. Either way, the decision and its reasoning go in the PR; a recurring
mechanically decidable gap becomes a deterministic guard.

An adopted crate must also keep `just deny`, `just audit`, and `just vet` green.
The cargo-vet gate distinguishes reviewed or imported audit coverage from an
explicit exact-version exemption; an exemption is visible audit debt, not a
certification.

## Where to go next

- [testing.md](testing.md) — the pyramid, from unit to DST to fuzz, and its gates.
- [quality-layers.md](quality-layers.md) — the L0–L4 defense-in-depth.
- [model-tiering.md](model-tiering.md) — the generator/reviewer cascade and its cost logic.
- [self-learning.md](self-learning.md) — how the repository adapts without weakening itself.
