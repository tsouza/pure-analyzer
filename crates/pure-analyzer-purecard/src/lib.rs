#![forbid(unsafe_code)]
#![deny(missing_docs)]

//! # PureCARD
//!
//! A grammar- and schema-constrained decoder for **Legend Pure**. PureCARD sits
//! between a language model's logits and its sampler and masks every next token
//! that cannot lead to a valid Pure query — so output is valid *by construction*
//! in a single forward pass, with no compile-repair round-trip.
//!
//! It offers two nested guarantees and deliberately refuses a third; see
//! [`GuaranteeLevel`]. The complete design — grammar, masking algorithm, schema
//! overlay, and the oracle-driven test strategy — is specified under
//! `docs/spec/`.
//!
//! ## Usage
//!
//! Drive the decoder over a host-supplied [`Vocab`] and [`CompiledGrammar`],
//! optionally narrowing the per-step mask to a [`Schema`]. This example is a
//! doctest: it is compiled and run against the real public surface, so a renamed
//! type, a removed constructor, or a changed receiver breaks the build.
//!
//! ```
//! use purecard::{
//!     CompiledGrammar, DecoderSession, Schema, SelfCheckError, Vocab, self_check_smoke,
//! };
//!
//! // `self_check_smoke` round-trips the embedded gold-shaped queries through a
//! // host `Vocab`, proving it can *express* them. A toy vocab that cannot even
//! // segment the first query's opening byte fails loud with a locatable drift
//! // error — asserted, not ignored, so a change to the self-check contract (or a
//! // silently-passing smoke check) breaks this example.
//! const TOY_EOS: u32 = 1; // reserved EOS id, one past the toy vocab's lone token
//! let toy = Vocab::from_byte_tokens(vec![b"filter".to_vec()], TOY_EOS);
//! let toy_grammar = CompiledGrammar::from_spec("", toy);
//! assert_eq!(
//!     self_check_smoke(&toy_grammar),
//!     Err(SelfCheckError::Unsegmentable { query_index: 0, pos: 0 }),
//! );
//!
//! // A host vocabulary of whole tokens (token id → raw bytes) that expresses the
//! // query `|X.all()->take(1)`; `from_spec` compiles the emitted-Pure grammar.
//! // The ids are named so reordering the vocabulary can't silently point a later
//! // `accept_token` at the wrong token.
//! const SOURCE: u32 = 0; // a complete source expression, `|X.all()`
//! const OPEN: u32 = 1; //   a step opening a call, `->take(`
//! const INT: u32 = 2; //    an integer literal, `1`
//! const CLOSE: u32 = 3; //  the closer, `)`
//! const EOS: u32 = 4; //    the reserved EOS id, one past the last token
//! let vocab = Vocab::from_byte_tokens(
//!     vec![
//!         b"|X.all()".to_vec(),
//!         b"->take(".to_vec(),
//!         b"1".to_vec(),
//!         b")".to_vec(),
//!     ],
//!     EOS,
//! );
//! let grammar = CompiledGrammar::from_spec("", vocab);
//!
//! // L1 (syntactic) session: the source token is admissible from the start; once
//! // accepted it is itself a complete query, and opening a call re-opens the stream.
//! let mut plain = DecoderSession::new(&grammar);
//! assert!(plain.allowed_mask().test(SOURCE), "the source token is admissible at Start");
//! plain.accept_token(SOURCE)?;                   // `Result<(), DecodeError>`
//! assert!(plain.is_complete(), "`|X.all()` is itself a complete query");
//! plain.accept_token(OPEN)?;                     // open `->take(`
//! assert!(!plain.is_complete(), "an open call is not complete");
//! plain.accept_token(INT)?;                      // `1`
//! plain.accept_token(CLOSE)?;                    // `)`
//! assert!(plain.is_complete(), "the closed `|X.all()->take(1)` is complete");
//! plain.reset();
//! assert!(!plain.is_complete(), "reset returns to a fresh, incomplete stream");
//!
//! // L2 (schema-consistent) session: the mask is additionally intersected with the
//! // schema-legal terminals at each identifier/operand position. This example shows
//! // the L2 *API* and the structural **L2 ⊆ L1** invariant — L2 only ever narrows,
//! // so a token L1 admits (here the source) is never *added* and, being
//! // schema-legal, still survives. That narrowing genuinely *removes* phantom
//! // classes/properties and type-mismatched operands is proven by the counterfactual
//! // suite (`tests/l2_precision.rs`) and, against fragmented BPE tokens, by
//! // `tests/bpe_split_soundness.rs` — not re-litigated in this doc example.
//! let schema = Schema::from_json(r#"{"db_id": "d", "db_path": "model::Db", "classes": {}}"#)?;
//! let mut sess = DecoderSession::with_schema(&grammar, schema);
//! assert!(sess.allowed_mask().test(SOURCE), "L2 only narrows; the L1 source token survives");
//! sess.accept_token(SOURCE)?;
//! assert!(sess.is_complete());
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! ## Status
//!
//! All milestones **M0–M5** are shipped. The core is the [`GuaranteeLevel`]
//! lattice, the [`vocab`] module's [`Vocab`] table (token id → raw bytes), the
//! byte-level recogniser (the [`grammar`] module's hand-written pushdown
//! automaton [`Pda`] over the emitted-Pure grammar (§5), the [`DecoderSession`]
//! that drives it as a [`ByteRecognizer`], and the [`DecodeError`] it reports),
//! the M2 mask cache ([`CompiledGrammar`]), and the M3 [`schema`] overlay:
//! [`Schema::from_json`] ingests the host contract as JSON and
//! [`DecoderSession::with_schema`] narrows the mask to schema-legal terminals at
//! each identifier/operand position. The gold-corpus loader and the Legend
//! completeness probe live in the test-oracle harness under `tests/` (see
//! `docs/decisions/0003-non-core-in-tests-deplight-core.md`); the core's runtime
//! dependencies are `thiserror` (error types) and `serde`/`serde_json` (the L2
//! JSON ingress, ADR-0005).
//!
//! Milestone **M4** adds the PyO3 boundary: the feature-gated `ffi` module
//! (compiled only under `--features python`) marshals the core to a Python
//! `purecard` extension module — a thin, decode-logic-free surface packaged as a
//! maturin abi3 wheel. The default build stays pyo3-free and pure.
//!
//! Milestone **M5** is the hardening pass: the [`selfcheck`] surface
//! ([`self_check`], [`self_check_smoke`], [`SelfCheckError`]) round-trips a host
//! tokenizer against the vocabulary before decode; [`accept_token`] finalizes on
//! the reserved EOS sentinel — the id one past the last vocab token
//! (`CompiledGrammar::eos_bit`), distinct from every real token id — accepted
//! only when the byte-PDA is in an accepting configuration (a complete query:
//! every frame closed and the last token lexed at a value boundary, so a trailing
//! top-level identifier terminates cleanly); the
//! [`DecodeError`] token-level channel is split into
//! [`InadmissibleToken`](DecodeError::InadmissibleToken),
//! [`UnknownToken`](DecodeError::UnknownToken), and
//! [`UnexpectedEos`](DecodeError::UnexpectedEos); and the fuzz targets and
//! benches under `fuzz/` and `benches/` guard the decoder against regressions.
//!
//! [`accept_token`]: DecoderSession::accept_token

pub mod error;
pub mod grammar;
pub mod mask;
pub mod recognizer;
pub mod schema;
pub mod selfcheck;
pub mod session;
pub mod vocab;

// The Python extension surface (M4): a private, feature-gated boundary module.
// Its items are not part of the Rust public API — they are reachable only from
// Python via the generated `purecard` module — so `deny(missing_docs)` does not
// reach them, but they are documented all the same.
#[cfg(feature = "python")]
mod ffi;

pub use error::DecodeError;
pub use grammar::Envelope;
pub use grammar::compiled::CompiledGrammar;
pub use grammar::pda::Pda;
pub use mask::BitMask;
pub use recognizer::ByteRecognizer;
pub use schema::{Schema, SchemaError};
pub use selfcheck::{SelfCheckError, self_check, self_check_smoke};
pub use session::DecoderSession;
pub use vocab::Vocab;

/// The nested guarantee levels a constrained decoder can offer, ordered from
/// weakest (the largest set of queries) to strongest (the smallest).
///
/// The sets form a strict containment hierarchy — every faithful query is
/// schema-consistent, and every schema-consistent query is syntactic, but not
/// the reverse:
///
/// ```text
/// faithful ⊂ schema-consistent ⊂ syntactic
/// ```
///
/// PureCARD moves a model's output into [`Syntactic`](GuaranteeLevel::Syntactic)
/// (L1) and, when given a schema, into
/// [`SchemaConsistent`](GuaranteeLevel::SchemaConsistent) (L2). It *cannot* reach
/// [`Faithful`](GuaranteeLevel::Faithful) (L3): a logits mask sees the schema and
/// the partial output, but never the question's intent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum GuaranteeLevel {
    /// **L1.** The query parses as (emitted-subset) Pure.
    Syntactic,
    /// **L2.** Every identifier and type resolves against a specific model: no
    /// phantom classes or properties, no type mismatch.
    SchemaConsistent,
    /// **L3.** The query actually answers the question that was asked.
    /// Structurally unreachable at decode time — PureCARD never claims it.
    Faithful,
}

impl GuaranteeLevel {
    /// The strongest guarantee PureCARD can enforce at decode time:
    /// schema-consistency (L2). [`Faithful`](GuaranteeLevel::Faithful) (L3) is
    /// out of reach by construction.
    pub const MAX_ENFORCEABLE: GuaranteeLevel = GuaranteeLevel::SchemaConsistent;

    /// Whether holding `self` also guarantees the weaker property `other`.
    ///
    /// Follows the containment hierarchy: a schema-consistent query is also
    /// syntactic, so `SchemaConsistent.guarantees(Syntactic)` is `true`, but
    /// `Syntactic.guarantees(SchemaConsistent)` is not.
    #[must_use]
    pub fn guarantees(self, other: GuaranteeLevel) -> bool {
        self >= other
    }

    /// Whether PureCARD can actually enforce this level — i.e. it is no stronger
    /// than [`MAX_ENFORCEABLE`](GuaranteeLevel::MAX_ENFORCEABLE). L3 is not
    /// enforceable.
    #[must_use]
    pub fn is_enforceable(self) -> bool {
        self <= Self::MAX_ENFORCEABLE
    }
}

#[cfg(test)]
mod tests {
    use super::GuaranteeLevel;
    use super::GuaranteeLevel::{Faithful, SchemaConsistent, Syntactic};

    #[test]
    fn containment_orders_weakest_to_strongest() {
        assert!(Syntactic < SchemaConsistent);
        assert!(SchemaConsistent < Faithful);
    }

    #[test]
    fn a_stronger_guarantee_implies_every_weaker_one() {
        assert!(SchemaConsistent.guarantees(Syntactic));
        assert!(SchemaConsistent.guarantees(SchemaConsistent));
        assert!(Faithful.guarantees(SchemaConsistent));
        assert!(Faithful.guarantees(Syntactic));
    }

    #[test]
    fn a_weaker_guarantee_does_not_imply_a_stronger_one() {
        assert!(!Syntactic.guarantees(SchemaConsistent));
        assert!(!Syntactic.guarantees(Faithful));
        assert!(!SchemaConsistent.guarantees(Faithful));
    }

    #[test]
    fn purecard_enforces_up_to_schema_consistency_only() {
        assert!(Syntactic.is_enforceable());
        assert!(SchemaConsistent.is_enforceable());
        assert!(!Faithful.is_enforceable());
        assert_eq!(GuaranteeLevel::MAX_ENFORCEABLE, SchemaConsistent);
    }
}
