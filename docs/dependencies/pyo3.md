# Vetting: pyo3 0.29.0

- Purpose: the M4 PyO3 boundary (`crates/pure-analyzer-purecard/src/ffi.rs`) —
  Rust/Python bindings so purecard's decoder can be driven from Python
  research code, the crate's stated primary use case. Optional, feature-gated
  (`python`), off in the default build.
- License: `MIT OR Apache-2.0` — compatible with Apache-2.0: yes (deny.toml
  allowlisted: yes).
- Maintenance: last release 2026-06-11 (v0.29.0), the current stable per
  crates.io. Repo (`pyo3/pyo3`) is the de facto standard Rust/Python bindings
  crate, actively maintained.
- Reputation: ubiquitous in the Rust/Python interop ecosystem; was already
  live in purecard's own published crate (v0.1.0, 191 downloads) before this
  migration — a real, working track record, not a speculative addition.
- Supply chain: `cargo audit` clean for `pyo3` and its transitive tree.
  `pyo3`'s own codegen can emit `unsafe` — but that unsafe is generated and
  encapsulated *inside pyo3's own crate*, not inlined into
  `purecard`'s compilation unit the way `logos`'s codegen is
  (see `docs/dependencies/logos.md` for that contrast) — so it does not
  threaten this crate's own `#![forbid(unsafe_code)]`. `self_cell` (see its
  own vetting note) exists specifically to keep the one unsafe-adjacent
  lifetime trick (`Rc<CompiledGrammar>` co-located with the borrowing
  `DecoderSession`) encapsulated the same way.
- Fit / adaptation cost: exact match, zero adaptation — this is the crate
  purecard's own architecture is already built around for its Python surface.
- Decision: **ADOPT** — already proven in purecard's own pre-migration usage;
  ported as-is, version already current.
- Reviewer sign-off: not required (green license, no `deny.toml` change).
