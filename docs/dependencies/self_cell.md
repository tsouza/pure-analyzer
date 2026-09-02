# Vetting: self_cell 1.3.0

- Purpose: co-locates the `Rc<CompiledGrammar>` owner and the borrowing
  `DecoderSession` in `purecard`'s PyO3 boundary (`src/ffi.rs`)
  without hand-written `unsafe` self-referential-struct code — the safe,
  audited crate for exactly this pattern. Optional, feature-gated (`python`),
  off in the default build.
- License: `Apache-2.0 OR GPL-2.0-only` — a dual license where the consumer
  chooses either arm; depending on it under the Apache-2.0 arm is fully
  compatible (`cargo deny check licenses` confirms this resolves clean —
  cargo-deny accepts an OR-expression dependency if any one arm is
  allowlisted, which Apache-2.0 already is; GPL-2.0-only alone would not be).
- Maintenance: last release 2026-07-16 (v1.3.0, bumped up from purecard's
  stale pre-migration pin of 1.2.2 as part of this migration's "latest
  stable, verified" pass), current per crates.io.
- Reputation: purpose-built, narrowly-scoped crate (co-locate an owner and a
  borrowing type) specifically to avoid hand-rolled `unsafe` self-referential
  structs — was already live in purecard's own published crate before this
  migration.
- Supply chain: `cargo audit` clean. Its own internal unsafe (the
  self-referential-struct trick) is encapsulated inside `self_cell`'s crate,
  verified not to leak into `purecard`'s own
  `#![forbid(unsafe_code)]` (the crate compiles clean under it — see PR
  verification).
- Fit / adaptation cost: exact match, zero adaptation — purpose-built for
  this exact pattern; a hand-rolled alternative would mean owning unsafe
  lifetime-extension code ourselves for no gain ("library before writing").
- Decision: **ADOPT** — already proven in purecard's own pre-migration usage;
  version bumped to current as part of porting it.
- Reviewer sign-off: not required (green via the OR-license arm, no
  `deny.toml` change).
