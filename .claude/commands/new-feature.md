---
description: Start a feature the right way — worktree + branch, spec, just target, failing test first.
argument-hint: <feature-name>
allowed-tools: Bash(just new-feature:*), Bash(just spec:*), Bash(just test-unit:*), Bash(git status:*), Read, Write, Edit, Glob, Grep
---

Use the **`start-feature`** skill to bootstrap work on `$ARGUMENTS`.

1. Confirm the working tree is clean (`git status`).
2. `just new-feature $ARGUMENTS` — create the git worktree + `feat/$ARGUMENTS`
   branch (worktree-per-branch). `cd` into the reported worktree.
3. `just spec $ARGUMENTS` — scaffold the spec; fill in context, goal, non-goals,
   acceptance criteria, risks.
4. Ensure a `just` target covers the work; add a minimal one to the `justfile` if
   none fits.
5. Write the **failing test first** (test-first) at the right layer, and confirm
   it fails for the right reason (`just test-unit` or the targeted layer).

Only after there is a failing test should implementation begin. Report: worktree
path, branch, spec path, the `just` target used/created, and the failing test.

If `$ARGUMENTS` is empty, ask for a kebab-case feature name first.
