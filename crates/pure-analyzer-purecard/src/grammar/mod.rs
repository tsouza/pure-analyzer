//! L1: the emitted-Pure grammar as a byte-level pushdown automaton (§5).
//!
//! [`pda`] holds the live automaton — the [`State`](pda::State) /
//! [`Frame`](pda::Frame) machine and its pure [`step`](pda::step) function. This
//! module adds the two things the automaton itself does not carry: the
//! [`Envelope`] classifier that names which corpus idiom a query belongs to, and
//! the [`DeadState`] carrier the [`Pda`](pda::Pda) hands back on rejection.
//!
//! The emitted-Pure grammar (§5) is fixed: the recogniser plus the
//! [`CompiledGrammar`] vocabulary/mask cache (`docs/spec/architecture.md` §4),
//! and [`CompiledGrammar::from_spec`] ignores its `spec` argument, compiling the
//! single fixed PDA against the vocab.

pub mod compiled;
pub mod pda;

pub use compiled::CompiledGrammar;

/// The automaton configuration at the point a byte was rejected: the names of the
/// current [`State`](pda::State) and the top [`Frame`](pda::Frame).
///
/// [`Pda::advance`](pda::Pda::advance) returns this on a dead state; the
/// [`DecoderSession`](crate::DecoderSession) pairs it with the byte offset to
/// build a [`DecodeError::DeadState`](crate::DecodeError::DeadState). Both fields
/// are `&'static str` names, not the enums themselves, so the error type stays
/// free of the automaton's internal vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeadState {
    /// The name of the [`State`](pda::State) the automaton was in.
    pub state: &'static str,
    /// The name of the top [`Frame`](pda::Frame), or `"none"` for an empty stack.
    pub stack_top: &'static str,
}

/// Which observed corpus idiom a query uses (§5.1).
///
/// The envelopes are mechanically distinguishable and non-overlapping: an arm-A
/// query is the relational `tableReference(…)->tableToTDS()` pipeline, an arm-C
/// query is the class-navigation `Class.all()->…` form, and an arm-R query uses
/// the modern Relation/Function API (any `~`-column construct — `project(~[…])`,
/// `groupBy(~[…])`, `over(~…)`, …). The soundness gate partitions each corpus by
/// this classifier and asserts an exact record count per arm
/// (`specs/m1-l1-grammar.md`, G2; ADR-0007), so a mis-partitioned or missing query
/// reddens the gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Envelope {
    /// Arm A — the relational TDS envelope (`…->tableReference(…)->tableToTDS()`).
    Relational,
    /// Arm C — the class-navigation envelope (`…Class.all()->…`).
    ClassNav,
    /// Arm R — the modern Relation/Function API (any `~`-column construct). Seeded
    /// in `corpus/modern_dialect_seeds.jsonl` (ADR-0007); absent from the Spider
    /// gold corpus.
    RelationApi,
}

/// The marker substring of the arm-A relational envelope.
const RELATIONAL_MARKER: &str = "tableReference";
/// The marker substring of the arm-C class-navigation envelope. The opening `.all(`
/// (not the empty-arg `.all()`) so a *milestoned* source — `Class.all(%latest)` /
/// `Class.all(%latest, %latest)` — still classifies as class-navigation. Arm-A is
/// checked first, so an arm-A query that ever carried `.all(` cannot be
/// mis-binned; over the committed Spider corpus every arm-C query contains `.all()`
/// ⊇ `.all(`, so the tightening leaves all 4,639 / 395 gold classifications
/// unchanged.
const CLASS_NAV_MARKER: &str = ".all(";
/// The marker of the arm-R Relation/Function API envelope: the `~` column sigil.
/// Checked **first**, because an arm-R query is class-nav-sourced (`Class.all()->
/// project(~[…])`) and so also carries `.all(`; the `~` is the discriminator. No
/// Spider gold contains `~`, so this leaves all arm-A / arm-C classifications
/// unchanged and only re-bins the `~`-bearing modern-dialect seeds.
const RELATION_API_MARKER: &str = "~";

impl Envelope {
    /// Classify a query by its envelope marker.
    ///
    /// Returns [`RelationApi`](Envelope::RelationApi) for any `~`-column construct,
    /// else [`Relational`](Envelope::Relational) for the `tableReference` marker,
    /// else [`ClassNav`](Envelope::ClassNav) for the `.all(` marker (the opening
    /// paren, so a milestoned `.all(%latest)` source still classifies as
    /// class-navigation), else `None`. The `~` check is first because an arm-R query
    /// is class-nav-sourced and thus also matches `.all(`; over the Spider gold
    /// corpus (no `~`) the order is immaterial and every classification is unchanged.
    #[must_use]
    pub fn classify(query: &str) -> Option<Envelope> {
        if query.contains(RELATION_API_MARKER) {
            Some(Envelope::RelationApi)
        } else if query.contains(RELATIONAL_MARKER) {
            Some(Envelope::Relational)
        } else if query.contains(CLASS_NAV_MARKER) {
            Some(Envelope::ClassNav)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Envelope;

    #[test]
    fn relational_query_classifies_as_arm_a() {
        let query = "|db::Db->tableReference('default','T')->tableToTDS()->limit(1)";
        assert_eq!(Envelope::classify(query), Some(Envelope::Relational));
    }

    #[test]
    fn class_nav_query_classifies_as_arm_c() {
        let query = "|spider::geo::model::default::River.all()->project([x|$x.n], ['n'])";
        assert_eq!(Envelope::classify(query), Some(Envelope::ClassNav));
    }

    #[test]
    fn a_query_with_neither_marker_is_unclassified() {
        assert_eq!(Envelope::classify("|X->foo()"), None);
    }

    #[test]
    fn a_relation_api_query_classifies_as_arm_r() {
        // Any `~`-column construct is arm-R, even though the query is class-nav
        // sourced (it also contains `.all(`) — the `~` is checked first.
        assert_eq!(
            Envelope::classify("|X.all()->project(~[Col: x|$x.a])"),
            Some(Envelope::RelationApi)
        );
        // A query with neither `~` nor a relational/class marker is still None.
        assert_eq!(Envelope::classify("|X->foo()"), None);
    }

    #[test]
    fn a_milestoned_class_nav_source_still_classifies_as_arm_c() {
        // A milestoned `.all(%latest)` / `.all(%latest, %latest)` source is
        // class-navigation, not unclassified — the marker is the opening `.all(`.
        assert_eq!(
            Envelope::classify("|spider::geo::River.all(%latest)->take(1)"),
            Some(Envelope::ClassNav)
        );
        assert_eq!(
            Envelope::classify("|spider::geo::River.all(%latest, %latest)->take(1)"),
            Some(Envelope::ClassNav)
        );
    }
}
