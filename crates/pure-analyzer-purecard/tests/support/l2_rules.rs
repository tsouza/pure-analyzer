//! The shipped L2 rule/scope-transition kinds, as stable display names.
//!
//! Shared via `#[path]` by the lanes that reason about *which* rule is active at
//! a position rather than what it masks: `schema_walk_rule_coverage.rs` (every
//! reachable rule fires somewhere in the generated corpus), `l2_precision.rs`
//! (every rule is the recorded closer of at least one frozen fixture), and
//! `l2_liveness.rs` (the liveness walk actually reached the rules issues #275 and
//! #296 lived in). The first two drive both symbols; the liveness lane names its
//! rules directly and reads only `rule_kind`, so the registry carries an
//! `allow(dead_code)` for that target rather than being duplicated per lane.

use purecard::schema::L2Position;

/// Every rule/scope-transition kind [`L2Position`] can name, as its stable
/// display name — the source of truth [`rule_kind`] must stay in lockstep
/// with (an exhaustive match with no wildcard arm makes a dropped variant a
/// compile error here, not a silent coverage hole).
#[allow(dead_code)]
pub const ALL_RULE_KINDS: &[&str] = &[
    "SourceIdent",
    "SourceMethod",
    "SourceMethodArg",
    "PropertyMethodArg",
    "StoreMethod",
    "StoreMethodArg",
    "StoreMethodArgSep",
    "SourceExtent",
    "ExtentMethod",
    "ExtentMethodArg",
    "ReceiverOnlyArg",
    "StoreResult",
    "StrOperator",
    "LogicalOperand",
    "Member",
    "ReValue",
    "Comparator",
    "OrderedOperand",
    "Reducer",
    "ScalarMethod",
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
        L2Position::SourceIdent | L2Position::BinderValueSourceIdent => Some("SourceIdent"),
        L2Position::SourceMethod => Some("SourceMethod"),
        L2Position::StoreMethod => Some("StoreMethod"),
        L2Position::SourceMethodArg => Some("SourceMethodArg"),
        L2Position::PropertyMethodArg => Some("PropertyMethodArg"),
        L2Position::StoreMethodArg => Some("StoreMethodArg"),
        L2Position::StoreMethodArgSep { .. } => Some("StoreMethodArgSep"),
        L2Position::SourceExtent { .. } => Some("SourceExtent"),
        L2Position::ExtentMethod => Some("ExtentMethod"),
        L2Position::ExtentMethodArg(_) => Some("ExtentMethodArg"),
        L2Position::ReceiverOnlyArg => Some("ReceiverOnlyArg"),
        L2Position::StoreResult { .. } => Some("StoreResult"),
        L2Position::StrOperator { .. } => Some("StrOperator"),
        L2Position::LogicalOperand => Some("LogicalOperand"),
        L2Position::Member(_) => Some("Member"),
        L2Position::ReValue(_) => Some("ReValue"),
        L2Position::Comparator(_) => Some("Comparator"),
        L2Position::OrderedOperand => Some("OrderedOperand"),
        L2Position::Reducer(_) => Some("Reducer"),
        L2Position::ScalarMethod(_) => Some("ScalarMethod"),
        L2Position::Column => Some("Column"),
        L2Position::RelationColumn => Some("RelationColumn"),
        L2Position::RefVar => Some("RefVar"),
        L2Position::ValueIdent => Some("ValueIdent"),
        L2Position::None => None,
    }
}
