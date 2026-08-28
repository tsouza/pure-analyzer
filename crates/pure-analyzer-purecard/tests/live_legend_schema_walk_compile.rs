//! Live Legend compile-rate proof for issue #55: send schema-aware
//! accepting walks (issue #59's generator) through the real engine's
//! two-call compile sequence — `grammarToJson/lambda` then
//! `lambdaReturnType` — instead of the placeholder lambda/model fixtures.
//!
//! Opt-in (`#[cfg(feature = "legend")]`): requires the pinned Legend stack
//! (`just test-legend` brings it up and tears it down).
#![cfg(feature = "legend")]
#![forbid(unsafe_code)]

use std::path::PathBuf;
use std::time::Duration;

#[path = "support/corpus.rs"]
mod corpus;
#[path = "support/error.rs"]
mod error;
#[path = "support/fixture_dbs.rs"]
mod fixture_dbs;
#[path = "support/l2.rs"]
mod l2;
#[path = "support/legend.rs"]
mod legend;
#[path = "support/lex.rs"]
mod lex;
#[path = "support/schema_context.rs"]
mod schema_context;

use corpus::load_gold;
use fixture_dbs::FIXTURE_DBS;
use l2::{TokenVocab, load_schema};
use legend::{LegendClient, ReturnTypeOutcome};
use purecard::CompiledGrammar;
use schema_context::{first_class_path, full_model_text, pure_model_text};
use schema_walker::generate_first_complete_schema_walks;

const ENGINE_BASE: &str = "http://localhost:6300/api";
const HEALTH_TIMEOUT: Duration = Duration::from_secs(90);
const STRUCTURAL_BYTES: &[u8] = b"abXY1_ |{}()[].,;:$%'-><=!&+*/";

fn corpus_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("corpus/gold_queries.jsonl")
}

/// The full, real, execution-verified gold corpus — arm-A
/// (`Db->tableReference(...)->tableToTDS()`) and arm-C (`Class.all()->...`)
/// alike, across all 8 `FIXTURE_DBS` (269 queries total) — compiles against
/// its DB's assembled store grammar. This closes the gap `PR #84` found and
/// `store_grammar.rs`'s own doc comment documents: the committed
/// schema-context Pure model text alone (Class/Association only) cannot
/// resolve arm-A's `tableReference`/`tableToTDS` calls, which need a real
/// `Database`/`Mapping`/`Connection`/`Runtime`. Live-verified 269/269 before
/// this test was written (see the PR description for the raw run); this pins
/// that result as a regression gate for this opt-in lane, not just a
/// diagnostic.
#[test]
fn every_fixture_gold_corpus_compiles_against_its_assembled_store_grammar() {
    let client = LegendClient::new(ENGINE_BASE);
    client
        .health_wait(HEALTH_TIMEOUT)
        .expect("Legend engine must become healthy");

    let mut total = 0usize;
    let mut failures: Vec<String> = Vec::new();
    for &db_id in FIXTURE_DBS {
        let pmcd = client
            .grammar_to_json_model(&full_model_text(db_id))
            .unwrap_or_else(|err| panic!("{db_id}: assembled model must parse: {err}"));
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
        total += queries.len();
        for (index, text) in queries.iter().enumerate() {
            match client.grammar_to_json_lambda(text) {
                Err(err) => failures.push(format!("{db_id} query {index}: PARSE: {err}\n  {text}")),
                Ok(lambda_json) => match client
                    .lambda_return_type(&lambda_json, &pmcd)
                    .unwrap_or_else(|err| panic!("{db_id} query {index} request failed: {err}"))
                {
                    ReturnTypeOutcome::ReturnType(_) => {}
                    ReturnTypeOutcome::CompileError(message) => failures.push(format!(
                        "{db_id} query {index}: COMPILE: {message}\n  {text}"
                    )),
                },
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{}/{total} gold queries failed to compile:\n{}",
        failures.len(),
        failures.join("\n\n")
    );
}

/// Every fixture DB's real, committed schema produces a genuinely compiling
/// `Class.all()` lambda against its own live-assembled PMCD — the documented
/// arm-C shape (`docs/spec/grammar.md` §5: "`|Class.all()->…`"), and the one
/// construct issue #55's own live-compile investigation confirmed reaches a
/// real `returnType`, not just a parse. This replaces the placeholder
/// lambda/model fixtures issue #55 calls out, end to end, for every fixture
/// schema, with zero live-workspace dependency (the PMCD is assembled from
/// each schema-context file's own committed Pure model text).
#[test]
fn every_fixture_class_all_lambda_compiles_against_its_own_pmcd() {
    let client = LegendClient::new(ENGINE_BASE);
    client
        .health_wait(HEALTH_TIMEOUT)
        .expect("Legend engine must become healthy");

    for &db_id in FIXTURE_DBS {
        let model_text = pure_model_text(db_id);
        let pmcd = client
            .grammar_to_json_model(&model_text)
            .unwrap_or_else(|err| panic!("{db_id}: committed Pure model must itself parse: {err}"));
        let class_path = first_class_path(db_id);
        let text = format!("|{class_path}.all()");
        let lambda_json = client
            .grammar_to_json_lambda(&text)
            .unwrap_or_else(|err| panic!("{db_id}: {text:?} failed to parse: {err}"));
        match client
            .lambda_return_type(&lambda_json, &pmcd)
            .unwrap_or_else(|err| panic!("{db_id}: {text:?} request failed: {err}"))
        {
            ReturnTypeOutcome::ReturnType(return_type) => assert_eq!(
                return_type, class_path,
                "{db_id}: {text:?} returned an unexpected type"
            ),
            ReturnTypeOutcome::CompileError(message) => {
                panic!("{db_id}: {text:?} failed to compile: {message}")
            }
        }
    }
}

/// Diagnostic, not a gate (this whole file only runs via the opt-in
/// `just test-legend`, never `just ci`/`just ci-full`): sends every
/// schema-aware walk (issue #59's generator) for `world_1` through the real
/// engine's two-call compile sequence and reports the parse/compile split.
///
/// Two real, walker-level bugs surfaced and were fixed this way (see
/// `schema_walker.rs`'s `PendingCall`/`would_fuse` docs: a `->name` call
/// missing its mandatory `()`, and two tokens silently fusing into one bogus
/// lexeme). What's left after those fixes is not a walker bug: replaying
/// against the now-assembled store grammar (this lane's model text includes
/// `store_grammar.rs`'s Database/Mapping/Connection/Runtime, closing the gap
/// the previous revision of this comment documented as separately blocking)
/// showed only 1/64 compiling at the time — the missing store grammar was
/// never the dominant cause here; `every_fixture_gold_corpus_compiles_against_its_assembled_store_grammar`
/// proves that gap is in fact closed (269/269 *real* gold queries compile
/// against the same grammar this diagnostic uses). What actually dominates
/// this walk set's failures, split into two causes neither of which
/// `schema_walker.rs` can fix by itself — the deterministic recipe walks
/// (issue #55) have since lifted the measured figure to 9/64, one walk per
/// recipe that realizes a real, compilable shape, leaving the residue below
/// unchanged in kind:
///
/// - ~1/3 fail to even *parse*: nested predicate/operator combinations
///   (`&&`, `||`, comparisons, arithmetic) that `docs/spec/grammar.md` §5.7
///   explicitly documents as "loosely typed... left to L2/compiler," so L1
///   admits shapes real Pure's parser doesn't accept. Closing this means
///   L1 modeling real operator/predicate arity, not a walker heuristic.
/// - ~2/3 parse but fail to *compile*, mostly "can't find property/element":
///   the walker draws `.`-navigated property names from the whole
///   cross-schema token vocabulary, unconstrained by which class is actually
///   being navigated (issue #56's remaining L2 scope), and separately, a
///   bare classpath followed directly by `.'prop'` (skipping the required
///   `.all()`) type-checks as a property lookup *on the class metatype*,
///   never a real one — confirmed live: `Class.all()` and
///   `Class.all()->filter(...)` both compile; a bare `Class.'name'` never can,
///   regardless of which name is chosen.
#[test]
fn schema_aware_walks_compile_against_a_real_pmcd() {
    let client = LegendClient::new(ENGINE_BASE);
    client
        .health_wait(HEALTH_TIMEOUT)
        .expect("Legend engine must become healthy");

    let db_id = "world_1";
    let pmcd = client
        .grammar_to_json_model(&full_model_text(db_id))
        .expect("the assembled model must itself parse");

    let extra: Vec<Vec<u8>> = STRUCTURAL_BYTES.iter().map(|&byte| vec![byte]).collect();
    let queries: Vec<String> = load_gold(&corpus_path())
        .expect("open the committed gold corpus")
        .filter_map(Result::ok)
        .filter(|record| record.db_id == db_id)
        .map(|record| record.pure_text)
        .collect();
    let refs: Vec<&str> = queries.iter().map(String::as_str).collect();
    let vocab = TokenVocab::build(&refs, &extra);
    let grammar = CompiledGrammar::compile(vocab.vocab());
    let schema = load_schema(db_id);

    let walks = generate_first_complete_schema_walks(&grammar, &schema);
    let mut compiled = 0usize;
    let mut parse_failures: Vec<(usize, String, String)> = Vec::new();
    let mut compile_failures: Vec<(usize, String, String)> = Vec::new();
    for (index, walk) in walks.iter().enumerate() {
        let text: String = walk
            .iter()
            .flat_map(|&id| {
                grammar
                    .vocab()
                    .bytes(id)
                    .expect("real token")
                    .iter()
                    .copied()
            })
            .map(char::from)
            .collect();
        match client.grammar_to_json_lambda(&text) {
            Err(err) => parse_failures.push((index, text, err.to_string())),
            Ok(lambda_json) => match client
                .lambda_return_type(&lambda_json, &pmcd)
                .unwrap_or_else(|err| panic!("walk {index} ({text:?}) request failed: {err}"))
            {
                ReturnTypeOutcome::ReturnType(_) => compiled += 1,
                ReturnTypeOutcome::CompileError(message) => {
                    compile_failures.push((index, text, message));
                }
            },
        }
    }

    eprintln!(
        "{compiled}/{} walks compiled for {db_id}\nparse_failures: {parse_failures:#?}\ncompile_failures: {compile_failures:#?}",
        walks.len()
    );
}
