# 0002. PureCARD is a single constrained-decoder library, not a layered server

- **Status:** Accepted
- **Date:** 2026-07-10
- **Deciders:** Thiago Souza; agent (Claude)

## Context

This repository began as a domain-agnostic Rust *server* starter kit: four layered
crates (`domain → app → infra → server`), a gRPC/HTTP stack (axum/tonic/prost), and
a stub "greeting" domain — plus a substantial, genuinely reusable engineering
methodology (deterministic gates, spec-driven loop, a reviewer agent, an oracle
mindset).

The actual project, specified in full in [the spec](../spec/README.md), is
**PureCARD**: a grammar- and schema-constrained decoder for Legend Pure that masks
a language model's logits so generated queries are valid by construction. It is a
single library crate exposing a thin PyO3 boundary — the Rust half of a Rust/Python
split. It has no server, no transport, no request/response layering. The scaffold
"tells more than the project needs," and carrying the server machinery onto a
library would be dead weight and false structure.

This supersedes ADR-0002-rust-workspace-axum-tonic (deleted), which recorded the
now-void server architecture.

## Decision

We will collapse the workspace to a single published library crate, `purecard`, at
the repository root (plus the `xtask` dev orchestrator), and **delete** everything
that describes the server domain: the four stub crates, `proto/` + `buf`, the
gRPC/HTTP stack, and the `domain → app → infra → server` layering gates. We will
**keep** the domain-agnostic methodology and quality gates, **retarget** the
PROTECTED gates (coverage, mutation, no-skip, ast-grep hygiene) onto `src/**` /
`purecard`, and **defer** premature machinery (PyO3 + maturin → M4; semver-checks
and public-api → until a published baseline exists; fuzz/bench → their milestones).

## Alternatives considered

- **Keep the four-layer structure, put the decoder in `domain`.** Rejected: the
  layering models a server's I/O boundary, which a decoder library does not have.
  It would be structure for its own sake, and the layering gates would guard an
  invariant that no longer exists.
- **Port the full gate suite as-is.** Rejected: an empty library cannot feed a
  mutation/fuzz/public-api/bench lane meaningfully; running them on nothing is
  noise, and a baseline-dependent gate (semver-checks) cannot even pass before the
  first publish. Deferring is honest; deleting a PROTECTED gate would violate
  constitution §3/§7, so those are retargeted, not removed.
- **Start a fresh repository.** Rejected: the methodology, gates, and the committed
  test corpus are the scaffold's real value and transfer directly. A rewrite would
  discard them to avoid a bounded curation pass.

## Consequences

- **Easier:** the tree now matches the project; `just ci` gates exactly the code
  that ships; the crates.io publish lane is wired (`purecard`, `publish = true`,
  with `CARGO_REGISTRY_TOKEN`).
- **Harder / follow-on obligations:** the decoder itself (M0–M5 in
  [`../spec/overview.md`](../spec/overview.md) §10) is still entirely ahead. The
  deferred lanes must be re-enabled at their milestones:
  set `SEMVER_ENABLED` after the first crates.io release, `PUBLIC_API_ENABLED`
  after committing a baseline, add the PyO3 `python` feature + a maturin/PyPI lane
  at M4, and restore fuzz/bench when there is code to exercise.
- **Revisit if:** PureCARD grows a second published crate (re-introduce a real
  multi-member workspace) or a genuine service front-end (a separate binary crate,
  not a return to layering inside the library).
