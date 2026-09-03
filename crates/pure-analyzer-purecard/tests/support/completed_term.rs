//! The completed-term half of the L2 liveness contract (`docs/spec/schema.md`
//! §6.7): which positions govern a term that is already whole, and which bytes
//! end one.
//!
//! Shared via `#[path]` by the two lanes that assert the invariant over a walk —
//! `tests/l2_liveness.rs` (seeded, all 8 fixture schemas) and
//! `fuzz/fuzz_targets/l2_mask_liveness.rs` (the fuzzer chooses each step) — so the
//! classification cannot drift between them. Every symbol here is used by both.

use purecard::schema::L2Position;

/// The bytes that end a completed term instead of extending it: the three frame
/// closers, the `,` between elements, the `;` between block statements.
///
/// Restated here rather than read from the shipped `TERM_END_BYTES`
/// (`src/grammar/pda.rs`, where a unit test derives it from the `AfterValue` hub):
/// a test that read the constant it is checking would pass whatever that constant
/// said.
pub const TERM_END_BYTES: &[u8] = b"),;]}";

/// Whether `pos` governs a term that is already **whole** — a closed call or a
/// finished literal, with no step arrow half-emitted and nothing outstanding.
///
/// Exhaustive with no wildcard arm on purpose: a new [`L2Position`] has to be
/// classified here, so a future rule that governs a completed term is held to the
/// terminator invariant by construction rather than by someone remembering.
///
/// The `after_dash` halves are excluded because their term is *not* whole — the
/// `-` already emitted owes the `>` that completes its arrow. The two argument-slot
/// rules are excluded for the reason that decides the whole classification: a slot
/// with an arity still to meet has an obligation outstanding, and clearing a
/// terminator is how N3d and N3g state it.
#[must_use]
pub fn is_completed_term(pos: &L2Position) -> bool {
    match pos {
        L2Position::SourceExtent { after_dash }
        | L2Position::StoreResult { after_dash }
        | L2Position::StrOperator { after_dash } => !after_dash,
        L2Position::None
        | L2Position::SourceIdent
        | L2Position::BinderValueSourceIdent
        | L2Position::SourceMethod
        | L2Position::SourceMethodArg
        | L2Position::PropertyMethodArg
        | L2Position::StoreMethod
        | L2Position::StoreMethodArg
        | L2Position::StoreMethodArgSep { .. }
        | L2Position::ExtentMethod
        | L2Position::ExtentMethodArg(_)
        | L2Position::ReceiverOnlyArg
        | L2Position::LogicalOperand
        | L2Position::Member(_)
        | L2Position::ReValue(_)
        | L2Position::Comparator(_)
        | L2Position::OrderedOperand
        | L2Position::Reducer(_)
        | L2Position::ScalarMethod(_)
        | L2Position::Column
        | L2Position::RelationColumn
        | L2Position::RefVar
        | L2Position::ValueIdent => false,
    }
}
