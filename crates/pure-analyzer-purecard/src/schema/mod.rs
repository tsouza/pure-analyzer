//! L2: the schema-consistency overlay (`docs/spec/schema.md` §6).
//!
//! Given a [`Schema`] for the target database, the implemented L2 subset narrows
//! the L1 mask at its covered identifier and operand positions. At those
//! positions it removes prefixes that cannot reach the selected real,
//! type-compatible model elements; deferred positions pass through. It is
//! composed of three pure (crate-internal) pieces:
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
//! # Covered and pass-through rules
//!
//! This overlay applies the rules the 8 committed schema fixtures exercise and
//! precise: **N3** (source-class exists), **N1/N2** (member/nav after `.`), **N6**
//! (relation-column strings), and **T1** (comparison operand type-class — the
//! `car_1` `horsepower:String` lever). T1 applies its **string/numeric** levers;
//! Boolean and Temporal operands pass through (see `narrow`). The `ScopeTracker`
//! (S1–S3) is whole — a partial scope machine is a soundness hazard. N5 as a
//! distinct rule, T2/T3/T4/T6/T7, and N4/T5 pass through; the `navigable` map is
//! retained because N1/N2 need it.

pub(crate) mod model;
pub(crate) mod narrow;
pub(crate) mod scope;
pub(crate) mod trie;

pub use model::{Schema, SchemaError};
