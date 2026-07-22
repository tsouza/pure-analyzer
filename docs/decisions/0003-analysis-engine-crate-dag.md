# 0003. Analysis-engine crate DAG, replacing the domain/app/infra/server layering

- **Status:** Proposed — pending maintainer review; not yet ratified
- **Date:** 2026-07-22
- **Deciders:** Agent (project instantiation), pending sign-off from the project
  maintainer. Per [ADR-0001](0001-record-architecture-decisions.md), an ADR is
  immutable and Accepted only once it has actually gone through the normal PR
  - review flow — this one has not yet.

## Context

This repository was created from a domain-agnostic Rust server starter kit
(ADR-0001, ADR-0002): a hexagonal `domain → app → infra → server` layering
over an axum (HTTP) + tonic (gRPC) service, with `.proto` contracts governed
by `buf`. That shape assumes the product being built is a network service with
business logic behind a transport.

`pure-analyzer` (see `docs/design/pure-analyzer-design.md`, the full
implementation spec) is not that. It is a mechanical, standalone static-analysis
toolchain for Legend Pure: a lexer, a resilient parser producing a lossless
CST, a model loader, a source-threaded resolver, and validate/lint passes,
exposed through a CLI today and an LSP in v0.2. There is no network service,
no persistence, and no business-domain/use-case split — the "business logic"
*is* the parsing and analysis pipeline itself, and it has its own natural
layering: a directed acyclic graph from raw tokens up to a rendered
diagnostic, not four generic hexagonal rings.

Keeping the old `domain`/`app`/`infra`/`server` names and their linear
inward-only rank would force every future crate to be mischaracterized as one
of those four roles, and — more importantly — the template's layering
*enforcement* (`xtask verify-layering`'s rank comparison, the `no-io-in-domain`
ast-grep rule, `deny.toml`'s tokio/axum/tonic ban) is specific to a linear
4-layer service and cannot express a DAG with siblings (e.g.
`pure-analyzer-model` and `pure-analyzer-resolve` both build on
`pure-analyzer-parser` but neither depends on the other).

## Decision

We replace the `domain → app → infra → server` layering with the crate DAG
from the design doc §3:

```text
pure-analyzer-lexer -> pure-analyzer-syntax -> pure-analyzer-parser
  -> { pure-analyzer-model, pure-analyzer-resolve }
  -> pure-analyzer-analysis -> libpure -> pure-analyzer-cli
pure-analyzer-diagnostics: a leaf, depended on by pure-analyzer-parser and
  everything above it (not by lexer/syntax).
```

`libpure` is a thin facade (`pub use` of syntax/parser/model/resolve/analysis)
so `pure-analyzer-cli` — and, in v0.2, `pure-analyzer-lsp` — are the only
crates that need to know about the whole engine; everything below `libpure` is
an implementation detail of it. Only front-end crates may depend on a
renderer (`ariadne`, `codespan-reporting`) or protocol crate (`clap`, later
`tower-lsp`/`lsp-types`) — the direct analogue of the old "no I/O in domain"
rule, now scoped to "no front-end deps in the core."

Enforcement is rebuilt to match a DAG rather than a linear rank:
`xtask::tasks::ALLOWED_INTERNAL_DEPS` is an explicit per-crate allow-set (not
a rank comparison), checked by `cargo xtask verify-layering` against `cargo
metadata`'s real dependency graph (any kind, including dev/build deps — the
class of edge a crate-global `cargo-deny` ban misses). The per-crate half
lives in `ast-grep-rules/no-front-end-deps-in-core.yml`. Axum, tonic, tower,
hyper, prost, and the `buf`/`.proto` toolchain are removed outright — nothing
in this project's scope needs a network service or a wire protocol.

We keep the *rest* of the starter kit's methodology unchanged: the constitution's
PROTECTED/EVOLVABLE split, the spec-driven workflow, the quality-layer gates
(coverage floor, mutation testing, fuzzing, public-API snapshots), and
`just`/`xtask` as the sole automation entry point. This ADR only revises §1's
concrete layer names and enforcement mechanism; the *principle* — dependencies
point one way, and it's a CI-enforced fact rather than a convention — is
unchanged and, if anything, stricter: a DAG with an explicit allow-set per
crate is a tighter check than a 4-way linear rank.

## Alternatives considered

- **Keep `domain`/`app`/`infra`/`server` as aliases and force-fit the new
  crates into them** (e.g. `domain` = lexer+syntax+parser, `app` = model,
  `infra` = resolve+analysis). Rejected: the mapping is arbitrary and would
  mislead every future contributor reading crate names against the design
  doc's own §3 crate list, which is the actual source of truth for this
  project's architecture.
- **Drop layering enforcement entirely** and rely on code review. Rejected outright
  by constitution §2/§7 (anti-gaming, deterministic gates over hope) — the whole
  point of the starter kit is that architecture invariants are machine-checked.
- **Model the DAG as a linear rank anyway**, picking an arbitrary total order
  across the siblings (e.g. alphabetical: model before resolve). Rejected: a
  fake total order would forbid nothing between true siblings but silently
  *permit* a rank-adjacent edge that isn't actually in the design (e.g. a
  false "resolve may depend on model" when actually intended, but also
  wrongly implying resolve and model could depend on each other in either
  direction if their ranks were swapped later) — an explicit allow-set names
  exactly the edges that are legal, no more.

## Consequences

- `cargo xtask verify-layering`, `ast-grep-rules/no-front-end-deps-in-core.yml`,
  `deny.toml`'s layering comment, `.coderabbit.yaml`'s review guidance, and the
  PR/issue templates' "layer touched" checklists all now speak in terms of the
  new crate DAG. Keep them in sync when the DAG changes (adding
  `pure-analyzer-ir`/`pure-analyzer-eq` in v0.2, `pure-analyzer-lsp` in v0.2,
  `pure-analyzer-eq-smt` in v2+ — see design doc §9).
- `release-plz.toml`'s `[[package]]` overrides and `xtask`'s
  `PUBLIC_API_CRATES` list now name the real crates; a new crate must be added
  to both (and to `ALLOWED_INTERNAL_DEPS`) or it silently ships outside the
  release-notes/public-API-snapshot/layering gates.
- Proto/buf-specific CI jobs, mise tools (`buf`, `protoc`), and the `just
  proto`/`proto-breaking` targets are gone. If a future front-end needs a wire
  protocol (unlikely for an LSP, which uses JSON-RPC over stdio via
  `tower-lsp`, not protobuf), that infrastructure is re-added deliberately,
  not left dormant "just in case."
- We revisit this ADR if `pure-analyzer` ever needs a genuine network service
  (e.g. a hosted analysis API) — at that point the *service* gets its own
  layering decision; it does not retroactively reshape the analysis-engine DAG
  this ADR defines.
