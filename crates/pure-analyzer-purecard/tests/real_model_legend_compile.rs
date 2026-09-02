//! Live Legend compile-rate proof for issue #58: compiles every completed,
//! real-model-generated constrained query
//! (`python/tests/test_real_model_inference.py`'s own output) through the
//! pinned engine, and separately measures return-type faithfulness against a
//! hand-authored gold reference per fixture database — issue #58's fourth
//! bullet: "Compile every constrained output through the pinned Legend stack
//! and measure compile success plus faithfulness/execution-equivalence
//! separately."
//!
//! Opt-in (`#[cfg(feature = "legend")]`) and further gated on its own input
//! file: requires `python/tests/test_real_model_inference.py` to have already
//! run (`just real-model-infer`) and the pinned Legend stack up. `just
//! test-real-model` (`cargo xtask test-real-model`) runs both, in order, with
//! guaranteed teardown — the same shape as `just test-legend`.
#![cfg(feature = "legend")]
#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

#[path = "support/legend.rs"]
mod legend;
#[path = "support/schema_context.rs"]
mod schema_context;

use legend::{LegendClient, ReturnTypeOutcome};
use schema_context::full_model_text;

const ENGINE_BASE: &str = "http://localhost:6300/api";
const HEALTH_TIMEOUT: Duration = Duration::from_secs(90);

/// One line of `target/purecard/real-model/generated_queries.jsonl`, written
/// by `write_generated_queries_jsonl` (`python/tests/support/real_model.py`):
/// one completed L1/L2 constrained generation, decoded to Pure text.
/// `gold_pure_text` is the *fixture's own* committed gold reference (from
/// `real_model_prompts.json`), carried through verbatim rather than
/// re-derived here — the gold text and the "arm-C `Class.all()`" shape it
/// happens to take are the fixture author's call, not this test's to assume.
#[derive(Debug, Clone, serde::Deserialize)]
struct GeneratedQuery {
    fixture_id: String,
    db_id: String,
    gold_pure_text: String,
    mode: String,
    query_text: String,
}

/// Path to the Python harness's JSONL output, supplied by the caller
/// (`just test-real-model`) rather than hardcoded — mirrors
/// `qwen_soundness.rs::load_tokenizer`'s `QWEN_TOKENIZER_JSON` convention: a
/// clear, actionable panic if unset, never a silent skip (issue #58 bullet 5).
fn generated_queries_path() -> PathBuf {
    let raw = std::env::var("PURECARD_REAL_MODEL_QUERIES").unwrap_or_else(|_| {
        panic!(
            "set PURECARD_REAL_MODEL_QUERIES to generated_queries.jsonl's path \
             (run via `just test-real-model`, which runs the Python harness first)"
        )
    });
    PathBuf::from(raw)
}

fn load_generated_queries(path: &PathBuf) -> Vec<GeneratedQuery> {
    let text = fs_read_to_string_or_panic(path);
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .enumerate()
        .map(|(index, line)| {
            serde_json::from_str(line)
                .unwrap_or_else(|err| panic!("{}:{}: {err}", path.display(), index + 1))
        })
        .collect()
}

fn fs_read_to_string_or_panic(path: &PathBuf) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|err| {
        panic!(
            "read {}: {err}. Run `just real-model-infer` first to produce it.",
            path.display()
        )
    })
}

/// Per-mode (`l1`/`l2`) compile/faithfulness tally, reported alongside the
/// assertion so a future investigation has the split without rerunning.
#[derive(Debug, Default)]
struct ModeTally {
    total: usize,
    compiled: usize,
    faithful: usize,
    failures: Vec<String>,
}

/// Every completed L1/L2 real-model generation compiles against its DB's
/// assembled store grammar, and separately, its `returnType` matches the
/// hand-authored gold reference's own `returnType` for that DB (issue #58
/// bullet 4). This is a **return-type faithfulness** metric, not full
/// execution-equivalence: `store_grammar.rs`'s own documented scope seeds no
/// data into the H2 store, so no query here ever executes against real rows,
/// only type-checks — execution-equivalence over real data is a distinct,
/// larger scope this proof does not claim.
///
/// Live-verified locally against the pinned model/fixtures before this was
/// written (see the PR description); the 100% floor is pinned as a regression
/// gate, matching `live_legend_schema_walk_compile.rs`'s own 269/269 precedent
/// — a future model/fixture change that regresses this is a real finding, not
/// noise to retune away. No CI run has re-executed it: the scheduled lane that
/// owns it (`.github/workflows/purecard-real-model.yml`) dispatches to a
/// self-hosted runner that is not registered, so the floor is a local
/// observation pinned for the next local run, not a continuously enforced gate.
#[test]
fn real_model_constrained_outputs_compile_and_are_return_type_faithful() {
    let path = generated_queries_path();
    let queries = load_generated_queries(&path);
    assert!(
        !queries.is_empty(),
        "{} has no generated queries — the harness produced nothing to compile",
        path.display()
    );

    let client = LegendClient::new(ENGINE_BASE);
    client
        .health_wait(HEALTH_TIMEOUT)
        .expect("Legend engine must become healthy");

    let mut pmcds: BTreeMap<String, serde_json::Value> = BTreeMap::new();
    let mut gold_return_types: BTreeMap<String, String> = BTreeMap::new();
    let mut tallies: BTreeMap<String, ModeTally> = BTreeMap::new();

    for query in &queries {
        let pmcd = pmcds.entry(query.db_id.clone()).or_insert_with(|| {
            client
                .grammar_to_json_model(&full_model_text(&query.db_id))
                .unwrap_or_else(|err| panic!("{}: assembled model must parse: {err}", query.db_id))
        });
        // Keyed by `fixture_id`, not `db_id`: two fixtures over the same
        // database can target different classes, and the gold reference must
        // match *this* fixture's own committed `gold_pure_text`, not merely
        // some other class that happens to share its database.
        let gold_return_type = gold_return_types
            .entry(query.fixture_id.clone())
            .or_insert_with(|| {
                let lambda = client
                    .grammar_to_json_lambda(&query.gold_pure_text)
                    .unwrap_or_else(|err| {
                        panic!("{}: gold reference must parse: {err}", query.fixture_id)
                    });
                match client
                    .lambda_return_type(&lambda, pmcd)
                    .unwrap_or_else(|err| {
                        panic!("{}: gold reference request failed: {err}", query.fixture_id)
                    }) {
                    ReturnTypeOutcome::ReturnType(return_type) => return_type,
                    ReturnTypeOutcome::CompileError(message) => {
                        panic!(
                            "{}: gold reference failed to compile: {message}",
                            query.fixture_id
                        )
                    }
                }
            })
            .clone();

        let tally = tallies.entry(query.mode.clone()).or_default();
        tally.total += 1;
        match client.grammar_to_json_lambda(&query.query_text) {
            Err(err) => tally.failures.push(format!(
                "{} ({}): PARSE: {err}\n  {}",
                query.fixture_id, query.mode, query.query_text
            )),
            Ok(lambda_json) => match client
                .lambda_return_type(&lambda_json, pmcd)
                .unwrap_or_else(|err| panic!("{} request failed: {err}", query.fixture_id))
            {
                ReturnTypeOutcome::ReturnType(return_type) => {
                    tally.compiled += 1;
                    if return_type == gold_return_type {
                        tally.faithful += 1;
                    } else {
                        tally.failures.push(format!(
                            "{} ({}): FAITHFULNESS: got returnType {return_type}, want {gold_return_type}",
                            query.fixture_id, query.mode
                        ));
                    }
                }
                ReturnTypeOutcome::CompileError(message) => tally.failures.push(format!(
                    "{} ({}): COMPILE: {message}\n  {}",
                    query.fixture_id, query.mode, query.query_text
                )),
            },
        }
    }

    for (mode, tally) in &tallies {
        eprintln!(
            "real-model {mode}: {}/{} compiled, {}/{} return-type-faithful",
            tally.compiled, tally.total, tally.faithful, tally.total
        );
    }

    let all_failures: Vec<&str> = tallies
        .values()
        .flat_map(|tally| tally.failures.iter())
        .map(String::as_str)
        .collect();
    assert!(
        all_failures.is_empty(),
        "{} real-model generated queries failed compile/faithfulness:\n{}",
        all_failures.len(),
        all_failures.join("\n\n")
    );
}
