//! The heavy tokenizer self-check round-trip (M5, `specs/m5-hardening.md` Area 1).
//!
//! The pure core ships only a ~4-query `SMOKE` set; the full-corpus proof lives
//! here. For every one of the 5034 gold `pure_text` queries this builds a
//! *faithful* per-query `Vocab` at lexeme granularity (the oracle harness's exact
//! partitioning tokenizer), compiles a grammar over it, and calls the shipped
//! [`purecard::self_check`] — asserting the host vocabulary can express the query:
//! it segments, every segment is admissible, and the stream completes.
//!
//! This is the token-surface analogue of `tests/soundness_replay.rs`: that lane
//! proves the byte-PDA streams every gold query; this proves a faithful token →
//! bytes vocabulary expresses every gold query through the *token* API, so
//! host-vocab drift would redden a gate.
#![forbid(unsafe_code)]

use std::path::PathBuf;

#[path = "support/corpus.rs"]
mod corpus;
#[path = "support/error.rs"]
mod error;
#[path = "support/lex.rs"]
mod lex;

use corpus::load_gold;
use lex::lex;
use purecard::{CompiledGrammar, Vocab, self_check};

/// A faithful per-query [`Vocab`]: exactly the distinct lexemes of `query`, in
/// first-seen order, EOS one past the last id. Greedy longest-match over these
/// reproduces the lexeme segmentation, so `self_check` drives the query through
/// its own faithful tokens.
fn faithful_query_vocab(query: &str) -> Vocab {
    let mut tokens: Vec<Vec<u8>> = Vec::new();
    for tok in lex(query) {
        if !tokens.contains(&tok) {
            tokens.push(tok);
        }
    }
    let eos = tokens.len() as u32;
    Vocab::from_byte_tokens(tokens, eos)
}

/// The full committed corpus size (arm-A 4639 + arm-C 395), asserted exactly so
/// corpus shrinkage reddens the gate.
const EXPECTED_GOLD_RECORDS: usize = 5034;

fn corpus_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("corpus/gold_queries.jsonl")
}

#[test]
fn every_gold_query_is_expressible_by_a_faithful_vocab() {
    let records = load_gold(&corpus_path()).expect("open the committed gold corpus");
    let mut checked = 0usize;
    for item in records {
        let record = item.expect("gold corpus line parses");
        let query = record.pure_text.as_str();
        // A faithful per-query vocabulary: exactly this query's lexemes.
        let grammar = CompiledGrammar::compile(faithful_query_vocab(query));
        if let Err(err) = self_check(&grammar, &[query.as_bytes()]) {
            panic!("SELF-CHECK: {} — {err}\n  query: {query}", record.source_id);
        }
        checked += 1;
    }
    assert_eq!(
        checked, EXPECTED_GOLD_RECORDS,
        "self-checked gold record count"
    );
}
