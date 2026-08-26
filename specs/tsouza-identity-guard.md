# Spec: tsouza-identity-guard

- **Status:** complete
- **Date:** 2026-08-26
- **Owner:** shared repository governance

## Context

The workstation can hold valid sessions for both `tsouza` and
`tsouza-squid`. GitHub CLI chooses an active session independently from Git's
author configuration and SSH key selection, so a remembered account switch is
not a durable boundary. The maintainer made a hard project rule: every human
development and GitHub write uses `tsouza`, never `tsouza-squid`. This rule
supersedes any earlier convention to restore or use another account later.

## Goal and acceptance criteria

- [x] Make the human identity a PROTECTED rule in the constitution and the root
  instructions consumed by Codex and Claude.
- [x] Require the exact effective Git author and committer
  `Thiago Souza <122435+tsouza@users.noreply.github.com>`.
- [x] Require the exact account-specific origin fetch/push URL
  `git@github.com-tsouza:tsouza/pure-analyzer.git`.
- [x] Enforce both requirements in declarative pre-commit and pre-push hooks.
- [x] Provide a Bun GitHub CLI entry point that ignores command-local config
  redirection, obtains the stored `tsouza` token from the canonical OS-user
  config, pins it to `github.com`, verifies that same token with `gh api user`,
  and executes only with the same pinned credential boundary.
- [x] Allow only a bare `gh auth switch --user tsouza` as an unauthenticated
  repair; reject path-prefixed executables and missing, ambiguous, or different
  switch targets.
- [x] Add a Claude Code `PreToolUse` Bun hook that refuses raw `gh`, except the
  exact repair switch, and checks identity before Git object creation/push.
- [x] Add offline unit and command-boundary coverage by injecting effects and
  using temporary Git repositories/event files.
- [x] Make the required lint aggregate reject non-`tsouza` human actors and PR
  authors while allowing event-authoritative GitHub `Bot` identities.
- [x] Inspect every PR-range commit: `tsouza` PRs require the exact author and
  committer; bot PRs require a matching numeric GitHub noreply identity.

## Threat model

The primary threat is accidental cross-account use when several valid local
credentials coexist. The guard also treats command-local credential/config
overrides, skipped hooks, extra push URLs, shell indirection, extension commands,
and forged human commit metadata as bypass attempts. Local hooks give early
feedback; the required CI job independently checks event identities and the PR
commit range without a network or credential dependency. The wrapper owns
GitHub CLI execution, pins one verified `tsouza` token across verification and
execution, rejects token-display and explicit Authorization/host overrides, and
normalizes config paths before any effect. Git transport and agent guards reject
command-local SSH/config/home overrides that could redirect account selection.

## Non-goals and enforcement limits

- This does not remove another account from the workstation or change global
  Git/GitHub configuration. It never recommends restoring another account.
- Repository code cannot intercept browser/API issue creation before GitHub
  receives it. GitHub has no per-repository setting that prohibits one account
  from opening issues while leaving the account otherwise unblocked. The
  PROTECTED rule, GitHub CLI wrapper, and agent hook cover project-operated
  paths; globally blocking the account would be separate server administration.
- A `merge_group` event does not expose each queued PR author. The required
  pull-request run checks the author before queue admission; merge-group and
  push events can still reject a visible non-project human actor or sender.
- PreToolUse is defense in depth, not a shell-language security sandbox. It
  refuses raw and recognizable indirect mutation forms, while native hooks, the
  wrapper, CI commit-range scan, and review remain authoritative independent
  layers. Low-level Git object plumbing is closed by the CI history scan.
- Genuine bots are identified by GitHub's payload `type: Bot` plus the reserved
  `[bot]` login suffix. Bot commit mail must also be the matching numeric GitHub
  noreply address. A bot-shaped string with user type `User` is rejected.

## Design

`scripts/lib/project-identity.mjs` owns constants and pure policy functions.
The local/CI checker, token-pinned wrapper, and Claude hook import that module
so the identity cannot drift between layers. `scripts/checks/project-identity.mjs
git` reads effective identities with `git var`, which observes environment
overrides as well as config. The pre-push hook additionally checks the remote
name and URL supplied by Git itself. Its `ci` mode reads the checked-in GitHub
event payload and local commit graph, with no identity API call.

## Verification

Run `just test-scripts`, `just project-identity`, `just lint-actions`, and
`just review`. Negative tests prove wrong author/email/remotes, forbidden or
unavailable credentials, active-account races, wrong auth switches,
non-maintainer humans, bot-shaped users, and mismatched PR commit identities all
fail without executing a mutation.

## Rollback

This is a PROTECTED ratchet. Loosening or removing any layer requires an
explicit maintainer decision under the constitution; ordinary agents may only
tighten it.
