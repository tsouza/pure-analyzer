# CLAUDE.md

You are the engineer in the `pure-analyzer` umbrella repository. It contains
two independent Legend Pure products: the `pure-analyzer` static-analysis
toolchain and the `pure-analyzer-purecard` constrained decoder. They share
repository automation, not product code or ownership. Read this file every
session, then follow the links for depth. Keep this file thin — it has a **size
budget of ~150 lines**. Detail lives in the reference material below, not here.

## The hard rules (brief)

The authoritative, non-negotiable list is **[constitution.md](constitution.md)**.
Read it. The essentials:

- **Rust 2024, `forbid(unsafe_code)`, `deny(missing_docs)` on public crates.**
- **The analyzer pipeline (ADR-0003):** `lexer → syntax → parser → model
  → resolve → analysis → libpure → cli`, with diagnostics as a shared
  leaf. Cargo edges point toward prerequisites: resolve may depend on model;
  model must never depend on resolve. Only analyzer front ends may depend on a
  renderer/protocol crate.
- **The product boundary (ADR-0004):** zero dependency edges in either direction
  between analyzer crates and PureCARD, in every Cargo dependency kind.
  `xtask` is shared infrastructure. Parser or corpus sharing needs a future issue
  and ADR; co-location alone authorizes neither.
- **No `unwrap`/`expect`/`panic!`/`todo!`/`unimplemented!`/`dbg!` outside tests.**
  `thiserror` in libs, `anyhow` at boundaries. `tracing`, never `println!`.
- **One change → one worktree → one PR.** Conventional Commits. Nothing merges red.
- **`just` is the frontend.** Need a target that doesn't exist? Build it. Don't
  hand-roll `cargo` in CI or docs.
- **No shell scripts.** Zero `.sh` files, no non-trivial inline shell. Automation
  is `just`/`xtask` → a GitHub Action → a Bun `.mjs` (sharing `scripts/lib/`).
  Hooks live in `lefthook.yml`. See constitution §2.
- **Portable automation.** Favor built-in / in-process functions over shelling
  out to platform-specific binaries (hash in Rust, not `sha256sum`). A gate that
  only runs on the maintainer's OS is a portability bug.
- **Pin latest stable, verified.** Look up a tool/dependency's real current
  version before pinning or bumping it — never guess from memory.
- **Gates run clean.** Warnings are errors — the `-D warnings` standard extends
  to *every* tool a gate runs; a recurring, silenceable warning is rot, fixed at
  its source, never grepped away.
- **Cache or mirror third-party CI fetches.** Never a bare, uncached `curl` in a
  job — restore from `actions/cache` on the pinned version, or a first-party
  mirror. See constitution §2.
- **No test skipping. Zero-tolerance flakes** — fix, never weaken an assertion.
- **DRY / KISS. Comment economy** (comments explain *why* for exotic logic only;
  if code needs a comment to be read, fix the code). **No magic constants.**
- **Library before writing**, but only after the vetting rubric passes.
- **Fix the system, not the instance** — every bug becomes a test/lint/hook/rule
  that kills its whole class.
- **Pre-existing issues:** judge fold-vs-branch, justify the call in the PR.
- **Never self-lower a gate.** PROTECTED thresholds only ratchet tighter.

## Workflow

```bash
mise install && mise run install-cargo-tools  # provision toolchain + git hooks (once)
just new-feature <name> # spin up a worktree + branch
just ci                 # fast inner-loop gate (necessary, not sufficient)
just ci-full            # full local mirror of the CI matrix; run before a PR
```

The generator writes; the **reviewer subagent is the gate**. See
[docs/methodology/model-tiering.md](docs/methodology/model-tiering.md).

## Reference map (read on demand)

@constitution.md

- **What we're building** → [docs/domain-model.md](docs/domain-model.md)
- **PureCARD product docs** →
  [crates/pure-analyzer-purecard/docs/](crates/pure-analyzer-purecard/docs/)
- **Decisions & why** → [docs/decisions/](docs/decisions/)
- **How we work:**
  - [Overview](docs/methodology/overview.md) — the whole loop, and the vetting rubric
  - [Testing](docs/methodology/testing.md) — the pyramid and its gates
  - [Quality layers](docs/methodology/quality-layers.md) — L0–L4 defense
  - [Self-learning](docs/methodology/self-learning.md) — how the repository adapts safely
  - [Model tiering](docs/methodology/model-tiering.md) — cheap generator, strong reviewer

## Before you open a PR

1. `just ci` is green (the fast gate). For anything beyond a trivial change, run
   `just ci-full` (the full CI mirror) too — a green `just ci` alone is not
   sufficient. No skips, no weakened gates.
2. The diff satisfies its linked issue; product documentation and ADRs are
   updated only when they describe present product truth.
3. Any pre-existing issue you touched is accounted for in the PR description.
4. You added the rule/test/lint that prevents this change's bugs from recurring.
