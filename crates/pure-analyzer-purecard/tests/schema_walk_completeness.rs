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

#[test]
fn every_fixture_schema_generates_walks_that_replay_to_completion() {
    let extra: Vec<Vec<u8>> = STRUCTURAL_BYTES.iter().map(|&byte| vec![byte]).collect();

    let mut total_walks = 0usize;
    for db_id in FIXTURE_DBS {
        let queries: Vec<String> = load_gold(&corpus_path())
            .expect("open the committed gold corpus")
            .filter_map(Result::ok)
            .filter(|record| record.db_id == *db_id)
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
