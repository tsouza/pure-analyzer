# 0003. Scope the crate DAG to the pure-analyzer product

- **Status:** Accepted
- **Date:** 2026-07-22; ratified and corrected 2026-08-26
- **Deciders:** Project maintainer and agent, through the normal issue/PR review flow

## Context

The repository began from a domain-agnostic Rust server starter kit with a
hexagonal `domain → app → infra → server` layout. That layout assumed an
HTTP/gRPC service with business logic behind a transport.

`pure-analyzer` has a different architecture: a mechanical Legend Pure
analysis pipeline with lexer, lossless syntax, parser, model loader, resolver,
passes, facade, and front ends. There is no persistence or network-service layer
in that product. A per-crate dependency allow-set expresses its real topology
more precisely than the starter kit's four linear ranks.

The original proposed version of this ADR incorrectly described
`pure-analyzer-model` and `pure-analyzer-resolve` as siblings. The implemented
allow-set permits resolver to consume the model crate, while forbidding the
reverse edge. This ratification corrects that ordering and distinguishes the
processing pipeline from Cargo's dependency-arrow direction.

This ADR governs **only the `pure-analyzer` product**. The later-migrated
`pure-analyzer-purecard` crate is an independent sibling product governed by
[ADR-0004](0004-purecard-independent-workspace-product.md) and PureCARD's nested
[ADR-0009](../../crates/pure-analyzer-purecard/docs/decisions/0009-monorepo-placement.md).

## Decision

The analyzer processing pipeline is:

```text
lexer → syntax → parser → model → resolve → analysis → libpure → front ends (CLI, LSP)
```

That arrow means data-processing order. Cargo dependency edges point toward
prerequisites. The essential direction is therefore:

```text
pure-analyzer-resolve → pure-analyzer-model
```

The reverse dependency is forbidden. More generally, the enforced analyzer
allow-set permits each later layer to depend only on its declared prerequisites:

- `pure-analyzer-lexer` has no internal prerequisite.
- `pure-analyzer-syntax` may depend on lexer.
- `pure-analyzer-parser` may depend on lexer, syntax, and diagnostics.
- `pure-analyzer-model` may depend on lexer, syntax, parser, and diagnostics.
- `pure-analyzer-resolve` may additionally depend on model.
- `pure-analyzer-analysis` may additionally depend on resolve.
- `libpure` may facade the analyzer layers below it.
- `pure-analyzer-cli` and `pure-analyzer-lsp` are front ends and may depend on `libpure` and diagnostics.

`pure-analyzer-diagnostics` is a shared leaf inside the analyzer product. It has
no analyzer dependencies and contains no renderer. Only analyzer front ends may
depend on renderer or protocol crates such as `clap`, `ariadne`, or a future LSP
stack.

`cargo xtask verify-layering` checks the explicit per-crate allow-set against
`cargo metadata`, including normal, development, build, optional, and renamed
dependencies. The `no-front-end-deps-in-core` ast-grep rule independently keeps
front-end libraries out of analyzer core crates.

Adding or moving an analyzer crate requires an explicit allow-set and governance
update in the same change.

PureCARD is not a front end, facade, parser layer, or any other node in this DAG.
`xtask` is shared repository infrastructure and likewise is not an analyzer
product layer.

## Alternatives considered

- **Keep `domain`/`app`/`infra`/`server` aliases.** Rejected because those names
  mischaracterize a compiler-style pipeline and preserve service constraints the
  analyzer does not need.
- **Treat model and resolver as siblings.** Rejected because resolution consumes
  the model graph. The permitted direction is resolve-to-model; forbidding it
  would contradict the implemented analyzer allow-set, while allowing the
  reverse would create the wrong ownership direction.
- **Place PureCARD in the analyzer DAG.** Rejected because it is a constrained
  decoder with its own runtime, tests, corpus, Python boundary, and release
  posture. Similar subject matter and workspace co-location do not make it an
  analyzer layer.
- **Rely on review instead of dependency enforcement.** Rejected because the
  constitution requires architecture invariants to be machine-checked across
  every Cargo dependency kind.

## Consequences

- Analyzer documentation must use the exact model-before-resolve processing
  order and state Cargo direction explicitly when ambiguity is possible.
- Adding or moving an analyzer crate requires updating the allow-set and the
  corresponding architecture records in the same change.
- Analyzer front ends stay thin, and diagnostics remain renderer-independent.
- PureCARD changes cannot be justified as analyzer-DAG work. Its zero-edge
  product boundary is enforced separately within the same verification command.
- Product capability claims are backed by code, tests, and product references.
