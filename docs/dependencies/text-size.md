# Vetting: text-size 1.1.1

- Purpose: byte-offset `TextSize`/`TextRange` newtypes — the span representation
  `pure-analyzer-lexer` returns from `lex()` and analyzer diagnostics use, so a
  token span stays directly comparable across analyzer boundaries.
- License: `MIT OR Apache-2.0` — compatible with Apache-2.0: yes (deny.toml
  allowlisted: yes, both arms already present).
- Maintenance: last crates.io release 2023-06-30 (v1.1.1). The upstream GitHub
  repo (`rust-analyzer/text-size`) is archived, but its README states why:
  "This crate now lives in
  <https://github.com/rust-lang/rust-analyzer/tree/master/lib/text-size>" — the
  code was folded into the rust-analyzer monorepo, not abandoned. A
  newtype-wrapper crate over a `u32` offset has a complete API surface; no
  further crates.io releases are needed for it to keep working.
- Reputation: `text-size` is a direct dependency of
  `pure-analyzer-diagnostics` and `pure-analyzer-lexer`. Sharing it preserves
  one compatible span type across the analyzer.
- Supply chain: zero further dependencies (a leaf crate — newtype wrappers over
  `u32`); no `build.rs`; `cargo-audit` clean (no RustSec advisories against
  `text-size`). The cargo-vet bootstrap records its current version as an exact
  exemption, so an update must add audit coverage or an explicitly reviewed new
  exemption.
- Fit / adaptation cost: exact match, zero adaptation for the workspace span
  representation.
- Decision: **ADOPT** — already a mandatory dependency via
  `pure-analyzer-diagnostics`; using it directly in `pure-analyzer-lexer` too
  avoids a duplicate span type at zero incremental supply-chain cost.
- Reviewer sign-off: not required (green license, no `deny.toml` change, no
  protected-gate change).
