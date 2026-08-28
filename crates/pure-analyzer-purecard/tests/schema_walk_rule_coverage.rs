//! Per-named-rule coverage proof for issue #59's "record stable coverage of
//! ... scope transitions, rule firings, ... ; fail on uncovered shipped
//! rules" bullet — the half `schema_walk_construct_coverage.rs`'s own doc
//! comment left open pending a scope-tracker visibility decision.
//! `DecoderSession::active_l2_position` (`#[doc(hidden)]`, test-support
//! surface only) now answers "which named rule would narrow the mask at this
//! step", so this file can assert every shipped rule fires at least once
//! across the whole generated corpus, mirroring
//! `schema_walk_state_coverage.rs`'s state-coverage pattern exactly.
//!
//! **Finding (2026-08-28): six rules are currently unreachable through this
//! generator** — see [`EXPECTED_UNFIRED`]. Writing this test (rather than
//! `schema_walk_construct_coverage.rs`'s substring-on-decoded-text proxy) is
//! what surfaced it: `generate_schema_walks`'s random exploration essentially
//! never produces a `Class.all()->filter(x|$x.field …)`-shaped walk deep
//! enough to reach class-member navigation at all, across all 512 walks
//! (64 × 8 `FIXTURE_DBS`). That also means `schema_walk_construct_coverage.rs`'s
//! "dot navigation" construct check has always been a false positive — every
//! literal `.` its 512-walk corpus contains is either a decimal point or an
//! unrelated over-approximation artifact (confirmed by direct inspection), not
//! real `$x.field` access. Filed as #117 for the walker's own
//! generation-bias fix; this file stays honest and green against the
//! generator's *current* behavior in the meantime, per the same
//! `EXPECTED_UNREACHABLE` convention `schema_walk_state_coverage.rs`
//! established for PDA states.
#![forbid(unsafe_code)]

use std::collections::HashSet;
use std::path::PathBuf;

#[path = "support/corpus.rs"]
mod corpus;
#[path = "support/error.rs"]
mod error;
#[path = "support/fixture_dbs.rs"]
mod fixture_dbs;
#[path = "support/l2.rs"]
mod l2;
#[path = "support/lex.rs"]
mod lex;

use corpus::load_gold;
use fixture_dbs::FIXTURE_DBS;
use l2::{TokenVocab, load_schema};
use purecard::schema::L2Position;
use purecard::{CompiledGrammar, DecoderSession};
use schema_walker::generate_schema_walks;

/// The structural bytes offered as their own single-byte token, in addition to
/// a db's gold-query lexemes — the same set the other coverage/completeness
/// files use, kept as its own copy per `fixture_dbs.rs`'s documented house
/// convention (never partially reuse a support module's neighbor).
const STRUCTURAL_BYTES: &[u8] = b"abXY1_ |{}()[].,;:$%'-><=!&+*/";

/// Every rule/scope-transition kind [`L2Position`] can name, as its stable
/// display name — the source of truth [`rule_kind`] must stay in lockstep
/// with (an exhaustive match with no wildcard arm makes a dropped variant a
/// compile error here, not a silent coverage hole).
const ALL_RULE_KINDS: &[&str] = &[
    "SourceIdent",
    "SourceMethod",
    "SourceMethodArg",
    "Member",
    "ReValue",
    "Comparator",
    "Reducer",
    "Column",
    "RelationColumn",
];

/// [`L2Position`]'s stable display name, ignoring any payload (a `Member("A")`
/// and a `Member("B")` are the same *rule* for coverage purposes). `None`
/// (no constraint at this position) is not a rule firing, so it has no name.
fn rule_kind(pos: &L2Position) -> Option<&'static str> {
    match pos {
        L2Position::SourceIdent => Some("SourceIdent"),
        L2Position::SourceMethod => Some("SourceMethod"),
        L2Position::SourceMethodArg => Some("SourceMethodArg"),
        L2Position::Member(_) => Some("Member"),
        L2Position::ReValue(_) => Some("ReValue"),
        L2Position::Comparator(_) => Some("Comparator"),
        L2Position::Reducer(_) => Some("Reducer"),
        L2Position::Column => Some("Column"),
        L2Position::RelationColumn => Some("RelationColumn"),
        L2Position::None => None,
    }
}

/// Rules [`ALL_RULE_KINDS`] lists that `generate_schema_walks`'s current
/// exploration never fires, each with the concrete reason — a rule missing
/// from *both* the fired set and this list is a real coverage regression, not
/// documented residue.
///
/// All six require the walker to build a `Class.all()->filter(x|$x.field …)`
/// (or `->groupBy`/`agg(…)`) shaped walk deep enough to reach class-member
/// navigation, an aggregation, or a comparison after one. Across all 512
/// walks (64 × 8 `FIXTURE_DBS`), it never does: direct inspection of the
/// decoded corpus shows every walk either stays at `.all()` (closing
/// immediately) or wanders into unrelated arm-A/relational shapes (a store
/// path, a bare method call) without ever completing a bound-variable member
/// access. This is a real gap in the *generator's* own exploration bias, not
/// a gap in what the shipped rules cover — filed as a follow-up, not fixed
/// here (fixing it means changing `schema-walker`'s own candidate weighting,
/// out of #59's "expose per-rule coverage" scope).
///
/// - `Member` (N1/N2): no walk ever completes `$x.field` after a class-bound
///   lambda binder.
/// - `ReValue`/`Comparator` (T1/T2): both need a completed `Member` navExpr
///   immediately to their left; neither can fire without it.
/// - `Reducer` (T3): needs a `y: <Primitive>[*]|$y-><reducer>()` reduce
///   lambda inside an `agg(…)` call, itself deep behind a `groupBy(…)` the
///   walker never reaches.
/// - `Column`/`RelationColumn` (N6): need a closed `project`/`groupBy`
///   establishing op (arm-R), which the walker also never reaches.
const EXPECTED_UNFIRED: &[&str] = &[
    "Member",
    "ReValue",
    "Comparator",
    "Reducer",
    "Column",
    "RelationColumn",
];

fn corpus_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("corpus/gold_queries.jsonl")
}

fn grammar_and_schema(db_id: &str) -> (CompiledGrammar, purecard::Schema) {
    let extra: Vec<Vec<u8>> = STRUCTURAL_BYTES.iter().map(|&byte| vec![byte]).collect();
    let queries: Vec<String> = load_gold(&corpus_path())
        .expect("open the committed gold corpus")
        .filter_map(Result::ok)
        .filter(|record| record.db_id == db_id)
        .map(|record| record.pure_text)
        .collect();
    assert!(
        !queries.is_empty(),
        "no gold queries for fixture db {db_id}"
    );
    let refs: Vec<&str> = queries.iter().map(String::as_str).collect();

    let vocab = TokenVocab::build(&refs, &extra);
    let grammar = CompiledGrammar::compile(vocab.vocab());
    let schema = load_schema(db_id);
    (grammar, schema)
}

/// Every named L2 rule/scope-transition ([`ALL_RULE_KINDS`]) the generator can
/// currently reach fires at least once across the whole generated corpus
/// (every fixture schema's [`generate_schema_walks`] output). [`EXPECTED_UNFIRED`]
/// documents exactly which rules this generator cannot yet reach and why; a
/// rule missing from *both* the fired set and that list is a real coverage
/// regression, not documented residue — mirroring
/// `schema_walk_state_coverage.rs`'s `every_reachable_pda_state_is_visited_at_least_once`
/// exactly.
#[test]
fn every_reachable_rule_fires_at_least_once() {
    let mut fired: HashSet<&'static str> = HashSet::new();
    for &db_id in FIXTURE_DBS {
        let (grammar, schema) = grammar_and_schema(db_id);
        let walks = generate_schema_walks(&grammar, &schema);
        for walk in &walks {
            let mut session = DecoderSession::with_schema(&grammar, schema.clone())
                .expect("grammar is fixed-engine");
            if let Some(pos) = session.active_l2_position()
                && let Some(kind) = rule_kind(&pos)
            {
                fired.insert(kind);
            }
            for &id in walk {
                session
                    .accept_token(id)
                    .expect("a walk's own token is always admissible");
                if let Some(pos) = session.active_l2_position()
                    && let Some(kind) = rule_kind(&pos)
                {
                    fired.insert(kind);
                }
            }
        }
    }

    let mut missing: Vec<&str> = ALL_RULE_KINDS
        .iter()
        .filter(|kind| !fired.contains(*kind))
        .copied()
        .collect();
    missing.sort_unstable();
    let mut expected_sorted = EXPECTED_UNFIRED.to_vec();
    expected_sorted.sort_unstable();
    assert_eq!(
        missing, expected_sorted,
        "unfired rule set drifted from EXPECTED_UNFIRED — update the documented \
         list (with a reason) if this is an intentional walker/corpus change, \
         otherwise this is a real coverage regression"
    );
}
