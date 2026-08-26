---
description: Run the periodic rot sweep — deterministic L1 tools, then LLM judgment on the residue.
argument-hint: "[optional: path or crate to scope the sweep]"
allowed-tools: Bash(just sweep:*), Read, Grep, Glob, Edit, Write
---

Use the **`rot-sweep`** skill to run a deep audit${ARGUMENTS:+ scoped to `$ARGUMENTS`}.

1. **L1 (deterministic, run first):** `just sweep` — cargo-machete, ast-grep,
   duplication, complexity, postponed-marker. Fix or file everything it
   surfaces before spending any LLM budget.
2. **L2 (LLM judgment on the residue only):** semantic DRY, nonsense/dead intent,
   design smells (leaky layers, god-objects). Do not re-derive anything L1 already
   decides.
3. **Record** each independent finding in a GitHub Issue with affected paths
   and acceptance criteria; fix in-scope findings in the current PR.
4. **Promote** any mechanically decidable finding class into a new L1 rule
   (ast-grep pattern or clippy `disallowed-methods` entry) with a test, and note
   the implementation evidence in the PR.

Report: L1 findings (fixed/filed), L2 residue with confidence, linked Issues,
and any rule promoted this sweep.
