# Vetting: tokenizers 0.23.1

- Purpose: HuggingFace's real BPE tokenizer implementation — loads and runs
  the actual Qwen2.5-Coder / GPT-2 / cl100k_base byte-level tokenizers for
  purecard's `qwen-oracle` and `fused-extract` soundness lanes (see
  `docs/dependencies/`-adjacent feature docs in `Cargo.toml`). Optional,
  feature-gated, off in the default build, never in the per-PR `just ci` gate
  (heavy and network-fed — nightly/on-demand only, per its own feature docs).
- License: `Apache-2.0` — compatible with Apache-2.0: yes (deny.toml
  allowlisted: yes).
- Maintenance: last release 2026-04-27 (v0.23.1), current per crates.io.
  Maintained by HuggingFace, a well-resourced, actively-developed
  organization; the crate is the reference tokenizer implementation for the
  Python `transformers`/`tokenizers` ecosystem.
- Reputation: extremely widely used (the standard Rust tokenizer crate behind
  HuggingFace's Python `tokenizers` package); was already live in purecard's
  own published crate before this migration.
- Supply chain: `cargo audit` finds one advisory in its transitive tree —
  RUSTSEC-2024-0436 (unmaintained `paste`, via `macro_rules_attribute`) — an
  informational "unmaintained" notice, not a known vulnerability, with no
  upstream fix available. Accepted via `deny.toml`'s `[advisories.ignore]`,
  scoped to the fact that `tokenizers` (and therefore `paste`) is optional and
  never in a default build — ported verbatim from purecard's own
  pre-migration exception. No other advisories.
- Fit / adaptation cost: exact match, zero adaptation — this *is* the real
  tokenizer these oracle lanes exist to test against; a bespoke
  reimplementation would defeat the entire purpose of "real-tokenizer
  soundness" testing.
- Decision: **ADOPT** — already proven in purecard's own pre-migration usage,
  version already current, its one advisory already scoped and accepted.
- Reviewer sign-off: not required (green license, the one `deny.toml`
  advisory exception is documented and scoped, not a blanket loosening).
