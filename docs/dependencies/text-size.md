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
- Reputation: **correction (caught in review) — not pulled in via `rowan`.**
  `rowan` is declared in `[workspace.dependencies]` but nothing depends on it
  yet, so it doesn't appear in `Cargo.lock` at all; the claim that `text-size`
  arrives transitively through it was wrong. It's actually already a **direct**
  dependency of `pure-analyzer-diagnostics` (bootstrapped before this PR,
  confirmed in `crates/pure-analyzer-diagnostics/Cargo.toml` and in
  `Cargo.lock`) — so it was already unconditionally in this workspace's
  dependency tree, just via a different crate than originally stated.
  Depending on it directly in `pure-analyzer-lexer` still keeps the span type
  identical across lexer/diagnostics/(future syntax/parser) rather than
  introducing a second, incompatible span newtype at the bottom of the DAG —
  that reasoning holds, it was only the *reverse-dependency name* that was
  wrong. Once `pure-analyzer-syntax` (via `rowan`) lands for real, `rowan`
  will pull in `text-size ^1.1.0` too (confirmed via crates.io), so all three
  reverse-deps converge on the same version either way.
- Supply chain: zero further dependencies (a leaf crate — newtype wrappers over
  `u32`); no `build.rs`; `cargo-audit` clean (no RustSec advisories against
  `text-size`). The cargo-vet bootstrap records its current version as an exact
  exemption, so an update must add audit coverage or an explicitly reviewed new
  exemption.
- Fit / adaptation cost: exact match, zero adaptation — this *is* the type the
  design doc's shared span representation is specified against.
- Decision: **ADOPT** — already a mandatory dependency via
  `pure-analyzer-diagnostics`; using it directly in `pure-analyzer-lexer` too
  avoids a duplicate span type at zero incremental supply-chain cost.
- Reviewer sign-off: not required (green license, no `deny.toml` change, no
  protected-gate change).
