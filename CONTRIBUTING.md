# Contributing

Contributions are welcome — human or agent. Everyone runs through the same gates,
so the bar is the same for all.

By contributing you agree that your contributions are licensed under
[Apache-2.0](LICENSE), and you certify the
[Developer Certificate of Origin](https://developercertificate.org/) for each
commit.

## Ground rules

The authoritative rules live in [`constitution.md`](constitution.md). Read it
before your first change. The short version:

- **One change → one branch → one PR.** Use a git worktree per branch.
- **Conventional Commits** for every commit message
  (`feat:`, `fix:`, `docs:`, `refactor:`, `test:`, `chore:` …).
- **`just` is the frontend.** Don't hand-roll `cargo` invocations in CI or docs —
  add a `just` target instead.
- **Nothing merges red.** `just ci` must be green.
- **No test skipping, no weakened assertions.** Flakes are bugs; fix them.
- **Fix the system, not the instance.** A bug fix must also add the test, lint,
  or rule that prevents the whole class from recurring.

## Workflow

```sh
mise install && mise run install-cargo-tools   # provision toolchain + dev tools
just hooks-install                             # wire the git hooks
just new-feature <name>    # worktree + branch
just spec <name>           # scaffold a spec, then plan → implement → verify
# ... make your change ...
just ci                    # must pass before you open a PR
```

Then open a PR. In the description:

- link the spec the change implements,
- note anything you updated in `docs/domain-model.md`, `docs/lessons.md`, or
  `docs/decisions/`,
- if you touched a **pre-existing** unrelated issue, state your
  **fold-vs-branch** decision and why.

## Review

Every PR is reviewed by the project's reviewer agent (the gate) and, for OSS PRs,
by CodeRabbit as an independent second opinion. Reviewers check the diff against
its spec, look for gaming or gate-tampering, and enforce DRY/KISS and comment
economy. See [`docs/methodology/model-tiering.md`](docs/methodology/model-tiering.md).

## Dependencies

New dependencies must clear the vetting rubric in
[`docs/methodology/overview.md`](docs/methodology/overview.md) (license-compatible,
reputable, maintained, low rug-pull risk, good fit). "Just add a crate" is not
automatic — prefer a vetted library, but write our own when nothing clears the bar.

## Changing a guardrail

Domain rules, docs, lessons, and ADRs evolve through the normal PR flow.
**PROTECTED** thresholds (coverage floor, mutation floor, forbid-skip,
`cargo-deny`) can be **tightened** by anyone but **loosened only by a maintainer**,
via the documented ratchet in
[`docs/methodology/self-learning.md`](docs/methodology/self-learning.md). Do not
attempt to lower a gate to make CI pass — CI recomputes gate values independently.
