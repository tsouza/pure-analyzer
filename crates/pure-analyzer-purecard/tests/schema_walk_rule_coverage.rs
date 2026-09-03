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
//! **Finding (2026-08-28, issue #117): six rules were unreachable through this
//! generator**, confirmed by writing this test rather than trusting
//! `schema_walk_construct_coverage.rs`'s substring-on-decoded-text proxy — its
//! "dot navigation" check turned out to be a false positive matching decimal
//! points and unrelated over-approximation artifacts, never real `$x.field`
//! access. `generate_schema_walks` (`schema-walker/src/lib.rs`) was first
//! fixed (#117) to bias its random exploration — a wider growth budget, a
//! preference for real classes over the store path at the source position, a
//! preference for real Pure builtin method names right after `->`, and
//! biasing a lambda binder's later `$`-reference (and the `.` right after it)
//! toward reusing the exact name it was declared with. That closed the gap
//! for **`Column`** (N6) alone — weight tuning could not reliably reach the
//! rest: reaching `$x.field cmp literal` needs *every* one of several nested
//! grammar branches to independently land on the specific path toward
//! navigation, and escalating every bias's weight roughly 25× produced no
//! change in which rules fired.
//!
//! **Fix (issue #119): deterministic, schema-parameterized "recipe" walks.**
//! Rather than nudge independent per-token weights further, `generate_schema_walks`
//! now builds a handful of walks that commit to a target production shape up
//! front — `Class.all()->filter(a|$a.<member> < <digit>)` for `Member`/
//! `Comparator`/`ReValue`, `Class.all()->agg(a|$a.<member>,b:<PrimType>[*]|$b-><reducer>())`
//! for `Reducer` — substituting real class/member names (and any primitive
//! type-annotation/reducer name it can find) looked up directly from the
//! vocabulary, and only falls back to `None` (silently, per db) when no
//! admissible combination exists. These walks are tried first and included
//! whenever they succeed, with `attempt`'s random exploration filling the
//! remainder up to `WALK_COUNT`. That closed all four of the remaining
//! `filter`-reachable rules.
//!
//! **`RelationColumn` (N6, arm-R bare-ident form) stays permanently
//! unreachable**, for a structural reason no amount of weight tuning or
//! recipe-building can fix: it needs a bare `~[...]` column-set argument, and
//! **none of the 8 fixture corpora use arm-R syntax at all** (the same reason
//! `schema_walk_state_coverage.rs`'s `SawTilde` state is permanently
//! unreachable there) — there is no real column name to substitute, and
//! synthesizing one would mean inventing vocabulary content no committed gold
//! query actually contains. This mirrors N4/T5's existing "no evidence, no
//! implementation" precedent (`docs/spec/schema.md` §6.5/§6.6) rather than
//! forcing a walk through content that isn't real. `EXPECTED_UNFIRED`
//! documents this so the gap stays honest and visible, mirroring
//! `schema_walk_state_coverage.rs`'s `EXPECTED_UNREACHABLE` convention.
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
#[path = "support/l2_rules.rs"]
mod l2_rules;
#[path = "support/lex.rs"]
mod lex;

use corpus::load_gold;
use fixture_dbs::FIXTURE_DBS;
use l2::{TokenVocab, load_schema};
use l2_rules::{ALL_RULE_KINDS, rule_kind};
use purecard::{CompiledGrammar, DecoderSession};
use schema_walker::generate_schema_walks;

/// The structural bytes offered as their own single-byte token, in addition to
/// a db's gold-query lexemes — the same set the other coverage/completeness
/// files use, kept as its own copy per `fixture_dbs.rs`'s documented house
/// convention (never partially reuse a support module's neighbor).
const STRUCTURAL_BYTES: &[u8] = b"abXY1_ |{}()[].,;:$%'-><=!&+*/";

/// Rules [`ALL_RULE_KINDS`] lists that `generate_schema_walks`'s current
/// exploration never fires — a rule missing from *both* the fired set and
/// this list is a real coverage regression, not documented residue. See the
/// module doc comment (issues #117/#119) for the full investigation.
///
/// - `RelationColumn` (N6, arm-R bare-ident form): needs a closed
///   `groupBy(~[…], …)` establishing op with a bare column-set argument, and
///   none of the 8 fixture corpora use arm-R syntax at all — there is no real
///   column name any recipe could substitute (unlike the quoted-string
///   `Column` form, reachable via arm-A TDS-getter/`project`-string-argument
///   shapes that don't need arm-R at all). Permanently out of scope, the same
///   way N4/T5 are (`docs/spec/schema.md` §6.5/§6.6): no evidence, no
///   implementation.
/// - `SourceMethodArgSep` (issue #384's `all()`-arity separator, right after a
///   *completed* milestoning argument in a class whose declared arity the
///   schema states): needs a fixture class carrying the new `temporal` field,
///   and none of the 8 Spider-derived fixture schemas do — the corpus predates
///   the field entirely (that absence is exactly what issue #384 reports), so
///   there is no real milestoned class any recipe could substitute without
///   fabricating schema content the corpus does not actually carry. The
///   identical "no evidence, no implementation" reasoning as `RelationColumn`
///   above; `SourceMethodArg`'s own value-slot half stays reachable
///   (`required: None`, the pass-through case every fixture class hits), only
///   its arity-aware separator does not. Live-verified directly instead, via
///   `tests/l2_precision.rs`'s dedicated `milestoning` schema fixture and
///   `tests/fixtures/schemas/milestoning.json`.
const EXPECTED_UNFIRED: &[&str] = &["RelationColumn", "SourceMethodArgSep"];

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
