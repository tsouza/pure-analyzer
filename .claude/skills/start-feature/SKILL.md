---
name: start-feature
description: >-
  Bootstrap a new unit of work the right way: isolated git worktree + feature
  branch, a spec, a `just` target for the work, and a failing test first. Use
  when the user says "start a feature", "let's build X", "new feature", "begin
  work on", or when kicking off any non-trivial change. Enforces
  worktree-per-branch and test-first.
---

# Start Feature

Every change starts isolated and test-first. No work happens on `main`, and no
implementation is written before a failing test pins the intended behavior. Drive
everything through `just` so the workflow matches CI.

## Preconditions

- Working tree is clean (`git status`), or the user has explicitly agreed to
  stash. Never start a feature on top of unrelated uncommitted work.
- You have a short kebab-case name for the feature, e.g. `rate-limit-login`.

## Procedure

1. **Create the worktree + branch.**

   ```sh
   just new-feature <name>
   ```

   This creates a dedicated git worktree and a `feat/<name>` branch (worktree-per-
   branch: parallel work never shares a checkout). `cd` into the new worktree that
   the target reports. If the target is missing, create the worktree manually and
   fix the `justfile` in a follow-up — but prefer `just`.

2. **Scaffold the spec.**

   ```sh
   just spec <name>
   ```

   This drops a spec skeleton (see the `spec` skill for the template). Fill in
   context, goal, non-goals, acceptance criteria, and risks *before* coding. The
   acceptance criteria become the tests you write in step 4, and the reviewer
   checks the diff against this spec.

3. **Ensure a `just` target exists for the work.**
   - The unit of work should be runnable/verifiable via a `just` target
     (e.g. a `run`, a specific `test-*`, a bench). If a suitable target exists,
     use it. If not, **add a minimal one** to the `justfile` so the work is
     reproducible and CI-observable. Never leave the work invokable only by a raw
     ad-hoc command — put it behind `just`.

4. **Write the failing test first (test-first, non-negotiable).**
   - Translate each acceptance criterion into a test at the right layer (see
     `layered-testing`): unit in the crate, integration under `tests/`, etc.
   - Run it and confirm it **fails for the right reason**:

     ```sh
     just test-unit         # or the targeted layer
     ```

   - A test that passes before you've implemented anything is not testing what
     you think — fix the test, not the timing.

5. **Only now implement**, looping plan → implement → verify (see `spec`) until
   the failing tests pass and `just lint` / `just test` are green.

## Guardrails

- New dependency needed? Run the `dependency-vetting` skill first.
- Discover an unrelated pre-existing issue while working? Do **not** silently fix
  it — run `fold-or-branch` to decide, and record the justification in the PR.
- One change per PR (PR-per-change). If the feature grows two heads, split it.
- Commits are conventional (`feat:`, `fix:`, `test:`, `refactor:`, …); the
  `commit-msg` hook enforces the format.

## Definition of done for this skill

Worktree + `feat/<name>` branch exist, a filled-in spec is committed, a `just`
target covers the work, and there is at least one test that **fails** pending the
implementation. Hand off to normal implement/verify from here.
