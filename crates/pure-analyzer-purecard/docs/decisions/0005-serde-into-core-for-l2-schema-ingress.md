# 0005. serde + serde_json into the core for L2 schema ingress

- **Status:** Accepted
- **Date:** 2026-07-11
- **Deciders:** Thiago (human approval of the PROTECTED-gate widen), agent

## Context

M3 adds the L2 schema-consistency overlay. The decoder consults a per-database
`Schema` (`docs/spec/schema.md` §6.2), which the host builds once and hands over
at session init as JSON (§6.3, §9): `Schema::from_json` is the ingress. That
parser is **shipped, host-facing code** — a consumer of the published `purecard`
crate calls it — so it cannot live in the dev-only oracle harness (`tests/`,
ADR-0003) the way `serde`/`serde_json` did through M2.

ADR-0004 set the core's runtime-dependency allowlist to `{ thiserror }` and made
`cargo xtask check-core-deplight` enforce it — a PROTECTED gate (constitution §2,
§7). Parsing JSON in-crate needs `serde` + `serde_json`, which are not on that
list. So M3 must either widen the allowlist or avoid the dependency.

## Decision

We will move `serde` (with `derive`) and `serde_json` into the core
`[dependencies]` and widen the `check-core-deplight` allowlist to
`{ thiserror, serde, serde_json }`. The gate stays enforcing — any other runtime
dep still fails it — and the widening is recorded here as required for a
PROTECTED-gate change (constitution §7: only a human may loosen a PROTECTED gate,
with a machine-checkable justification).

## Alternatives considered

- **Hand-roll a JSON parser in-crate.** Rejected by "library before writing"
  (constitution §4): a bespoke parser would own JSON's escaping, number, and
  unicode edge cases for no benefit, whereas `serde`/`serde_json` are the
  ubiquitous ecosystem standard, already in the lockfile, and clean under
  `cargo deny` (license + advisory). Our crate keeps `#![forbid(unsafe_code)]`
  regardless of serde's internal implementation. A bespoke parser is *more*
  risk, not less.
- **Keep `from_json` in the test harness only.** Rejected: the host calls it in
  production, so it is not oracle code. Hiding shipped API in `tests/` would be a
  layering lie.
- **A non-JSON ingress (bincode, a builder API).** Rejected: §6.3/§9 fix JSON as
  the contract's wire form (it mirrors the MCP reflection tools' output); a second
  format would fork the contract.

## Consequences

- The published crate now pulls `serde` + `serde_json` into a downstream
  resolution graph. Both are near-universal and unsafe-free, so the supply-chain
  cost is minimal; `cargo deny` continues to vet their licenses/advisories.
- `check-core-deplight` remains the guard: the allowlist is a three-crate set,
  not "any dep", and the gate's unit tests pin that an unrelated dep (e.g.
  `tokio`) still fails — including behind a `package = "…"` alias or a trailing
  comment (anti-gaming, §7).
- The allowlist may still only be widened by a human with a recorded
  justification; it never silently disables the check. A future dep needs its own
  ADR.
