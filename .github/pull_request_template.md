<!--
Keep changes small and focused: one worktree, one branch, one PR per change.
The reviewer subagent and CI both read this template — fill it out honestly.
-->

## What & why

What does this change do, and what problem does it solve? Link the spec/issue.

Closes #

## Crate(s) touched

- [ ] pure-analyzer-lexer
- [ ] pure-analyzer-syntax
- [ ] pure-analyzer-parser
- [ ] pure-analyzer-model
- [ ] pure-analyzer-resolve
- [ ] pure-analyzer-analysis
- [ ] pure-analyzer-diagnostics
- [ ] libpure
- [ ] pure-analyzer-cli
- [ ] pure-analyzer-purecard
- [ ] PureCARD corpus / oracle fixtures
- [ ] PureCARD Python wheel / FFI
- [ ] tooling / CI

## Testing

How is this verified? Note the layers exercised (unit / integration / chaos /
mutation / fuzz), any PureCARD hermetic/live oracle boundary, and any Python
boundary coverage. Flaky tests are not acceptable.

## Pre-existing issues

If you touched code with a pre-existing problem, state whether you FOLDED the
fix into this PR or BRANCHED it out, and justify the choice.

## Checklist

- [ ] `just ci` passes locally.
- [ ] `just ci-full` passes for changes that touch slow or specialized gates.
- [ ] `just review` (structural rules, unused deps, secret scan) is clean.
- [ ] No `unwrap`/`expect`/`todo!`/`unimplemented!` outside tests.
- [ ] Public API / diagnostic-code changes are intentional; stability gates pass
      or are accompanied by a justified version bump.
- [ ] PureCARD remains unpublished and independent from analyzer internals; any
      parser/corpus sharing has a dedicated ADR.
- [ ] Docs updated (`#![deny(missing_docs)]` on public items) and `just docs` passes.
- [ ] Conventional-commit title (feat/fix/chore/...); breaking changes marked `!`.
