//! State-coverage proof for issue #59's "record stable coverage of
//! states, ... fail on uncovered shipped rules" bullet, the residual half
//! `schema_walk_construct_coverage.rs` left open: that file's own doc
//! comment explains why it stays on frame/construct proxies rather than
//! `L2Position`/rule identity (private, the L2 overlay's scoping boundary) —
//! but `grammar::pda::State` carries no such boundary. It was already
//! `pub`, just not *enumerable* from outside the crate (the exhaustive
//! `ALL_STATES` list lived in `pda.rs`'s own `#[cfg(test)] mod tests`,
//! backing only the in-crate `index`/`COUNT` bijection check). Promoting
//! that one list to `#[doc(hidden)] pub` (no public-API-surface growth —
//! excluded from the `cargo public-api` snapshot) gives this file the same
//! source of truth, rather than a second, independently-maintained copy.
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
use purecard::grammar::pda::{ALL_STATES, State};
use purecard::{CompiledGrammar, DecoderSession};
use schema_walker::generate_schema_walks;

/// The structural bytes offered as their own single-byte token, in addition to
/// a db's gold-query lexemes, so a walk can explore beyond verbatim replay —
/// the same set the other coverage/completeness files use, kept as its own
/// copy per `fixture_dbs.rs`'s documented house convention (never partially
/// reuse a support module's neighbor).
const STRUCTURAL_BYTES: &[u8] = b"abXY1_ |{}()[].,;:$%'-><=!&+*/";

/// States `ALL_STATES` lists that this corpus is not expected to reach, each
/// with the concrete reason — a state absent here that the test still finds
/// unvisited is a real regression, not documented residue.
///
/// - `LetLe`: block-query syntax (`{|let m = …; …}`). The walker is
///   fundamentally class-anchored (`ClassPath.all()->…`, `docs/spec/grammar.md`
///   §5's arm-C shape) — it never opens a block query at all, so no state
///   reachable only through one can appear (`LetL` itself IS visited: it is
///   also the incidental first byte of any classpath merely starting with
///   `l`, a false start into the `let`-candidate path that diverges before
///   `Le`).
/// - `SawExp`/`NeedExpDigit`/`InExp`: scientific-notation numeric literals
///   (`1e5`). None of the 8 `FIXTURE_DBS` gold corpora (Spider-derived SQL
///   translations) contain one, and the walker only draws numeric tokens from
///   corpus lexemes plus `STRUCTURAL_BYTES`, neither of which supplies an `e`
///   exponent shape.
/// - `SawTilde`: the arm-R Relation/Function API sigil (`~Col`, `~[…]`). None
///   of the 8 `FIXTURE_DBS` gold corpora contain an arm-R construct (arm-R is
///   exercised elsewhere, e.g. `l2_precision.rs`'s hand-written queries, not
///   through this generator).
///
/// `InMultiplicity` and `InDateLit` were on this list until issue #117's
/// deeper/broader exploration (a wider `GROW_MIN`/`GROW_MAX`, several new
/// candidate-selection biases) started reaching them too — not through any
/// intentional multiplicity/date-literal construct, but incidentally: a much
/// longer walk occasionally strings `STRUCTURAL_BYTES`' `[`/`*` or `%`/digit
/// tokens adjacently by chance. Removed rather than re-added with a now-false
/// "never reaches" justification.
const EXPECTED_UNREACHABLE: &[&str] = &["LetLe", "SawExp", "NeedExpDigit", "InExp", "SawTilde"];

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

/// Every [`State`] reachable through the schema-walk generator's own
/// constructs is visited at least once across the whole generated corpus
/// (every fixture schema's [`generate_schema_walks`] output) — the residual
/// half of #59's "states... coverage" bullet
/// `schema_walk_construct_coverage.rs` left open. [`EXPECTED_UNREACHABLE`]
/// documents exactly which states this specific corpus cannot reach and why;
/// a state missing from *both* the visited set and that list is a real
/// coverage regression.
#[test]
fn every_reachable_pda_state_is_visited_at_least_once() {
    let mut visited: HashSet<State> = HashSet::new();
    for &db_id in FIXTURE_DBS {
        let (grammar, schema) = grammar_and_schema(db_id);
        let walks = generate_schema_walks(&grammar, &schema);
        for walk in &walks {
            let mut session = DecoderSession::with_schema(&grammar, schema.clone())
                .expect("grammar is fixed-engine");
            visited.insert(
                session
                    .pda()
                    .expect("fixed-engine grammar always exposes its Pda")
                    .state(),
            );
            for &id in walk {
                session
                    .accept_token(id)
                    .expect("a walk's own token is always admissible");
                visited.insert(
                    session
                        .pda()
                        .expect("fixed-engine grammar always exposes its Pda")
                        .state(),
                );
            }
        }
    }

    let missing: Vec<&str> = ALL_STATES
        .iter()
        .filter(|state| !visited.contains(state))
        .map(|state| state.name())
        .collect();
    let mut missing_sorted = missing.clone();
    missing_sorted.sort_unstable();
    let mut expected_sorted = EXPECTED_UNREACHABLE.to_vec();
    expected_sorted.sort_unstable();
    assert_eq!(
        missing_sorted, expected_sorted,
        "unvisited state set drifted from EXPECTED_UNREACHABLE — update the \
         documented list (with a reason) if this is an intentional corpus/walker \
         change, otherwise this is a real coverage regression"
    );
}
