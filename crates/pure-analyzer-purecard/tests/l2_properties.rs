//! L2 structural properties (`docs/spec/schema.md` §8, spec M3 G4).
//!
//! The load-bearing invariant: **L2 never widens L1**. At every step of every
//! in-scope gold query, the schema-aware mask must be a subset of the L1-only
//! mask — the overlay may only clear bits, never set one L1 did not. This is a
//! consequence of the pure `intersect`, but the property test pins it against any
//! future change that might set a bit outside the L1 set (a mutant that flips the
//! intersect to a union, say). It also confirms the two sessions stay in lockstep
//! (identical acceptance) so the subset comparison is over the same positions.
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
/// threshold: a mis-count reddens the gate. Mirrors `l2_soundness.rs`.
const IN_SCOPE_TOTAL: usize = 269;

fn corpus_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("corpus/gold_queries.jsonl")
}

/// Assert the L2 mask is a subset of the L1 mask at the two sessions' current
/// position — every bit L2 admits, L1 admits too (`L2 ⊆ L1`).
fn assert_masks_subset(
    l1: &mut DecoderSession<'_>,
    l2: &mut DecoderSession<'_>,
    source_id: &str,
    query: &str,
) {
    let l1_mask = l1.allowed_mask().clone();
    let l2_mask = l2.allowed_mask();
    for set_id in l2_mask.iter_ones() {
        assert!(
            l1_mask.test(set_id),
            "L2 WIDENED L1 ({source_id}): token id {set_id} set in the schema mask \
             but not the L1 mask\n  {query}"
        );
    }
}

/// Assert `l2 ⊆ l1` at every gold token step of `query` (identified by `id`) —
/// including the terminal position after the final token.
fn assert_l2_subset_l1(
    grammar: &CompiledGrammar,
    schema: &purecard::Schema,
    vocab: &TokenVocab,
    source_id: &str,
    query: &str,
) {
    let mut l1 = DecoderSession::new(grammar);
    let mut l2 = DecoderSession::with_schema(grammar, schema.clone());
    for token in lex(query) {
        let id = vocab.id_of(&token).expect("gold token in vocab");
        assert_masks_subset(&mut l1, &mut l2, source_id, query);
        // Lockstep: the same token must be admissible to both (soundness already
        // proves L2 admits the gold token).
        l1.accept_token(id).expect("L1 admits gold");
        l2.accept_token(id).expect("L2 admits gold");
    }
    // The terminal position too: a regression that widens L2 only once the query
    // is complete (after the last accepted token) would slip past a prefix-only
    // check. `l2_soundness` pins the terminal EOS bit; this pins the full set.
    assert_masks_subset(&mut l1, &mut l2, source_id, query);
}

#[test]
fn l2_never_widens_l1_over_every_in_scope_gold_query() {
    let mut by_db: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    for item in load_gold(&corpus_path()).expect("open gold corpus") {
        let record = item.expect("gold line parses");
        if FIXTURE_DBS.contains(&record.db_id.as_str()) {
            by_db
                .entry(record.db_id)
                .or_default()
                .push((record.source_id, record.pure_text));
        }
    }

    let mut steps_checked = 0usize;
    for (db_id, queries) in &by_db {
        let schema = load_schema(db_id);
        let texts: Vec<&str> = queries.iter().map(|(_, text)| text.as_str()).collect();
        let vocab = TokenVocab::build(&texts, &[]);
        let grammar = CompiledGrammar::compile(vocab.vocab());
        for (source_id, query) in queries {
            assert_l2_subset_l1(&grammar, &schema, &vocab, source_id, query);
            steps_checked += 1;
        }
    }
    // Non-vacuity: the property actually ran over the whole in-scope corpus.
    assert_eq!(steps_checked, IN_SCOPE_TOTAL, "in-scope query count");
}
