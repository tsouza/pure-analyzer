# 0004. Keep PureCARD as an independent workspace product

- **Status:** Accepted
- **Date:** 2026-08-26
- **Deciders:** Project maintainer and agent

## Context

PureCARD began as a standalone Legend Pure constrained-decoder repository. It
was migrated to `crates/pure-analyzer-purecard/` so one checkout can provide a
shared Rust toolchain, dependency policy, quality gates, and CI orchestration.
The analyzer and decoder concern the same language, and PureCARD owns a useful
engine-backed and offline corpus, but those similarities do not establish a
shared runtime abstraction or product contract.

The products are at different stages and expose different guarantees:

- `pure-analyzer` is currently an early scaffold for a static-analysis pipeline.
  Lexer and diagnostics are substantive; most higher layers are version stubs,
  and CLI subcommands are not implemented.
- PureCARD is a grammar-constrained decoder with an implemented partial schema
  overlay, its own Rust API, PyO3 boundary, corpus, fuzz targets, and specialized
  test lanes. Its M0–M5 code artifacts exist, but its documented end-to-end
  proof obligations remain open, so it does not yet claim feature completeness.

Cargo workspace membership alone cannot express whether one is a layer of the
other. Without an explicit boundary, convenient local imports or corpus reuse
could silently turn governance co-location into permanent architectural and
ownership coupling.

This root decision mirrors PureCARD's product-local
[ADR-0009](../../crates/pure-analyzer-purecard/docs/decisions/0009-monorepo-placement.md).
[ADR-0003](0003-analysis-engine-crate-dag.md) remains scoped exclusively to the
analyzer product.

## Decision

The repository is an umbrella containing two independent sibling products:
`pure-analyzer` and `pure-analyzer-purecard` (PureCARD).

The product boundary has these rules:

1. Analyzer crates and PureCARD have **zero Cargo dependency edges in either
   direction**. The prohibition covers normal, development, build, optional,
   and renamed dependencies.
2. PureCARD is not a node in ADR-0003's analyzer DAG and is not a front end over
   `libpure`.
3. `xtask`, the root `just` interface, root CI, toolchain declarations, and root
   governance may orchestrate or constrain both products. They are shared
   repository infrastructure, not a third product and not an analyzer layer.
4. Each product retains ownership of its runtime code, domain records, specs,
   ADRs, tests, and corpus. Co-location grants no implied right to reuse or
   redefine the other product's assets.
5. Sharing a parser implementation, model type, corpus, or other product asset
   requires a future change with both a dedicated spec and a new ADR. That ADR
   must identify ownership direction, stable contract, versioning impact, and
   why the zero-edge boundary is no longer preferable.
6. The migrated PureCARD Cargo package remains unpublished with
   `publish = false`. CI may build and test its maturin wheel, but that wheel is
   a verification artifact, not authorization to publish to PyPI or another
   registry.

`cargo xtask verify-layering` enforces the analyzer DAG and this cross-product
boundary against root `cargo metadata` plus the manifests of workspace-excluded
fuzz/lint packages. Repository infrastructure and third-party dependencies
remain outside the product-edge prohibition.

## Alternatives considered

- **Make PureCARD an analyzer layer or `libpure` front end.** Rejected because
  the decoder neither consumes analyzer internals nor implements an analyzer
  surface. It has a different runtime role, API, test oracle, and release
  posture.
- **Immediately extract a shared parser or corpus package.** Rejected because
  similarity has not yet established one stable abstraction or ownership model.
  Premature extraction would couple two independently evolving grammar and
  oracle concerns.
- **Return PureCARD to a standalone repository.** Rejected because it would
  duplicate toolchain, dependency, CI, and governance maintenance without
  improving runtime independence.
- **Publish the renamed crate or CI-built wheel.** Rejected because the move is
  source-governance co-location, not creation of a supported release channel.

## Consequences

- One checkout and one root automation surface can validate both products while
  their runtime graphs remain independent.
- Analyzer refactors cannot silently become PureCARD dependencies, and PureCARD
  cannot become an undeclared analyzer feature.
- Cross-product reuse carries deliberate spec-and-ADR overhead. That friction is
  intentional and makes ownership, compatibility, and release consequences
  reviewable before code couples the products.
- PureCARD's package name follows workspace conventions while its Rust library
  and Python module remain `purecard`.
- The boundary may be revisited only when measured duplication or a supported
  integration justifies a stable shared contract.
