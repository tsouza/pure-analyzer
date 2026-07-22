//! The L2 soundness killer-test (`docs/spec/schema.md` §8.1, spec M3 G1).
//!
//! For every gold `pure_text` whose `db_id` has a committed schema fixture (the 8
//! fixtures → 269 in-scope queries), this builds the `Schema` via the shipped
//! `Schema::from_json`, creates a `DecoderSession::with_schema`, and replays the
//! query token-by-token under L2 — asserting the killer property: **no shipped
//! N/T rule ever masks a token the gold query actually emits** (the gold's next
//! token is admissible at every step), the stream never dead-states, and it is
//! `is_complete` (with the EOS bit set) at end-of-stream.
//!
//! Soundness must hold with the schema *active*, exactly as M1 holds without it.
//! If a gold query reddens this, the schema or a rule is wrong — the corpus is the
//! spec (§8.6): fix the rule to admit the token, never weaken the assertion.
//!
//! Honest coverage note: 256 of the 269 are arm-A relational, exercising only the
//! N6 relation-column check plus a table-exists check; the §6 property/type rules
//! fire on only the **13 arm-C** queries. L2 soundness runs over all 269, but the
//! load-bearing narrowing surface is those 13 (see `specs/m3-schema-overlay.md`).
#![forbid(unsafe_code)]

use std::collections::BTreeMap;
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
use l2::{TokenVocab, lex, load_schema};
use purecard::{CompiledGrammar, DecoderSession};

/// Total in-scope gold queries (the 8 fixtures). A named constant, not a
/// threshold: a mis-count reddens the gate.
const IN_SCOPE_TOTAL: usize = 269;
/// Arm-A (relational) in-scope count.
const IN_SCOPE_ARM_A: usize = 256;
/// Arm-C (class-navigation) in-scope count — the load-bearing narrowing surface.
const IN_SCOPE_ARM_C: usize = 13;

fn corpus_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("corpus/gold_queries.jsonl")
}

/// Replay `query`'s lexed tokens through a schema-aware session, asserting the
/// gold's next token is admissible at every step and the stream completes.
fn replay_under_l2(
    grammar: &CompiledGrammar,
    schema: &purecard::Schema,
    vocab: &TokenVocab,
    query: &str,
) {
    let mut session = DecoderSession::with_schema(grammar, schema.clone());
    for (step, token) in lex(query).into_iter().enumerate() {
        let id = vocab
            .id_of(&token)
            .unwrap_or_else(|| panic!("token not in vocab: {:?}", String::from_utf8_lossy(&token)));
        let mask = session.allowed_mask();
        assert!(
            mask.test(id),
            "L2 SOUNDNESS: rule masked a gold token at step {step} ({:?}) in query:\n  {query}",
            String::from_utf8_lossy(&token)
        );
        session
            .accept_token(id)
            .unwrap_or_else(|err| panic!("gold token rejected at step {step}: {err}\n  {query}"));
    }
    assert!(
        session.is_complete(),
        "L2 SOUNDNESS: stream not complete at EOS for query:\n  {query}"
    );
    assert!(
        session.allowed_mask().test(grammar.vocab().len() as u32),
        "L2 SOUNDNESS: EOS bit cleared by L2 for a complete query:\n  {query}"
    );
}

#[test]
fn every_in_scope_gold_query_streams_soundly_under_l2() {
    // Group the in-scope gold by db so each db's vocabulary and schema are built
    // once and shared across its queries.
    let mut by_db: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let records = load_gold(&corpus_path()).expect("open the committed gold corpus");
    let mut arm_a = 0usize;
    let mut arm_c = 0usize;
    for item in records {
        let record = item.expect("gold corpus line parses");
        if !FIXTURE_DBS.contains(&record.db_id.as_str()) {
            continue;
        }
        match record.arm.as_str() {
            "A" => arm_a += 1,
            "C" => arm_c += 1,
            other => panic!("unexpected arm {other} for {}", record.source_id),
        }
        by_db
            .entry(record.db_id)
            .or_default()
            .push(record.pure_text);
    }

    let mut total = 0usize;
    for (db_id, queries) in &by_db {
        let schema = load_schema(db_id);
        let refs: Vec<&str> = queries.iter().map(String::as_str).collect();
        let vocab = TokenVocab::build(&refs, &[]);
        let grammar = CompiledGrammar::compile(vocab.vocab());
        for query in &refs {
            replay_under_l2(&grammar, &schema, &vocab, query);
            total += 1;
        }
    }

    assert_eq!(arm_a, IN_SCOPE_ARM_A, "arm-A in-scope count");
    assert_eq!(arm_c, IN_SCOPE_ARM_C, "arm-C in-scope count");
    assert_eq!(total, IN_SCOPE_TOTAL, "total in-scope replay count");
}
