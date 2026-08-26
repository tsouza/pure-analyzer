# Contributing

Proposals are welcome — human or agent. Maintainer-authored repository changes
use the singular project identity described below, and everyone runs through the
same quality gates.

This is an umbrella repository with two independent products: the
`pure-analyzer` scaffold and the PureCARD constrained decoder. PureCARD's M0–M5
code artifacts exist, but its end-to-end proof obligations remain open. State
which product or shared-infrastructure surface your change owns; repository
co-location is not permission to couple their runtime code or test assets.

By contributing you agree that your contributions are licensed under
[Apache-2.0](LICENSE), and you certify the
[Developer Certificate of Origin](https://developercertificate.org/) for each
commit.

## Ground rules

The authoritative rules live in [`constitution.md`](constitution.md). Read it
before your first change. The short version:

- **One change → one branch → one PR.** Use a git worktree per branch.
- **One project identity.** Human commits and GitHub writes use `tsouza`, never
  `tsouza-squid`; `just project-identity` checks the exact Git identity and
  account-specific origin. GitHub CLI operations use `just github <arguments>`.
- **Conventional Commits** for every commit message
  (`feat:`, `fix:`, `docs:`, `refactor:`, `test:`, `chore:` …).
- **`just` is the frontend.** Don't hand-roll `cargo` invocations in CI or docs —
  add a `just` target instead.
- **Nothing merges red.** `just ci` must be green.
- **No test skipping, no weakened assertions.** Flakes are bugs; fix them.
- **Keep product edges at zero.** Analyzer crates and PureCARD may not depend on
  one another in any Cargo dependency kind. Parser or corpus sharing requires a
  dedicated spec and ADR; `xtask` remains shared infrastructure.
- **Fix the system, not the instance.** A bug fix must also add the test, lint,
  or rule that prevents the whole class from recurring.

## Workflow

```sh
mise install && mise run install-cargo-tools   # provision toolchain + dev tools
just hooks-install                             # wire the git hooks
just project-identity                          # verify Git author + origin
just new-feature <name>    # worktree + branch
just spec <name>           # scaffold a spec, then plan → implement → verify
# ... make your change ...
just ci                    # must pass before you open a PR
```

Then open a PR. In the description:

- link the spec the change implements,
- identify whether the diff belongs to analyzer, PureCARD, or shared
  infrastructure,
- note anything you updated in `docs/domain-model.md`, `docs/lessons.md`, or
  `docs/decisions/`,
- if you touched a **pre-existing** unrelated issue, state your
  **fold-vs-branch** decision and why.

## Review

Every PR is reviewed against its spec, with a separate reviewer pass for risky
changes. Reviewers look for gaming or gate-tampering and enforce DRY/KISS and
comment economy. The default-branch ruleset independently requires an up-to-date
branch, resolved review conversations, and the `ci-gate`, `lint-gate`,
`security-gate`, `purecard-fuzz-gate`, and `purecard-wheels-gate` checks. See
[`docs/methodology/model-tiering.md`](docs/methodology/model-tiering.md).

GitHub approval is not required while the project has only one maintainer:
GitHub does not allow a PR author to approve their own change. This avoids a
deadlock; it does not waive review. Add a second trusted maintainer before
enabling required code-owner approval.

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
