# 0003. Non-core lives in `tests/`; the published `purecard` core is dep-light

- **Status:** Accepted
- **Date:** 2026-07-11
- **Deciders:** Thiago Souza; agent (Claude)

## Context

At M0 the only code that genuinely ships is the constrained-decoder core: the
`GuaranteeLevel` lattice (`lib.rs`) and the `Vocab` table (`vocab.rs`), both pure
`std`. Everything else in the M0 changeset is *oracle harness* — the gold-corpus
loader, the throwaway byte recognizer + replay driver, and the Legend
completeness probe (a pure `classify_return_type` plus a feature-gated live-HTTP
client). Those exist to prove the two feedback loops are wired (see
`specs/m0-oracle-harness.md`); none of them is decoder API.

While the harness sat under `src/`, it dragged `serde`, `serde_json`,
`thiserror`, `ureq`, and `anyhow` into the crate's `[dependencies]`. A downstream
consumer of `purecard` would then resolve that whole tree transitively, even
though it is pure test scaffolding — the opposite of the "src/ = core only"
invariant the milestone wants. ADR-0002 already fixed the repository as a
**single** published crate; the open question was only *where within that crate*
the core/harness boundary lives.

## Decision

We will keep `purecard` a single crate (ADR-0002 unchanged) and draw the
core/harness line as `src/` vs `tests/`, not as a crate split. The published
crate ships `src/**` only — `GuaranteeLevel` and `Vocab` — with an **empty
`[dependencies]` table**, so it has zero runtime dependencies. All harness code
moves to `tests/support/*.rs`, pulled into the integration-test binaries via
`#[path]` modules, and every harness dependency becomes a `[dev-dependency]`,
absent from any consumer's resolution graph. The live-HTTP Legend lane stays an
opt-in bare `legend` feature, absent from the hermetic `just ci`.

The invariant is enforced by `cargo xtask check-core-deplight` (wired into
`xtask ci` and exposed as `just check-core-deplight`), which asserts the
`[dependencies]` table is empty and that `cargo package --list` names no file
under `tests/` or `corpus/`.

## Alternatives considered

- **Split the harness into a second crate (`crates/oracle`).** Rejected: moving
  the deps to `[dev-dependencies]` already removes them from a consumer's graph,
  so a second crate buys no packaging benefit while adding a manifest, a path
  dependency, split coverage/mutation scoping, and — fatally — a revision to
  ADR-0002's single-crate decision, for a boundary M0 has no use for. DOMAIN §3.2
  keeps the decoder a single crate; introducing `crates/oracle` now is false
  structure, deferred until an engine lane grows real weight.
- **Leave `DecodeError` / `ByteRecognizer` in `src/` as public API.** Rejected:
  with the byte recognizer gone from the core, `DecodeError` has no in-`src/`
  producer at M0, so shipping it would be premature public surface. Both return
  to `src/` at M1 in token-level form alongside `session.rs`.
- **Enforce the boundary by convention / review only.** Rejected: constitution
  §5 requires a fix be closed as a *class* by a gate. A silent re-addition to
  `[dependencies]` or an edit to the `include` list would otherwise slip through;
  `check-core-deplight` makes both fail a PR.

## Consequences

- **Easier:** `purecard` publishes with no runtime deps; a consumer pays nothing
  for the harness. `cargo tree -e normal -p purecard` shows the crate alone.
- **Harder / follow-on obligations:** harness modules are shared across test
  binaries via `#[path]` includes (their `#[cfg(test)]` unit tests run inside the
  including binary), so a new harness consumer wires the module explicitly rather
  than `use purecard::…`. `check-core-deplight` must stay green; a genuinely
  core dependency added at M1+ requires updating the gate deliberately.
- **Revisit if:** the decoder core grows a real runtime dependency (the gate is
  updated with justification, never silently loosened), or the engine lane grows
  enough weight to warrant its own crate (revisit the single-crate boundary, not
  ADR-0002's single-*published*-crate stance).
