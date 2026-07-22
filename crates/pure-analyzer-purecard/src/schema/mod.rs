//! L2: the schema-consistency overlay (`docs/spec/schema.md` §6).
//!
//! Given a [`Schema`] for the target database, L2 narrows the L1 mask at exactly
//! the identifier and operand positions §7 enumerates, so a partial query
//! references only real, correctly-typed model elements. It is composed of three
//! pure (crate-internal) pieces:
//!
//! - `model` — the [`Schema`] data-contract (§6.2) and its JSON ingress;
//! - `scope` — the `ScopeTracker` state machine (§6.4) that threads a typed scope
//!   through the parse and yields an `L2Position`;
//! - `narrow` — the N/T rules (§6.5–§6.6) that turn a position into a schema-legal
//!   [`BitMask`](crate::mask::BitMask) the mask is intersected with.
//!
//! Because the composition is a pure intersect, L2 only ever *clears* bits: the
//! `L2 ⊆ L1` guarantee is structural. When a [`DecoderSession`](crate::DecoderSession)
//! holds no schema the overlay is skipped entirely (zero added per-step cost).
//!
//! # Shipped vs. deferred rules
//!
//! This milestone ships the rules the 8 committed schema fixtures prove sound and
//! precise: **N3** (source-class exists), **N1/N2** (member/nav after `.`), **N6**
//! (relation-column strings), and **T1** (comparison operand type-class — the
//! `car_1` `horsepower:String` lever). T1 ships only its **string/numeric** levers
//! (the classes with a corpus operand); Boolean and Temporal operand narrowing is
//! deferred and passes through (see `narrow`). The `ScopeTracker` (S1–S3) ships **whole**
//! — a partial scope machine is a soundness hazard. Deferred (documented, not
//! half-built): N5-as-a-distinct-rule (the `navigable` map still ships — N1/N2
//! need it), T2/T3/T4/T6/T7 (thin corpus coverage), and N4/T5 (inert — no corpus
//! enum operand exists; they mask nothing).

pub(crate) mod model;
pub(crate) mod narrow;
pub(crate) mod scope;
pub(crate) mod trie;

pub use model::{Schema, SchemaError};
