# Spec: revert-premature-cli-libpure-dep

- Status: draft
- Created: 2026-07-22
- Owner: agent (fold-vs-branch fix, discovered mid-work on the lexer feature)

## Problem

The bootstrap-instantiation audit found `pure-analyzer-cli` didn't depend on
`libpure` despite the design doc/ADR-0003 describing that edge, and recommended
adding it "as soon as the first real subcommand lands" — since every subcommand
currently just `anyhow::bail!("not implemented yet")`, there's no real usage
yet. I added the dependency anyway during the audit-fix pass, ahead of that
condition. `cargo machete` (`just machete`, part of `just ci-full`, not the
fast `just ci` — which is why this didn't surface until now) correctly flags it
as unused: nothing in `crates/pure-analyzer-cli/src` references `libpure::`.

Discovered while starting the `lexer` feature, in a different worktree — this
fix is unrelated to that work, so it gets its own branch per constitution §2.

## Goals

- [x] `pure-analyzer-cli/Cargo.toml` no longer declares `libpure` until a real
      subcommand actually calls into it.
- [x] `just machete` clean again.

## Non-goals

- Not implementing any real CLI subcommand here — that's separate, larger
  future work (needs `pure-analyzer-analysis`/`libpure` to have real logic
  first, which doesn't exist yet either).

## Design

One-line revert in `crates/pure-analyzer-cli/Cargo.toml`.

## Testing plan

`just machete` and `just ci` both green.

## Risks & rollout

None — reverts an unused, premature addition back to the audited-correct
"deferred" state.
