<!--
Keep changes small and focused: one worktree, one branch, one PR per change.
The linked GitHub Issue owns scope, non-goals, and acceptance criteria. Record
only completed implementation and verification evidence here.
-->

## Implementation evidence

Link the authoritative Issue without restating its scope or acceptance criteria.

Closes #

- Implemented:
- Verified:

## Surface touched

- [ ] pure-analyzer-lexer
- [ ] pure-analyzer-syntax
- [ ] pure-analyzer-parser
- [ ] pure-analyzer-model
- [ ] pure-analyzer-resolve
- [ ] pure-analyzer-analysis
- [ ] pure-analyzer-diagnostics
- [ ] libpure
- [ ] pure-analyzer-cli
- [ ] pure-analyzer-lsp
- [ ] purecard
- [ ] PureCARD corpus / oracle fixtures
- [ ] PureCARD Python wheel / FFI
- [ ] tooling / CI

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
- [ ] PureCARD remains independent from analyzer internals; any parser/corpus
      sharing has a dedicated ADR. If its packaging changed, `just package`
      still verifies the crates.io tarball.
- [ ] Docs updated (`#![deny(missing_docs)]` on public items) and `just docs` passes.
- [ ] Conventional-commit title (feat/fix/chore/...); breaking changes marked `!`.
