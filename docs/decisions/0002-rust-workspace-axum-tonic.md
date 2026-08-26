# 0002. Rust workspace with axum (HTTP) and tonic (gRPC) over tower/hyper

- **Status:** Superseded by ADR-0003
- **Date:** 2026-07-04
- **Deciders:** Project maintainer

> **2026-07-22:** This ADR was written for the domain-agnostic starter-kit
> template, before this repository was instantiated as `pure-analyzer` — a
> mechanical static-analysis CLI/LSP toolchain, not a network service. It
> never shipped any domain logic behind the axum/tonic stack it describes.
> Superseded in full by [ADR-0003](0003-analysis-engine-crate-dag.md); kept
> for the historical record.

## Context

The kit must seed a **high-performance server** that is domain-agnostic and safe
to hand to an AI agent for extension. Two forces shape the choice:

1. **Performance and safety.** We want predictable latency and memory behavior,
   no garbage-collection pauses, and compile-time guarantees strong enough that a
   whole class of agent mistakes simply won't compile.
2. **A layered structure the machine can enforce.** The "clean architecture"
   boundary (domain → app → infra → server) must be checkable, not aspirational,
   so the agent can't accidentally let a framework type leak into the domain.

We also want HTTP and gRPC from the same process without a bespoke networking
stack, and no coupling to any serverless runtime (which would constrain the
deployment story and pull in vendor-specific assumptions the kit shouldn't make).

## Decision

We will build on a **multi-crate Cargo workspace**, edition 2024, latest stable
toolchain, with crates `domain`, `app`, `infra`, `server`, plus `xtask` (typed CI
logic). Dependencies point inward only, enforced by crate visibility and
`cargo-deny` bans.

For networking we will use **axum** for HTTP and **tonic** for gRPC, both sharing
**tower** middleware over **hyper**. `tokio` is the async runtime. Errors use
`thiserror` in libraries and `anyhow` at boundaries; `tracing` is wired from the
first commit. `#![forbid(unsafe_code)]` and `#![deny(missing_docs)]` (public
crates) are mandatory. No serverless runtime coupling.

## Alternatives considered

- **A GC'd language (Go, a JVM/BEAM stack).** Faster to write, but gives up the
  compile-time guarantees that make agent-authored code safer, and the "make
  illegal states unrepresentable" lever that Rust's type system provides.
- **A single-crate layout.** Simpler at first, but the architectural boundary
  becomes convention only — exactly the thing an agent erodes. Separate crates let
  `cargo-deny` enforce the layering mechanically.
- **A batteries-included web framework (actix-web, Rocket).** Heavier and more
  opinionated; axum's tower/hyper foundation is lighter, composes HTTP and gRPC
  through one middleware model, and shares the ecosystem tonic already builds on.
- **gRPC via a separate stack.** Duplicates middleware and runtime concerns; tonic
  on hyper/tower lets HTTP and gRPC share one tower stack.
- **Coupling to a serverless runtime.** Rejected outright — it would bake a
  deployment assumption into a kit that is meant to be domain- and
  deployment-agnostic.

## Consequences

- The layering is enforceable in CI: `cargo-deny` bans, plus the `no-io-in-domain`
  ast-grep rule, keep dependencies pointing inward and keep `domain` free of I/O
  and frameworks.
- HTTP and gRPC share a tower middleware stack, so cross-cutting concerns
  (tracing, timeouts, auth) are written once.
- We inherit the tokio/tower/hyper ecosystem's maturity and its learning curve;
  async Rust is a real cost the agent's rules and lessons must help manage.
- Persistence is intentionally left open (see the design notes): when added it
  will use `sqlx` compile-checked queries with forward/backward migration tests
  and testcontainers integration, and will land as its own ADR.
- Toolchain is pinned (`rust-toolchain.toml`) and the lockfile committed, so
  agent, CI, and laptop resolve identically.
