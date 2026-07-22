# Vetting: proptest 1.11.0

- Purpose: property-based testing — drives `pure-analyzer-purecard`'s §8.5
  rollback/equivalence properties from committed seeds
  (`proptest-regressions/`). Dev-only dependency, newly added to this
  workspace's `[workspace.dependencies]` as part of the purecard migration
  (pure-analyzer had no property-testing dependency before this).
- License: `MIT OR Apache-2.0` — compatible with Apache-2.0: yes (deny.toml
  allowlisted: yes, both arms already present).
- Maintenance: current stable per crates.io (verified via `cargo add`, which
  resolves the live registry rather than a remembered version — the only way
  a version is allowed to enter this workspace's `Cargo.toml`, per
  constitution §2). The de facto standard property-testing crate for Rust,
  actively maintained.
- Reputation: extremely widely used across the Rust ecosystem for exactly
  this purpose; was already a dev-dependency in purecard's own published
  crate before this migration.
- Supply chain: `cargo audit` clean for `proptest` and its transitive tree
  (`rand`, `bit-set`/`bit-vec`, `rusty-fork` — a modest, well-known set for a
  property-testing crate that needs its own subprocess-based shrinking).
- Fit / adaptation cost: exact match, zero adaptation — this is the type the
  crate's already-written property tests are specified against; a bespoke
  property-testing harness would be reinventing generators/shrinking for no
  gain.
- Decision: **ADOPT** — already proven in purecard's own pre-migration usage;
  the workspace-level pin is now the single source of truth for its version
  across any future consumer, not re-declared per-crate.
- Reviewer sign-off: not required (green license, no `deny.toml` change).
