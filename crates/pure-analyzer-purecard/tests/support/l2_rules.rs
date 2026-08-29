//! The shipped L2 rule/scope-transition kinds, as stable display names.
//!
//! Shared via `#[path]` by the two lanes that reason about *which* rule is
//! active at a position rather than what it masks: `schema_walk_rule_coverage.rs`
//! (every reachable rule fires somewhere in the generated corpus) and
//! `l2_precision.rs` (every rule is the recorded closer of at least one frozen
//! fixture). Both use both symbols, so nothing here is a partial reuse.

use purecard::schema::L2Position;

/// Every rule/scope-transition kind [`L2Position`] can name, as its stable
/// display name — the source of truth [`rule_kind`] must stay in lockstep
/// with (an exhaustive match with no wildcard arm makes a dropped variant a
/// compile error here, not a silent coverage hole).
pub const ALL_RULE_KINDS: &[&str] = &[
    "SourceIdent",
    "SourceMethod",
    "SourceMethodArg",
    "StoreMethod",
    "StoreMethodArg",
    "StoreMethodArgSep",
    "SourceExtent",
    "ExtentMethod",
    "ReceiverOnlyArg",
    "StoreResult",
    "StrOperator",
    "LogicalOperand",
    "Member",
    "ReValue",
    "Comparator",
    "Reducer",
    "Column",
    "RelationColumn",
    "RefVar",
    "ValueIdent",
];

/// [`L2Position`]'s stable display name, ignoring any payload (a `Member("A")`
/// and a `Member("B")` are the same *rule* for coverage purposes). `None`
/// (no constraint at this position) is not a rule firing, so it has no name.
#[must_use]
pub fn rule_kind(pos: &L2Position) -> Option<&'static str> {
    match pos {
        L2Position::SourceIdent => Some("SourceIdent"),
        L2Position::SourceMethod => Some("SourceMethod"),
        L2Position::StoreMethod => Some("StoreMethod"),
        L2Position::SourceMethodArg => Some("SourceMethodArg"),
        L2Position::StoreMethodArg => Some("StoreMethodArg"),
        L2Position::StoreMethodArgSep { .. } => Some("StoreMethodArgSep"),
        L2Position::SourceExtent { .. } => Some("SourceExtent"),
        L2Position::ExtentMethod => Some("ExtentMethod"),
        L2Position::ReceiverOnlyArg => Some("ReceiverOnlyArg"),
        L2Position::StoreResult { .. } => Some("StoreResult"),
        L2Position::StrOperator { .. } => Some("StrOperator"),
        L2Position::LogicalOperand => Some("LogicalOperand"),
        L2Position::Member(_) => Some("Member"),
        L2Position::ReValue(_) => Some("ReValue"),
        L2Position::Comparator(_) => Some("Comparator"),
        L2Position::Reducer(_) => Some("Reducer"),
        L2Position::Column => Some("Column"),
        L2Position::RelationColumn => Some("RelationColumn"),
        L2Position::RefVar => Some("RefVar"),
        L2Position::ValueIdent => Some("ValueIdent"),
        L2Position::None => None,
    }
}
