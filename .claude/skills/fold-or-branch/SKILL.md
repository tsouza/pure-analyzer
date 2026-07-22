---
name: fold-or-branch
description: >-
  Decide what to do with a pre-existing issue you discovered mid-change: fold the
  fix into the current changeset, or spin a dedicated worktree/branch for it. Use
  whenever you notice a bug, smell, or gap that is NOT part of the work you're
  doing — "I found something unrelated", "there's a pre-existing bug here",
  "should I fix this too?". Requires writing the decision into the PR body.
---

# Fold or Branch

While doing scoped work you will trip over unrelated problems — a latent bug, a
dead function, a missing test, a smell. You must consciously decide whether to
**fold** the fix into the current changeset or **branch** it into its own
worktree. The one thing you may not do is fix it silently: an unexplained,
out-of-scope change pollutes the diff and defeats review.

## Decision procedure

Fold only when **all** of these hold; otherwise branch:

| Criterion                                                                   | Fold       | Branch             |
| --------------------------------------------------------------------------- | ---------- | ------------------ |
| **On-path?** Is the issue in code this change already touches?              | yes        | no                 |
| **Size** of the fix relative to the current diff                            | small      | large / comparable |
| **Risk / blast radius** (concurrency, data, public API, security)           | low        | non-trivial        |
| **Coupling** — does the current change depend on this being fixed?          | yes → fold | no                 |
| **Reviewability** — does folding keep the diff coherent and easy to review? | yes        | no                 |
| **Scope** — is it the same logical concern as the spec?                     | yes        | no                 |

Rules of thumb:

- **Blocking + on-path + small + low-risk → fold.** (e.g. the function you're
  editing has an off-by-one two lines down.)
- **Non-blocking, or off-path, or large, or risky → branch.** Start it with the
  `start-feature` flow (`just new-feature <name>`) so it gets its own spec, tests,
  and PR. Leave a breadcrumb (issue/TODO-in-tracker, *not* a `TODO` in code — the
  `postponed-marker` hook rejects those).
- **Unsure → branch.** A separate small PR is cheaper than an incoherent large one.

## Mandatory justification (the reviewer verifies this)

Whichever you choose, write it into the **PR body** under a `## Discovered issues`
heading:

```md
## Discovered issues
- <one-line description of the issue> @ <file:line>
  Decision: FOLD | BRANCH(<branch/issue ref>)
  Why: <on-path? size? risk? coupling?> — <one or two sentences>
```

- If **FOLD**: the diff must contain the fix *and* a test that covers it (fixing
  without a test just re-arms the bug — "fix the system, not the instance").
- If **BRANCH**: link the new branch/worktree or tracker issue so it isn't lost.

The `reviewer` subagent checks that every out-of-scope change in the diff has a
matching, justified entry here. An unjustified out-of-scope change is a `fail`.

## Anti-patterns

- Silently fixing unrelated code "while I'm here" → unreviewable diff.
- Leaving a `// TODO: fix later` in the source instead of branching/tracking →
  blocked by the `postponed-marker` hook.
- Folding a large refactor into a feature PR → split it; PR-per-change.
