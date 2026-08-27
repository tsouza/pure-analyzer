//! Coverage proof for issue #59 (schema-aware accepting-walk generation):
//! `schema_walk_completeness.rs` pins the generator's API shape and
//! end-to-end soundness; this file is the promised follow-up — "record
//! stable coverage of ... frames, ... tokenization shapes, and output
//! constructs; fail on uncovered shipped rules."
//!
//! The schema's internal `L2Position`/rule classification is private (the L2
//! overlay's scoping boundary), so — like `schema_walk_completeness.rs`'s own
//! `schema_narrowing_spans_more_than_one_pda_configuration` — this stays on
//! externally observable signals: the decoded walk text for *output
//! constructs*, and the public `Pda::stack_top()` for *frame* coverage.
//! Generation is fully deterministic (`schema_walker.rs`'s fixed seed), so
//! there is no run-to-run flakiness in what these tests observe.
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
use purecard::{CompiledGrammar, DecoderSession};
use schema_walker::generate_schema_walks;

/// The structural bytes offered as their own single-byte token, in addition to
/// a db's gold-query lexemes, so a walk can explore beyond verbatim replay —
/// the same set `schema_walk_completeness.rs` uses, kept as its own copy per
/// `fixture_dbs.rs`'s documented house convention (never partially reuse a
/// support module's neighbor).
const STRUCTURAL_BYTES: &[u8] = b"abXY1_ |{}()[].,;:$%'-><=!&+*/";

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

/// Decode a walk's token ids to its raw byte text via `grammar`'s vocabulary.
fn decode(grammar: &CompiledGrammar, walk: &[u32]) -> Vec<u8> {
    let mut text = Vec::new();
    for &id in walk {
        text.extend_from_slice(
            grammar
                .vocab()
                .bytes(id)
                .expect("a walk's own token is always a real vocab entry"),
        );
    }
    text
}

/// One representative, unambiguous shipped construct to require somewhere in
/// the generated corpus: a name plus a list of candidate substrings, any one
/// of which counts as the construct appearing (`docs/spec/schema.md` §6.5/6.6
/// rule families this stands in for are noted per entry).
struct Construct {
    name: &'static str,
    any_of: &'static [&'static str],
}

/// Deliberately a *subset* of every §6.5/6.6 rule position, not all of
/// them — each entry here is a substring that can only mean the construct it
/// names (no ambiguity with an identifier or classpath byte run), so a hit is
/// a real signal and a miss is a real gap. Purely lexical constructs that
/// need a proper tokenizer to disambiguate from an identifier run (bare
/// numeric/date literals) are left to a future, more precise pass rather than
/// risked here as a substring check.
const CONSTRUCTS: &[Construct] = &[
    Construct {
        name: "arrow step (pipeline `->`)",
        any_of: &["->"],
    },
    Construct {
        name: "dot navigation (N1/N2 property access)",
        any_of: &["."],
    },
    Construct {
        name: "comparison operator (T1/T2)",
        any_of: &["==", "!=", "<=", ">=", "<", ">"],
    },
    Construct {
        name: "collapse operator (T6 `->toOne()`)",
        any_of: &["toOne("],
    },
    Construct {
        name: "aggregation reducer (T3)",
        any_of: &["sum(", "count(", "average(", "min(", "max("],
    },
    Construct {
        name: "relation-shaping call (N6 scope-establishing)",
        any_of: &["project(", "groupBy(", "restrict(", "olapGroupBy("],
    },
    Construct {
        name: "quoted string literal",
        any_of: &["'"],
    },
    Construct {
        name: "bound variable reference (binder scope)",
        any_of: &["$"],
    },
    Construct {
        name: "date/milestone literal (`%`-prefixed)",
        any_of: &["%"],
    },
];

/// Every [`Construct`] appears at least once across the whole generated
/// corpus (every fixture schema's [`generate_schema_walks`] output) —
/// #59's "output constructs" coverage bullet. A miss here means the
/// generator's own bias (`ACCEPT_BONUS`, `GROW_MIN`/`GROW_MAX`, candidate
/// weighting) is steering walks away from an entire construct family, not
/// that the grammar/schema can't produce one.
#[test]
fn every_representative_output_construct_appears_at_least_once() {
    let mut corpus_text: Vec<u8> = Vec::new();
    for db_id in FIXTURE_DBS {
        let (grammar, schema) = grammar_and_schema(db_id);
        let walks = generate_schema_walks(&grammar, &schema);
        for walk in &walks {
            corpus_text.extend_from_slice(&decode(&grammar, walk));
            corpus_text.push(b'\n');
        }
    }
    let corpus_text = String::from_utf8(corpus_text)
        .expect("every vocab token is valid UTF-8 (corpus queries and structural bytes are ASCII)");

    let missing: Vec<&str> = CONSTRUCTS
        .iter()
        .filter(|construct| {
            !construct
                .any_of
                .iter()
                .any(|needle| corpus_text.contains(needle))
        })
        .map(|construct| construct.name)
        .collect();
    assert!(
        missing.is_empty(),
        "generated corpus never produced: {missing:?} — coverage regression, not a flake \
         (generation is deterministic)"
    );
}

/// Every [`Frame`](purecard::grammar::pda::Frame) kind is pushed at least
/// once across the whole generated corpus — #59's "frames" coverage bullet.
/// Walked by replaying each generated walk through a fresh `DecoderSession`
/// and sampling `pda().stack_top()` after every accepted token, mirroring
/// `schema_walk_completeness.rs`'s `schema_narrowing_spans_more_than_one_pda_configuration`
/// (same public API, different signal: every frame kind ever open, not only
/// the ones active when L2 narrows).
#[test]
fn every_frame_kind_is_pushed_at_least_once() {
    let mut seen: HashSet<String> = HashSet::new();
    for db_id in FIXTURE_DBS {
        let (grammar, schema) = grammar_and_schema(db_id);
        let walks = generate_schema_walks(&grammar, &schema);
        for walk in &walks {
            let mut session = DecoderSession::with_schema(&grammar, schema.clone())
                .expect("grammar is fixed-engine");
            for &id in walk {
                session
                    .accept_token(id)
                    .expect("a walk's own token is always admissible");
                if let Some(frame) = session
                    .pda()
                    .expect("fixed-engine grammar always exposes its Pda")
                    .stack_top()
                {
                    seen.insert(format!("{frame:?}"));
                }
            }
        }
    }
    assert!(
        seen.len() > 1,
        "generated corpus only ever pushed {} distinct frame kind(s): {seen:?} — \
         expected more than one of Paren/Bracket/Brace/BraceLambda to appear",
        seen.len()
    );
}
