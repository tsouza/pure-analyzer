---
description: Run the independent reviewer gate on the current changeset.
argument-hint: "[optional: issue or PR ref]"
allowed-tools: Bash(just review:*), Bash(git diff:*), Bash(git fetch:*), Bash(git log:*), Read, Grep, Glob, Task
---

Invoke the **`reviewer`** subagent as an independent review gate on the current
diff.

Context to hand it:

- Diff: `git diff --merge-base origin/main` (fall back to `git diff main...HEAD`).
- Scope: `$ARGUMENTS` if given, else use the linked GitHub issue. Use the PR body
  only as implementation evidence; it must not restate the issue.

Ask the reviewer to check, per its charter:

- diff vs. linked issue (acceptance criteria met, no non-goals implemented), with
  the PR's implementation evidence,
- test-gaming (skips, `#[ignore]`, weakened assertions, over-mocking, hardcoded
  seeds),
- config/threshold/gate tampering (only tightening allowed; flag any loosening
  for human sign-off),
- DRY/KISS/magic constants/comment litter,
- justified fold-vs-branch for any discovered issues.

Prefer `just review` if it wraps the flow. Relay the reviewer's structured
verdict **verbatim and in full** — VERDICT, blockers, nits, issue conformance,
any gate/threshold changes, the fold-vs-branch judgment, and any escalation.
Do not truncate or reformat it. Do not fix code from this command — it is a gate.
