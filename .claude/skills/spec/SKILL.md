---
name: spec
description: >-
  Run the plan → implement → verify loop against a written spec. Use when the
  user says "write a spec", "spec this out", "plan this change", "let's design
  before coding", or at the start of any non-trivial feature. Produces a spec
  (context, goal, non-goals, acceptance criteria/tests, risks) that the reviewer
  checks the diff against.
---

# Spec — Plan → Implement → Verify

Non-trivial work is driven by a short written spec. The spec is the contract: it
turns intent into checkable acceptance criteria, keeps scope honest, and gives the
`reviewer` subagent something concrete to check the diff against. No spec → no
shared definition of "done."

## The loop

1. **Plan.** Write the spec (template below). Resolve open questions *before*
   coding. Convert each acceptance criterion into a concrete test at the right
   layer (see `layered-testing`). If a dependency is involved, run
   `dependency-vetting` now.
2. **Implement.** Write the failing tests first (test-first), then the smallest
   implementation that makes them pass. Keep the diff scoped to the spec. Touch
   something unrelated? → `fold-or-branch`.
3. **Verify.** Run the gates through `just` (`just lint`, the relevant
   `just test-*`, then `just test`/`just ci`). Re-read the spec: is every
   acceptance criterion demonstrably met, and no non-goal implemented? Then hand
   to `reviewer`.

Loop 1→3 until green. Update the spec if reality forces a change — a stale spec is
worse than none, and the reviewer will flag drift.

## Scaffold it

```sh
just spec <name>
```

Drops a spec skeleton in the repo's spec location (e.g. `specs/<name>.md`). Fill
it in before implementing.

## Spec template

```md
# Spec: <name>

## Context
<Why now? What's the current behavior/limitation? Link the trigger — issue, bug,
request. Keep it to what a reviewer needs to understand the change.>

## Goal
<The one outcome this change delivers, in a sentence or two.>

## Non-goals
<Explicitly out of scope. This is what stops scope creep — the reviewer treats
anything here that shows up in the diff as a finding.>

## Acceptance criteria  (each becomes a test)
- [ ] <observable, checkable behavior> — test: <layer + file::name>
- [ ] <…>
- [ ] Failure/edge cases: <what must NOT happen>

## Design sketch (optional)
<Key types, module boundaries, data flow. Only if it aids review.>

## Risks & mitigations
<Concurrency, data migration, perf, API/semver, security. For each: how the tests
or rollout mitigate it. Note anything needing chaos/DST, fuzz, or semver-checks.>

## Dependencies
<New crates? Link the dependency-vetting note. None? Say so.>
```

## What "good" looks like

- Acceptance criteria are **observable and testable**, not vague ("works better").
- Non-goals are stated, so the diff can be judged for scope.
- Every criterion maps to a real test at an appropriate layer.
- Risks name the *specific* failure mode and the test/layer that guards it.

The reviewer reads this spec first and checks the diff against it criterion by
criterion. Write it for that reader.
