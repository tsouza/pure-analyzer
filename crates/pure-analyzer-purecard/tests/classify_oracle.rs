//! Always-on classifier gate for the Legend completeness probe.
//!
//! The response→outcome classifier ([`classify_return_type`]) is the pure,
//! offline-testable substance of the §8.2 completeness loop, and it lives in the
//! oracle harness under `tests/support/` (ADR-0003) rather than the published
//! crate. Pulling `support/legend.rs` in here as a crate-local module runs its
//! `#[cfg(test)] mod tests` classifier unit tests under default features (no
//! network, no docker), so the return-type/compile-error split is covered and
//! mutation-tested without shipping `serde_json` in `purecard`.
//!
//! Gated to `not(feature = "legend")`: with the `legend` feature on, the
//! `LegendClient` shim in `support/legend.rs` compiles but has no consumer here
//! (it would be dead code), so `legend_completeness.rs` — which does use the
//! client — carries the same `mod tests` in that configuration. Either way the
//! classifier tests run in exactly one binary per feature set, and the hermetic
//! `just ci` (default features) always exercises them here.
//!
//! This cfg sits below the doc comment, not above it: an inner attribute as
//! the very first line would cfg out everything textually after it in the
//! same file, including a `//!` written later — stripping the crate-level
//! doc entirely once `legend` is off (the default), tripping this
//! workspace's `#![deny(missing_docs)]` (inherited via `[lints] workspace =
//! true`, broader than purecard's own pre-migration lint scope).
#![cfg(not(feature = "legend"))]

#[path = "support/legend.rs"]
mod legend;
