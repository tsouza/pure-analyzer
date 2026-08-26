# Constitution

The non-negotiable rules of this project. Everything else in the repo — code,
docs, lessons, ADRs — is subordinate to this file.

Two tiers of rule live here:

- **EVOLVABLE** rules describe *the domain* (the "what"). The agent may add,
  refine, or retire them through the normal PR + reviewer flow as understanding
  grows.
- **PROTECTED** rules are the guardrails that keep the system honest. The agent
  may only ever make them **stricter**. Loosening a PROTECTED rule requires an
  explicit maintainer decision (see `docs/methodology/self-learning.md`).

Every rule is tagged. If a rule is untagged, treat it as PROTECTED.

---

## 1. Language, safety, and layering — PROTECTED

- Rust, edition 2024, latest stable toolchain pinned in `rust-toolchain.toml` —
  the *single* declaration of the toolchain. CI does not re-declare it with a
  `dtolnay/rust-toolchain` step or a `rustup default`; a second declaration only
  fights the file over which pin wins. Reach for a toolchain action solely when a
  job needs a channel the file doesn't provide (e.g. a one-job nightly install).
- `#![forbid(unsafe_code)]` in every crate. No exceptions, no `allow`.
- `#![deny(missing_docs)]` on every public crate (`pure-analyzer-lexer`,
  `pure-analyzer-syntax`, `pure-analyzer-parser`, `pure-analyzer-model`,
  `pure-analyzer-resolve`, `pure-analyzer-analysis`, `pure-analyzer-diagnostics`,
  `libpure`, `pure-analyzer-cli`, `pure-analyzer-purecard`). Public items are
  documented or they do not merge.
- The analyzer product follows the processing pipeline in ADR-0003: `lexer →
  syntax → parser → model → resolve → analysis → libpure → cli`,
  with `pure-analyzer-diagnostics` a shared leaf. Cargo dependencies point
  toward prerequisites; specifically, `pure-analyzer-resolve` may depend on
  `pure-analyzer-model`, and the reverse edge is forbidden. Only analyzer front
  ends (`pure-analyzer-cli` today; `pure-analyzer-lsp` in the target design) may
  depend on a renderer (`ariadne`, `codespan-reporting`) or protocol crate
  (`clap`, later `tower-lsp`/`lsp-types`).
- The umbrella contains a second, independent product,
  `pure-analyzer-purecard`
  ([ADR-0004](docs/decisions/0004-purecard-independent-workspace-product.md) and
  its nested
  [ADR-0009](crates/pure-analyzer-purecard/docs/decisions/0009-monorepo-placement.md)).
  Analyzer crates and PureCARD have **zero Cargo dependency edges in either
  direction**, across normal, development, build, optional, and renamed
  dependencies. `xtask` and
  root automation are shared infrastructure, not a product layer. Co-location
  grants no permission to share a parser, corpus, or product ownership; any
  such integration requires its own spec and ADR.
- `cargo xtask verify-layering` checks the analyzer allow-set and the
  analyzer–PureCARD boundary against `cargo metadata`; the
  `no-front-end-deps-in-core` ast-grep rule independently protects analyzer
  core crates. These are enforced facts, not conventions.
- No `unwrap`, `expect`, `panic!`, `todo!`, `unimplemented!`, or `dbg!` outside
  `#[cfg(test)]`. Libraries return `Result` with `thiserror` types; boundaries
  (`pure-analyzer-cli`, `xtask`) may use `anyhow`.
- Structured `tracing` from the first commit. No `println!` for diagnostics.

## 2. Change discipline — PROTECTED

- One change, one branch, one PR. Every branch gets its own git worktree.
- Before opening a PR, the branch must descend from current `origin/main` (check
  `git merge-base`); rebase onto it. If the branch diverged far back and
  re-implemented since-merged work, re-cut it off `origin/main` and cherry-pick
  the change rather than rebasing its whole history — so CI's first run is
  meaningful, not a conflict storm against a stale base.
- Conventional Commits for every commit message.
- `just` is the only supported entry point. If a `just` target you need does not
  exist, build it — do not run raw `cargo` incantations in CI or docs.
- **No shell scripts. — PROTECTED.** The repository contains zero `.sh`/`.bash`
  files and no non-trivial inline shell in CI or hooks. Automation lives, in
  order of preference: (1) a `just` target or `cargo xtask` subcommand; (2) a
  first-party GitHub Action step; (3) a Bun-executed `.mjs` script
  (`#!/usr/bin/env bun`, using Bun's `$` shell API). A `run:`/recipe line that
  merely invokes one tool is fine; anything with branching, loops, or piped
  logic becomes xtask or `.mjs`. Keep single-tool pass-throughs *out* of xtask
  specifically: a nested `cargo xtask` → `cargo <plugin>` call can mangle the
  plugin's argv, so a one-tool recipe stays a plain `just` line. Every `.mjs`
  draws on the shared library under
  `scripts/lib/` — duplicated scripting logic is a DRY defect (see §4). Git
  hooks are declared in `lefthook.yml`, never hand-written shell.
- **Portable automation — favor built-in functions. — PROTECTED.** `just`/`xtask`
  targets and CI steps run on every contributor's platform, not just Linux CI.
  When crafting automation — `xtask` especially — favor a built-in or in-process
  function over shelling out to a platform-specific binary: hash with a Rust
  crate, not `sha256sum` (absent on macOS/Windows); read and parse a file in Rust,
  not `sed`/`awk`. Shell out only for a genuine external tool (java, cargo, buf),
  through the shared process helpers, with a clear "is it installed?" error. A
  gate that only runs on the maintainer's OS is a portability bug, not a gate.
- Nothing merges red. `just ci` is the **fast inner-loop gate** (layering, fmt,
  clippy, test) — necessary but **not sufficient**: the full gate is the CI
  matrix (coverage, mutation, supply-chain, api-stability, structural,
  postponed-markers, proto, docs, script tests), mirrored locally by `just
  ci-full`. A green `just ci` does not imply a green CI; "nothing merges red" is
  enforced by CI, and every check CI runs is reachable through `just` (§2.4).
- **Gates run clean — warnings are errors. — PROTECTED.** A gate is green only
  when it is also quiet: no unaddressed tool warnings. The `-D warnings` standard
  that governs clippy and rustdoc extends to *every* tool a gate runs — when a
  tool asks for a configuration (a flag, an env var), apply it rather than
  tolerate a warning on every run. A recurring, silenceable warning is rot; it is
  fixed at its source, never normalized or grepped away.
- **Gates run pre-merge and reproduce in CI. — PROTECTED.** Every invariant is
  checked on the PR, not only after it lands. Automation that fires only on push
  to `main` — release cutting, changelog/version bumps, deploys — must have a
  pre-merge counterpart that validates its config against the workspace, so a
  drift fails a PR instead of reddening the trunk. A gate must pass or fail for
  reasons reproducible in CI's own environment; it may not lean on local-only
  state (a branch's upstream, full git history, an interactive tool). When a
  tool can't run faithfully in a PR's detached-`HEAD` checkout, reproduce the
  invariant it enforces in a form that can — e.g. diff the `release-plz.toml`
  package overrides against `cargo metadata` rather than invoking `release-plz`.
- **Latest stable, verified — PROTECTED.** When pinning a new third-party tool
  or dependency, or bumping an existing pin, look up its actual current stable
  release from its authoritative source (crates.io, the project's GitHub
  releases, etc.) at the time of the change. Never carry over a version from
  memory or training data — those go stale. A pin is only as trustworthy as
  the check that produced it. For crates, `cargo add <name>` (no version) writes
  the current release; Dependabot is a last-mile safety net, not the mechanism
  that makes a pin current in the first place.
- **Cache or mirror third-party CI fetches. — PROTECTED.** When writing or
  changing a CI pipeline, any step that pulls from a third-party source — directly
  (a `curl`/download) or indirectly (an action that fetches a binary or dataset) —
  must be made reproducible and resilient: restore it from the GitHub Actions
  cache keyed on its pinned version, and/or pull from a first-party or Azure
  mirror. A gate that re-downloads an artifact from an external host on every run
  is a flake and a supply-chain liability — it reddens a PR the moment upstream
  rate-limits, moves, or 404s, for reasons unrelated to the change. Prefer
  first-party Actions (which cache themselves); when a raw download is
  unavoidable, gate it behind `actions/cache` on the version key so the fetch
  runs on a cache miss only. Never leave a bare, uncached `curl | …` in a job.

## 3. Testing — PROTECTED

- No test is skipped, ignored, or marked `#[ignore]` to make CI pass. The gate
  that forbids skip markers is itself PROTECTED.
- Flakes are bugs. Fix the flake immediately; never weaken or delete an
  assertion to silence one.
- Coverage floor, mutation-score floor, and every other numeric gate may be
  raised by the agent and lowered only by a human.

## 4. Craft — EVOLVABLE where noted

- **DRY / KISS.** Duplication and incidental complexity are defects. (EVOLVABLE
  in the specifics; the principle is PROTECTED.)
- **Comment economy — PROTECTED.** Comments explain *why* for genuinely exotic
  logic only. If code needs a comment to be understood, the code is wrong. No
  narrating comments, no commented-out code.
- **No magic constants — PROTECTED.** Named constants or config, always.
- **Library before writing — EVOLVABLE.** Prefer a vetted dependency over
  bespoke code, but only after it clears the vetting rubric
  (`docs/methodology/overview.md`). Otherwise, write our own. Reach first for a
  crate already in the lockfile before adding a new pin — and remember a new pin
  can't clear "latest stable, verified" (§2) when its current version can't be
  looked up. Bespoke code chosen over a dependency owns the format's edge cases:
  a hand-rolled parser is tested against inline comments, whitespace, and
  quoting, or it silently corrupts.

## 5. Fix the system, not the instance — PROTECTED

Every bug fix must also close the *class* of bug: a new test, lint, hook, or
rule that would have caught it. A fix that only patches the one instance is
incomplete and the reviewer will reject it.

## 6. Pre-existing issues — PROTECTED

When the agent discovers an unrelated pre-existing problem mid-change, it judges
**fold vs. branch** (fix here, or file and defer) and **must justify the call in
the PR description**. The reviewer checks that judgment.

## 7. Anti-gaming — PROTECTED

The agent may not tamper with, disable, or self-lower any quality gate, threshold
file, held-out test suite, or reviewer configuration. CI recomputes gate values
from the submitted branch and anti-gaming tests reject mechanically detectable
weakening; every attempted weakening is also a reviewer red flag. Policy and
implementation share a repository, so residual changes that cannot be classified
mechanically require explicit maintainer judgment.

---

## The domain (EVOLVABLE)

This repository is an umbrella for two Legend Pure products:

- **`pure-analyzer`** is a deterministic, standalone static-analysis toolchain
  for the modern `Relation<>` dialect. Its current implementation is an early
  scaffold: lexer and diagnostics are substantive; syntax, parser, model,
  resolve, analysis, and `libpure` are mostly version stubs; CLI subcommands are
  not implemented. Its design document describes target behavior, not shipped
  capability.
- **`pure-analyzer-purecard` (PureCARD)** is a grammar-constrained decoder with an
  implemented partial schema overlay. Its M0–M5 code artifacts are implemented,
  but its documented end-to-end proof obligations remain open, so it does not
  yet claim feature completeness. It is migrated into this workspace but remains an
  independently owned, unpublished product (`publish = false`; wheels are CI
  verification artifacts, not releases).

Both products are mechanical and domain-agnostic: no customer data or private
domain content belongs in either. Their independent-product boundary is a
domain invariant; sharing repository infrastructure does not merge their
runtime architecture, source ownership, test corpora, or release authority.
[`docs/domain-model.md`](docs/domain-model.md) elaborates these rules.
