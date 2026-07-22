# Vetting: ureq 3.3.0

- Purpose: the `legend` lane's live-HTTP client — talks to a local Legend
  engine instance (`http://localhost:6300`) for purecard's opt-in
  engine-backed completeness tests (`tests/legend_completeness.rs`,
  `just test-legend`). Dev-only dependency, `default-features = false`
  (TLS disabled — the engine is plain local HTTP, so no
  rustls/ring/webpki-roots tree is pulled in; `json` feature enables
  `send_json`/`Body::read_json`).
- License: `MIT OR Apache-2.0` — compatible with Apache-2.0: yes (deny.toml
  allowlisted: yes).
- Maintenance: last release 2026-03-21 (v3.3.0), current per crates.io.
  Actively maintained, minimal-dependency HTTP client crate.
- Reputation: a well-known, minimal alternative to `reqwest` for cases that
  don't need its full async/TLS surface — exactly this dev-only,
  local-plain-HTTP use case; was already live in purecard's own published
  crate (as a dev-dependency, so it never shipped in the published artifact
  anyway) before this migration.
- Supply chain: `cargo audit` clean for `ureq` and its transitive tree with
  TLS disabled (the smaller dependency surface `default-features = false`
  buys is also a supply-chain win, not just a size one).
- Fit / adaptation cost: exact match, zero adaptation — a plain local-HTTP
  dev-only client for exactly one opt-in test lane; hand-rolling this would
  mean owning HTTP client correctness for a test-only, low-stakes path.
- Decision: **ADOPT** — already proven in purecard's own pre-migration usage,
  version already current.
- Reviewer sign-off: not required (green license, dev-only, no `deny.toml`
  change).
