//! First proof for issue #59 (deterministic accepting PureCARD walks under a
//! supplied schema): for every committed fixture schema, [`generate_schema_walks`]
//! produces [`WALK_COUNT`] token-id walks that each replay cleanly through a
//! fresh schema-aware [`DecoderSession`] to completion. This pins the
//! generator's API shape and end-to-end soundness before its coverage
//! bookkeeping (states, frames, scope transitions, rule firings, tokenization
//! shapes, output constructs) is layered on in a follow-up.
#![forbid(unsafe_code)]

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
#[path = "support/schema_walker.rs"]
mod schema_walker;

use corpus::load_gold;
use fixture_dbs::FIXTURE_DBS;
use l2::{TokenVocab, load_schema};
use purecard::{CompiledGrammar, DecoderSession};
use schema_walker::{WALK_COUNT, generate_schema_walks};

/// The structural bytes offered as their own single-byte token, in addition to
/// a db's gold-query lexemes, so a walk can explore beyond verbatim replay.
/// Not `support/synth.rs`'s `ALPHABET`: that module's other export
/// (`synthetic_vocab`) is unused here, and `fixture_dbs.rs` documents the
/// house convention of never partially using a support module.
const STRUCTURAL_BYTES: &[u8] = b"abXY1_ |{}()[].,;:$%'-><=!&+*/";

fn corpus_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("corpus/gold_queries.jsonl")
}

/// Build the grammar/schema pair for `db_id`: a vocabulary of every lexeme in
/// that db's gold queries plus every [`STRUCTURAL_BYTES`] byte as its own
/// token (so a walk can explore beyond verbatim replay), compiled against the
/// db's committed schema fixture.
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

#[test]
fn every_fixture_schema_generates_walks_that_replay_to_completion() {
    let mut total_walks = 0usize;
    for db_id in FIXTURE_DBS {
        let (grammar, schema) = grammar_and_schema(db_id);

        let walks = generate_schema_walks(&grammar, &schema);
        assert_eq!(walks.len(), WALK_COUNT, "walk count for db {db_id}");

        for (index, walk) in walks.iter().enumerate() {
            let mut session = DecoderSession::with_schema(&grammar, schema.clone())
                .expect("grammar is fixed-engine");
            for &id in walk {
                session.accept_token(id).unwrap_or_else(|err| {
                    panic!("db {db_id} walk {index} rejected token {id}: {err}")
                });
            }
            assert!(
                session.is_complete(),
                "db {db_id} walk {index} did not reach completion"
            );
        }
        total_walks += walks.len();
    }
    assert_eq!(total_walks, WALK_COUNT * FIXTURE_DBS.len());
}

#[test]
fn generation_is_deterministic_across_repeated_calls() {
    for db_id in FIXTURE_DBS {
        let (grammar, schema) = grammar_and_schema(db_id);
        let first = generate_schema_walks(&grammar, &schema);
        let second = generate_schema_walks(&grammar, &schema);
        assert_eq!(
            first, second,
            "walk set for db {db_id} was not reproducible"
        );
    }
}

/// Every generated walk is admissible under **both** L1 and L2 at every step,
/// and the L2 mask never admits an id L1 would reject (`L2 ⊆ L1`, the
/// subtractive-overlay invariant §6 requires). Across the whole corpus, L2
/// must also narrow at least one step relative to L1 — a schema overlay that
/// never differs from L1 would be silently pass-through, not proof of a real
/// constraint.
#[test]
fn every_step_is_l1_and_l2_admissible_with_l2_subset_of_l1_and_narrower_somewhere() {
    let mut narrowed_anywhere = false;
    for db_id in FIXTURE_DBS {
        let (grammar, schema) = grammar_and_schema(db_id);
        let walks = generate_schema_walks(&grammar, &schema);

        for (index, walk) in walks.iter().enumerate() {
            let mut l1 = DecoderSession::new(&grammar);
            let mut l2 = DecoderSession::with_schema(&grammar, schema.clone())
                .expect("grammar is fixed-engine");
            for (step, &id) in walk.iter().enumerate() {
                let l1_ids: std::collections::BTreeSet<u32> =
                    l1.allowed_mask().iter_ones().collect();
                let l2_ids: std::collections::BTreeSet<u32> =
                    l2.allowed_mask().iter_ones().collect();
                assert!(
                    l2_ids.is_subset(&l1_ids),
                    "db {db_id} walk {index} step {step}: L2 admitted an id L1 rejects"
                );
                assert!(
                    l1_ids.contains(&id),
                    "db {db_id} walk {index} step {step}: emitted token {id} not L1-admissible"
                );
                assert!(
                    l2_ids.contains(&id),
                    "db {db_id} walk {index} step {step}: emitted token {id} not L2-admissible"
                );
                if l2_ids.len() < l1_ids.len() {
                    narrowed_anywhere = true;
                }
                l1.accept_token(id).unwrap_or_else(|err| {
                    panic!("db {db_id} walk {index}: L1 rejected {id}: {err}")
                });
                l2.accept_token(id).unwrap_or_else(|err| {
                    panic!("db {db_id} walk {index}: L2 rejected {id}: {err}")
                });
            }
        }
    }
    assert!(
        narrowed_anywhere,
        "L2 never narrowed the mask relative to L1 across any fixture schema's walks — \
         the schema overlay would be unproven, not just unexercised"
    );
}

/// The schema's internal `L2Position`/rule classification is private (the L2
/// overlay's scoping boundary, ADR-0010 and `DecoderSession::with_schema`'s
/// docs), so this cannot name *which* shipped rule (N1/N2/N3/N6/T1-numeric/
/// T1-string) fired at a given step. What is externally observable — and
/// still a genuine, checkable coverage signal — is the PDA `(state,
/// stack-top)` configuration active whenever L2 narrows relative to L1: each
/// named [`State`](purecard::grammar::pda::State) corresponds to a distinct
/// narrowing context, so narrowing at only one configuration would mean the
/// generated corpus exercises at most one rule family, not several.
#[test]
fn schema_narrowing_spans_more_than_one_pda_configuration() {
    let mut narrowed_configurations: std::collections::HashSet<String> =
        std::collections::HashSet::new();
    for db_id in FIXTURE_DBS {
        let (grammar, schema) = grammar_and_schema(db_id);
        let walks = generate_schema_walks(&grammar, &schema);

        for walk in &walks {
            let mut l1 = DecoderSession::new(&grammar);
            let mut l2 = DecoderSession::with_schema(&grammar, schema.clone())
                .expect("grammar is fixed-engine");
            for &id in walk {
                let l1_ids: std::collections::BTreeSet<u32> =
                    l1.allowed_mask().iter_ones().collect();
                let l2_ids: std::collections::BTreeSet<u32> =
                    l2.allowed_mask().iter_ones().collect();
                if l2_ids.len() < l1_ids.len() {
                    let pda = l2
                        .pda()
                        .expect("fixed-engine grammar always exposes its Pda");
                    narrowed_configurations.insert(format!(
                        "{:?}/{:?}",
                        pda.state(),
                        pda.stack_top()
                    ));
                }
                l1.accept_token(id)
                    .expect("a walk's own token is always L1-admissible");
                l2.accept_token(id)
                    .expect("a walk's own token is always L2-admissible");
            }
        }
    }
    assert!(
        narrowed_configurations.len() > 1,
        "schema narrowing was observed at only {} distinct (state, stack-top) \
         configuration(s) across the whole corpus: {narrowed_configurations:?} — \
         expected the shipped rule families to each narrow at their own position",
        narrowed_configurations.len()
    );
}
