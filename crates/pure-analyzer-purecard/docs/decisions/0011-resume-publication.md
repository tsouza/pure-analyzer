# 0011. Resume publishing PureCARD to crates.io and PyPI

- **Status:** Accepted
- **Date:** 2026-09-02
- **Deciders:** Thiago Souza (authorization); agent (implementation)
- **Supersedes:** the unpublished-package posture of
  [ADR-0009](0009-monorepo-placement.md)

## Context

[ADR-0009](0009-monorepo-placement.md) settled PureCARD's placement as a
colocated but independent product, and recorded its release posture as
unpublished: `publish = false`, wheels built only to verify the PyO3 boundary.
It named "treat a wheel build as a published product" as a failure mode to guard
against, because at the time no release channel had been authorized and a
CI-built wheel was the closest thing to one.

That condition has changed. The maintainer has authorized publication and
provisioned registry credentials. The root-level counterpart to this record is
[ADR-0006](../../../../docs/decisions/0006-purecard-resumes-publication.md),
which owns the package-identity argument; this ADR records what changes inside
the product.

## Decision

We will publish this crate to crates.io as `purecard` and its abi3 wheels to
PyPI under the same name. The wheel stops being a verification artifact and
becomes the released artifact, which raises the bar it must clear.

Concretely, the wheel build expands from one Linux x86_64 leg to five native
legs — Linux x86_64 and aarch64, macOS x86_64 and arm64, Windows x64 — each
building on its own architecture so its smoke-import genuinely loads the wheel
that leg produced. That matrix is defined once, in a reusable workflow, and used
unchanged by both the per-PR lane and the release lane.

## Alternatives considered

- **Keep wheels as verification artifacts and publish only the crate.** Rejected.
  The Python boundary is how most consumers reach this decoder; publishing the
  Rust half alone serves the smaller audience and leaves the larger one building
  from source.
- **Publish from the existing single-platform build.** Rejected. A Linux-only
  wheel makes every macOS and Windows consumer compile a Rust extension at
  install time, which is exactly the failure abi3 wheels exist to avoid.
- **Cross-compile the extra platforms from Linux runners.** Rejected. A
  cross-compiled wheel cannot be imported on the runner that produced it, so the
  smoke test degrades from "CPython loads and uses this" to "the cdylib linked" —
  on precisely the platforms we have the least other coverage for.
- **Separate build definitions for the CI lane and the release lane.** Rejected
  as a DRY defect with teeth: the two would drift, and the drift would surface
  as a green PR against a matrix the release does not use.

## Consequences

- The abi3 floor stays declared rather than exercised: wheels are built and
  imported under CPython 3.12 only, so `requires-python >=3.9` asserts forward
  compatibility no interpreter matrix checks. Widening that is a separate change.
- Four non-Linux runners join CI for the first time, so their toolchains' output
  now feeds the repository's warnings-are-errors log sweep.
- Releases are irreversible. `just package` verify-builds the crates.io tarball
  pre-merge, and the release lane must be gated so it fires once per PureCARD
  release rather than once per workspace crate.
- We would revisit the matrix if a platform's runner became unavailable, or if
  demand appeared for a musl or free-threaded-CPython target.
