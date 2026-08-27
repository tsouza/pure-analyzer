//! Property test for issue #59's schema-aware walk generator
//! (`tests/support/schema_walker.rs`).
//!
//! `schema_walk_completeness.rs` already proves `L2 ⊆ L1` exhaustively over
//! every step of every generated walk. This lane instead mirrors
//! `mask_properties.rs`'s house style: draw a reachable (db, prefix) pair from
//! the generator's own output — never a synthetic state — then check the
//! containment property against **every** vocabulary id at that position, not
//! only the ids the walk happened to emit. That is strictly more than the
//! deterministic corpus test checks (it also exercises ids every generated
//! walk rejected at that point), at the cost of being sampled rather than
//! exhaustive.
//!
//! The (db, prefix) corpus is computed once, via [`std::sync::LazyLock`], not
//! per proptest case: `generate_schema_walks` runs the full 8-schema corpus,
//! and a naive per-case regeneration would make this lane the slowest in the
//! suite for no added signal (the prefixes are the same reachable set every
//! time — generation is deterministic, `schema_walk_completeness.rs` already
//! proves that).
#![forbid(unsafe_code)]

use std::path::PathBuf;
use std::sync::LazyLock;

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
use proptest::prelude::*;
use purecard::{CompiledGrammar, DecoderSession, Schema};
use schema_walker::generate_schema_walks;

/// The deterministic proptest case count for this lane (constitution §4 — no
/// magic constants; this is gate configuration, tunable up only).
const PROPTEST_CASES: u32 = 128;

/// See `schema_walk_completeness.rs`'s identical constant for the rationale
/// (not `support/synth.rs`'s `ALPHABET`: that module's other export,
/// `synthetic_vocab`, would go unused here).
const STRUCTURAL_BYTES: &[u8] = b"abXY1_ |{}()[].,;:$%'-><=!&+*/";

fn corpus_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("corpus/gold_queries.jsonl")
}

/// Build the grammar/schema pair for `db_id` — see
/// `schema_walk_completeness.rs`'s identical helper.
fn grammar_and_schema(db_id: &str) -> (CompiledGrammar, Schema) {
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

/// Every prefix (including the empty one) of every generated walk, tagged by
/// its index into [`FIXTURE_DBS`]. `CompiledGrammar`/`Schema` are not
/// `Sync` (the grammar's lazy per-state mask cache is a `!Sync` `OnceCell`),
/// so only the plain `(usize, Vec<u32>)` data — not the grammar/schema
/// themselves — is cached across proptest cases; each case rebuilds its
/// db's grammar/schema fresh, which is cheap (no walk generation involved).
fn walk_prefixes() -> Vec<(usize, Vec<u32>)> {
    let mut prefixes = Vec::new();
    for (db_index, db_id) in FIXTURE_DBS.iter().enumerate() {
        let (grammar, schema) = grammar_and_schema(db_id);
        for walk in generate_schema_walks(&grammar, &schema) {
            for len in 0..=walk.len() {
                prefixes.push((db_index, walk[..len].to_vec()));
            }
        }
    }
    prefixes
}

static WALK_PREFIXES: LazyLock<Vec<(usize, Vec<u32>)>> = LazyLock::new(walk_prefixes);

proptest! {
    // A fixed, committed config: deterministic case count, and regressions are
    // persisted so a discovered counterexample re-runs forever.
    #![proptest_config(ProptestConfig { cases: PROPTEST_CASES, ..ProptestConfig::default() })]

    /// `L2 ⊆ L1` at every reachable prefix, checked against every vocabulary
    /// id — not only the ids a generated walk happened to try.
    #[test]
    fn l2_mask_is_a_subset_of_l1_mask_for_every_id_at_every_reachable_prefix(
        seed in any::<prop::sample::Index>(),
    ) {
        let (db_index, prefix) = seed.get(&WALK_PREFIXES).clone();
        let db_id = FIXTURE_DBS[db_index];
        let (grammar, schema) = grammar_and_schema(db_id);

        let mut l1 = DecoderSession::new(&grammar);
        let mut l2 = DecoderSession::with_schema(&grammar, schema)
            .expect("grammar is fixed-engine");
        for &id in &prefix {
            l1.accept_token(id)
                .expect("a generated walk prefix is always L1-admissible");
            l2.accept_token(id)
                .expect("a generated walk prefix is always L2-admissible");
        }

        let vocab_len = grammar.vocab().len() as u32;
        let eos = vocab_len;
        for id in 0..=eos {
            let l1_admits = l1.allowed_mask().test(id);
            let l2_admits = l2.allowed_mask().test(id);
            prop_assert!(
                !l2_admits || l1_admits,
                "db {db_id} prefix {:?}: id {id} is L2-admissible but not L1-admissible",
                prefix
            );
        }
    }
}
