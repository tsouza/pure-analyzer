//! L2 structural properties (`docs/spec/schema.md` §8, spec M3 G4).
//!
//! The load-bearing invariant: **L2 never widens L1**. At every step of every
//! in-scope gold query, the schema-aware mask must be a subset of the L1-only
//! mask — the overlay may only clear bits, never set one L1 did not. This is a
//! consequence of the pure `intersect`, but the property test pins it against any
//! future change that might set a bit outside the L1 set (a mutant that flips the
//! intersect to a union, say). It also confirms the two sessions stay in lockstep
//! (identical acceptance) so the subset comparison is over the same positions.
//!
//! The second invariant is the complementary direction, at a position §6's own
//! `L2 ⊆ L1` allows to be *equal*: a position the overlay has no rule for must
//! pass the L1 mask through unnarrowed, never merely a subset of it. A brace
//! lambda's binder-list comma (`{x,y,z|…}`'s `,` between `y` and `z`) is exactly
//! such a position — grammar-only structure, per the spec's own words — and
//! issue #351 found it narrowed anyway (regression pinned below).
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
use purecard::{CompiledGrammar, DecoderSession, Schema};

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
    let mut l2 =
        DecoderSession::with_schema(grammar, schema.clone()).expect("grammar is fixed-engine");
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

/// Issue #351's own hermetic schema: one class with an `Integer` and a `String`
/// property — minimal on purpose, since the witnesses below exercise nothing
/// beyond what a brace lambda's binder list needs to reach.
const ISSUE_351_SCHEMA_JSON: &str = r#"{
  "db_id": "issue351", "db_path": "issue351::Db",
  "classes": {"issue351::A": {"simple_name": "A", "properties": [
    {"name": "n", "type": {"kind": "primitive", "name": "Integer"}, "mult": {"lower": 1, "upper": 1}},
    {"name": "s", "type": {"kind": "primitive", "name": "String"},  "mult": {"lower": 1, "upper": 1}}],
    "qualified_properties": [], "super_types": []}},
  "associations": [], "enums": {}}"#;

/// Issue #351's own witnesses, generalized past its reported arity-3 to arity-5:
/// a brace lambda's binder list is L1 grammar structure with no arity cap
/// (`grammar/pda.rs`'s `separates_elements` admits a `,` after *any* completed
/// binder inside a `Frame::BraceLambda`), so L2 must admit every arity identically.
/// One-, two-, three-, four- and five-binder forms, each in both positions the
/// issue and this repo's differential corpus exercise a brace lambda at: a
/// `groupBy` aggregate key, and an `extend(over(…))` window frame.
const MULTI_BINDER_WITNESSES: &[(&str, &str)] = &[
    (
        "1-binder agg",
        "|issue351::A.all()->groupBy(~[s], ~'k': x|$x.n : y|$y->sum())",
    ),
    (
        "2-binder agg",
        "|issue351::A.all()->groupBy(~[s], ~'k': {x,y|$x.n} : y|$y->sum())",
    ),
    (
        "3-binder agg",
        "|issue351::A.all()->groupBy(~[s], ~'k': {x,y,z|$x.n} : y|$y->sum())",
    ),
    (
        "4-binder agg",
        "|issue351::A.all()->groupBy(~[s], ~'k': {w,x,y,z|$w.n} : y|$y->sum())",
    ),
    (
        "5-binder agg",
        "|issue351::A.all()->groupBy(~[s], ~'k': {v,w,x,y,z|$v.n} : y|$y->sum())",
    ),
    (
        "3-binder over",
        "|issue351::A.all()->groupBy(~[s], ~'k': x|$x.n : y|$y->sum())\
         ->extend(over(~k), ~'total': {p,w,r|$r.k} : y|$y->sum())",
    ),
];

/// Assert `l2` admits `query` exactly as `l1` does, token for token — the `L2 ⊆
/// L1` invariant sharpened to equality at a position the overlay has no rule
/// for. Unlike [`assert_l2_subset_l1`], which only ever checks the direction a
/// widening mutant could violate, this is the direction issue #351's own
/// regression needs: a *narrowing* one bit too far, at a position with no rule
/// to justify it.
fn assert_l2_admits_l1_admitted_binder_list(
    grammar: &CompiledGrammar,
    schema: &purecard::Schema,
    vocab: &TokenVocab,
    name: &str,
    query: &str,
) {
    let mut l1 = DecoderSession::new(grammar);
    let mut l2 =
        DecoderSession::with_schema(grammar, schema.clone()).expect("grammar is fixed-engine");
    for token in lex(query) {
        let id = vocab.id_of(&token).expect("witness token in vocab");
        assert!(
            l1.allowed_mask().test(id),
            "sanity: L1 must admit its own witness token ({name})\n  {query}"
        );
        assert!(
            l2.allowed_mask().test(id),
            "L2 NARROWED a binder-list position L1 admits ({name}): token {:?} masked\n  {query}",
            String::from_utf8_lossy(&token)
        );
        l1.accept_token(id).expect("L1 admits its own witness");
        l2.accept_token(id).unwrap_or_else(|err| {
            panic!("L2 rejected a witness token L1 admits ({name}): {err}\n  {query}")
        });
    }
    assert!(
        l1.is_complete(),
        "sanity: witness must be a complete L1 query ({name})"
    );
    assert!(
        l2.is_complete(),
        "L2 left an L1-complete binder-list witness incomplete ({name})\n  {query}"
    );
}

#[test]
fn l2_admits_every_brace_lambda_binder_list_arity_exactly_as_l1_does() {
    let schema = Schema::from_json(ISSUE_351_SCHEMA_JSON).expect("issue #351 schema parses");
    let texts: Vec<&str> = MULTI_BINDER_WITNESSES.iter().map(|(_, q)| *q).collect();
    let vocab = TokenVocab::build(&texts, &[]);
    let grammar = CompiledGrammar::compile(vocab.vocab());

    for (name, query) in MULTI_BINDER_WITNESSES {
        assert_l2_admits_l1_admitted_binder_list(&grammar, &schema, &vocab, name, query);
    }
}
