# 0009. Colocate PureCARD as an independent monorepo product

- **Status:** Accepted; the unpublished-package posture is superseded by
  [ADR-0011](0011-resume-publication.md)
- **Date:** 2026-08-26
- **Deciders:** Thiago Souza; Codex

## Context

PureCARD was developed as a standalone constrained-decoder project and has now
been folded into the Pure Analyzer repository. Colocation makes one checkout and
one automation surface sufficient for changes that must keep the products' Rust
toolchains, quality policy, and CI behavior aligned. It does not make PureCARD an
analyzer subsystem: the decoder has a different domain, public API, corpus, and
release posture from the analysis engine.

Cargo workspace membership alone does not state that distinction. Without an
explicit boundary, future changes could import analyzer parser internals into
PureCARD, import the decoder into the analyzer, or treat a wheel build as a
published product. Those changes would silently replace colocation with
architectural coupling.

## Decision

We will keep PureCARD in this monorepo as an **independent sibling product**. Its
Cargo package is `pure-analyzer-purecard`, while the Rust library and Python
module remain `purecard`. The package has `publish = false`; maturin wheels are
verification artifacts only.

PureCARD and the analyzer have zero dependency edges in either direction. They
may share only repository-level governance and orchestration: the root `just`
frontend, CI, constitution, methodology, toolchain, and workspace policy. The
root dependency-topology gate classifies PureCARD separately from the analyzer
DAG and rejects a cross-product edge in any Cargo dependency kind.

Any future proposal to share parser implementation, model types, or corpus data
across the product boundary requires a new ADR first. That ADR must identify the
ownership direction, public contract, versioning implications, and why a
dependency-free boundary is no longer the better design.

### Supersession after the monorepo migration

This decision supersedes the standalone-repository, package-publication, and CI
placement consequences in accepted ADRs 0002–0006 wherever they conflict with
the migrated layout. Those records retain their historical technical rationale,
but they no longer authorize a standalone repository structure, publishing the
Rust crate or Python wheel, or restoring retired standalone workflow names. The
current package and release posture is the unpublished sibling-product boundary
defined here.

## Alternatives considered

- **Return PureCARD to a standalone repository.** Rejected. It would restore a
  second toolchain, CI surface, dependency policy, and governance copy without
  changing the decoder's operational independence.
- **Make PureCARD an analyzer component.** Rejected. The decoder neither consumes
  analyzer internals nor serves the analyzer API. Treating it as part of the
  analysis-engine DAG would misstate both products and invite accidental
  coupling.
- **Extract a shared parser or corpus crate now.** Rejected. There is no current
  dependency edge or proven shared abstraction to extract. Premature sharing
  would couple independently evolving Pure dialect and oracle concerns.
- **Publish the renamed Cargo package or wheel.** Rejected. The monorepo move is
  for source and governance colocation; current wheels exist to verify the PyO3
  boundary, not to establish a supported package-release channel.

## Consequences

- One root workflow governs both products, while each product keeps its own
  domain model, specification, ADRs, tests, and runtime dependency graph.
- Analyzer refactors cannot become implicit PureCARD dependencies, and PureCARD
  cannot become an undeclared analyzer feature.
- The package name follows workspace conventions without breaking existing Rust
  `use purecard::...` or Python `import purecard` consumers.
- Cross-product reuse carries deliberate ADR overhead. That friction is
  intentional: it keeps an attractive local convenience from becoming an
  unreviewed permanent architecture.
- Revisit this decision only when measured duplication or a required supported
  integration justifies a stable shared contract.
