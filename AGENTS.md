# Agent rules

Read [`constitution.md`](constitution.md) and [`CLAUDE.md`](CLAUDE.md) before
changing this repository. Their rules apply to every agent and worktree.

## Mandatory project identity — PROTECTED

- Every human commit, branch, issue, pull request, review, merge, release, and
  repository-setting change uses GitHub account **`tsouza`**. Never use
  **`tsouza-squid`** for this project.
- Effective Git author and committer must both be exactly
  `Thiago Souza <122435+tsouza@users.noreply.github.com>`.
- `origin` fetch and push URLs must both be exactly
  `git@github.com-tsouza:tsouza/pure-analyzer.git`; the account-specific SSH
  alias is load-bearing.
- Run `just project-identity` before committing. Declarative pre-commit and
  pre-push hooks enforce the same values.
- Run GitHub CLI operations through `just github <gh arguments>`. It obtains
  the stored `tsouza` token from the canonical OS-user config, verifies that
  token as exactly `tsouza`, and pins the same token and `github.com` host
  through execution. Never run `gh auth switch` except the one repair form
  `just github auth switch --user tsouza`.
- GitHub Actions permits a human pull-request author only when the event says
  `tsouza`. An event-authoritative `Bot` account such as Dependabot remains
  allowed.

These checks are independent layers, not substitutes for one another. Do not
bypass, disable, or weaken one because another layer is present.
