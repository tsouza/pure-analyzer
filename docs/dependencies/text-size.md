# Vetting: text-size 1.1.1

- Purpose: byte-offset `TextSize`/`TextRange` newtypes — the span representation
  `pure-analyzer-lexer` returns from `lex()`, and (per `docs/domain-model.md`)
  the same representation `pure-analyzer-syntax`/`-parser` and `Diagnostic`
  spans use throughout the DAG, so a token's span is directly comparable to a
  CST node's range with no conversion at any layer boundary.
- License: `MIT OR Apache-2.0` — compatible with Apache-2.0: yes (deny.toml
  allowlisted: yes, both arms already present).
- Maintenance: last crates.io release 2023-06-30 (v1.1.1). The upstream GitHub
  repo (`rust-analyzer/text-size`) is archived, but its README states why:
  "This crate now lives in
  <https://github.com/rust-lang/rust-analyzer/tree/master/lib/text-size>" — the
  code was folded into the rust-analyzer monorepo, not abandoned. A
  newtype-wrapper crate over a `u32` offset has a complete API surface; no
  further crates.io releases are needed for it to keep working.
- Reputation: used transitively by `rowan` 0.16.1 (already a workspace
  dependency for the future `pure-analyzer-syntax` crate) via `rowan`'s own
  `text-size = "^1.1.0"` requirement, confirmed via crates.io — so it is
  already unconditionally in this workspace's dependency tree regardless of
  this decision. Depending on it directly in `pure-analyzer-lexer` keeps the
  span type identical across lexer/syntax/parser rather than introducing a
  second, incompatible span newtype at the bottom of the DAG.
- Supply chain: zero further dependencies (a leaf crate — newtype wrappers over
  `u32`); no `build.rs`; `cargo-audit` clean (no RustSec advisories against
  `text-size`); not yet in the local `cargo-vet` store (first use in this repo).
- Fit / adaptation cost: exact match, zero adaptation — this *is* the type the
  design doc's shared span representation is specified against.
- Decision: **ADOPT** — already a mandatory transitive dependency via `rowan`;
  using it directly avoids a duplicate span type at zero incremental
  supply-chain cost.
- Reviewer sign-off: not required (green license, no `deny.toml` change, no
  protected-gate change).
