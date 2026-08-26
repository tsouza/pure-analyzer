# Vetting: logos 0.16.1

- Purpose: the DFA-based tokenizer generator used by
  `pure-analyzer-lexer`.
- License: `MIT OR Apache-2.0` — compatible with Apache-2.0: yes (deny.toml
  allowlisted: yes, both arms already present).
- Maintenance: last crates.io release 2026-01-30 (v0.16.1). Upstream repo
  (`maciejhirsz/logos`) not archived, pushed 2026-07-08 (two weeks before this
  vetting), 87 open issues (an actively
  triaged, non-trivial project, not a red flag on its own for a crate this
  widely used), 3,536 GitHub stars.
- Reputation: the de facto standard lexer-generator in the Rust ecosystem for
  this exact shape of work (`rust-analyzer` and many other language tools use
  it or its lineage); pinned in this workspace's root `Cargo.toml` and adopted
  by `pure-analyzer-lexer`.
- Supply chain: pulls in `logos-derive` (proc-macro) and `logos-codegen`
  (the actual DFA-generation logic) plus `fnv` — a small, well-scoped
  transitive tree for a codegen crate. `cargo audit` clean (0 RustSec
  advisories against `logos`/`logos-derive`/`logos-codegen`/`fnv` in this
  workspace's dependency tree). The cargo-vet bootstrap records the current
  `logos` version as an exact-version exemption, so future version changes fail
  closed until an audit or updated exemption is reviewed.
- **Unsafe-code surface — the one axis needing real scrutiny.** `logos`'s
  default codegen path can emit `unsafe` blocks in the state-machine it
  generates (a performance optimization). Since that code is generated
  *inline inside the consuming crate* via the derive macro, not in a
  separate crate, it would count against `pure-analyzer-lexer`'s own
  `#![forbid(unsafe_code)]` — meaning the crate would fail to compile at all
  under the constitution's mandatory, no-exceptions attribute unless this is
  handled. `logos-codegen` ships a `forbid_unsafe` feature ("Don't use or
  generate unsafe code", confirmed from its `Cargo.toml` source) specifically
  for this case. This dependency is adopted **only** with that feature
  enabled (`crates/pure-analyzer-lexer/Cargo.toml`:
  `logos = { workspace = true, features = ["forbid_unsafe"] }`), verified by
  the crate actually compiling under `#![forbid(unsafe_code)]` (it does — see
  `just ci` in this PR).
- Fit / adaptation cost: exact match, zero adaptation for this lexer layer.
- Decision: **ADOPT**, conditional on `forbid_unsafe` always being enabled —
  that condition is encoded in the dependency declaration itself, not just
  this note, so it can't silently regress on a future edit that drops the
  feature flag without also dropping `#![forbid(unsafe_code)]` (which would
  itself fail to compile, so the two can't drift apart silently).
- Reviewer sign-off: not required (green license, no `deny.toml` change, no
  protected-gate change) — but the `forbid_unsafe` reasoning above is exactly
  the kind of thing a reviewer should re-verify, given it's the one axis with
  real teeth.
