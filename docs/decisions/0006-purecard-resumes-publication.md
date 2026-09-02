# 0006. PureCARD resumes publication as `purecard`

- **Status:** Accepted
- **Date:** 2026-09-02
- **Deciders:** Project maintainer (authorization); agent (implementation)
- **Supersedes:** rule 6 of
  [ADR-0004](0004-purecard-independent-workspace-product.md)

## Context

[ADR-0004](0004-purecard-independent-workspace-product.md) rule 6 froze the
migrated PureCARD package as unpublished: `publish = false`, with maturin wheels
declared verification artifacts rather than releases. That rule was written when
migration had just happened and no release channel had been re-authorized.

PureCARD was published standalone as `purecard` v0.1.0 on crates.io on
2026-07-10, before the migration. Folding it into this workspace renamed the
Cargo package to `pure-analyzer-purecard` — matching the analyzer's crate-naming
convention — and set `publish = false` specifically to retire that listing. The
crates.io listing still exists and is not yanked.

The maintainer has now authorized resuming publication and provisioned the two
credentials it needs (`CARGO_REGISTRY_TOKEN`, `PYPI_API_TOKEN`). That makes rule
6 the only thing standing between the product and a release channel, and raises
a naming question the rename cannot dodge: publishing as
`pure-analyzer-purecard` would strand the existing `purecard` listing and mint a
second one that looks like a different, analyzer-owned product.

Two forces are in tension. The workspace convention says a crate is named after
its directory and its siblings. Package identity on a public registry is a
promise to consumers, who already have `purecard` in their lockfiles and
`import purecard` in their code.

## Decision

We will publish the PureCARD crate to crates.io as **`purecard`**, resuming its
pre-migration identity, and publish its abi3 wheels to PyPI under the same name.
Registry identity wins over workspace naming convention where the two conflict.

Only the Cargo package identity changes. The workspace-member directory stays
`crates/pure-analyzer-purecard/`, and `[lib] name` stays `purecard` — so the
Rust library, the PyO3 module, and the Python distribution all already agree
with the package name, and the directory is the sole remaining place the longer
name appears.

`purecard` becomes the only crate in this workspace with `publish = true`. Both
halves of a release are cut from release-plz's release PR: crates.io by
release-plz itself, PyPI by a workflow triggered on the published GitHub
Release. An ordinary merge to `main` still releases nothing.

## Alternatives considered

- **Publish as `pure-analyzer-purecard`.** Rejected. It abandons a live
  crates.io listing and creates a second one whose name implies analyzer
  ownership — the exact coupling ADR-0004 exists to prevent. The rename would
  also have to reach `[lib] name`, or the package and library would disagree.
- **Rename the directory to `crates/purecard/` for consistency.** Rejected as
  churn: it moves every path in the workspace's automation, docs, and CI filters
  to buy symmetry that no consumer observes. Cargo does not require the two to
  match, and ADR-0004's boundary is enforced by package name, not by path.
- **Keep `publish = false` and distribute wheels only.** Rejected. It splits the
  product's identity across two registries with different names and leaves Rust
  consumers on a stale 0.1.0 that no longer reflects the code.
- **Yank crates.io `purecard` 0.1.0 and start clean.** Rejected. Yanking does
  not free the name, breaks existing lockfiles that resolve to it, and buys
  nothing the next version bump does not.

## Consequences

- PureCARD holds independent release authority. Its version and release cadence
  are its own; no analyzer crate is published under its release. This is part of
  the ADR-0004 product boundary, not an exception to it.
- The workspace now has a naming exception that must be stated wherever it could
  mislead: the directory and the package differ for exactly one crate.
- Publishing is irreversible in a way the repository's other outputs are not — a
  published version can be yanked but never withdrawn. `just package`
  verify-builds the real `.crate` tarball pre-merge so a packaging defect fails a
  PR rather than a release.
- The first release must clear the version already on crates.io. The workspace
  is at 0.1.0 and crates.io `purecard` 0.1.0 exists, so release-plz must bump
  past it or the publish is rejected.
- We would revisit this if PureCARD's API churn made a public listing actively
  misleading, or if the products' ownership ever merged — at which point the
  naming argument here would be void.
