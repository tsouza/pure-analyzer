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
use schema_walker::{WALK_COUNT, generate_first_complete_schema_walk_set};

const ENGINE_BASE: &str = "http://localhost:6300/api";
const HEALTH_TIMEOUT: Duration = Duration::from_secs(90);
const STRUCTURAL_BYTES: &[u8] = b"abXY1_ |{}()[].,;:$%'-><=!&+*/";

/// Issue #55's own criterion database: the schema its live-compile-rate
/// target names, and the one the decoder-rule series' failure taxonomy was
/// built from.
const CRITERION_DB: &str = "world_1";

/// The generalization-guard database (issue #55's Phase 0 scope ruling): a
/// second, independent fixture measured and floored alongside the criterion,
/// so a rule tuned tightly to [`CRITERION_DB`]'s taxonomy cannot pass as a
/// general precision win.
const GENERALIZATION_DB: &str = "car_1";

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

/// One walk partition's live tally: how many of its walks reached a real
/// `returnType` against the fixture's own PMCD.
#[derive(Default)]
struct Tally {
    compiled: usize,
    total: usize,
}

impl std::fmt::Display for Tally {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.compiled, self.total)
    }
}

/// A database's live compile outcome, split at the walk set's own recipe /
/// exploration boundary (`SchemaWalkSet::recipe_len`).
///
/// The split is the whole point of this lane's reporting (issue #55): recipe
/// walks are deterministic, oracle-verified constructs that compile *by
/// construction*, so counting one toward a decoder-rule's precision win would
/// be circular. Only the exploration partition is evidence about the mask.
struct CompileRate {
    recipe: Tally,
    exploration: Tally,
    failures: Vec<String>,
}

/// A database's recorded Phase 0 live baseline, and thus its floors.
///
/// The recipe partition is deterministic — the same schema and vocabulary
/// always build the same recipe walks — so its floor is the exact measured
/// count. The exploration partition is drawn from one chained SplitMix64
/// stream that *any* rule change reshuffles end to end, so its floor carries
/// [`RATCHET_SLACK`]: an exact pin would red this lane on reshuffle noise
/// alone, and since the constitution (§3) forbids the agent lowering a floor,
/// an exact pin is a designed deadlock.
///
/// PROTECTED, ratchet-only: raise a baseline at a phase boundary once a
/// re-measure beats it. Only a maintainer may lower one.
struct Baseline {
    db_id: &'static str,
    recipe_compiled: usize,
    exploration_compiled: usize,
}

/// Slack every exploration floor carries below its recorded baseline (issue
/// #55's plan, standing gate (e)) — sized to the reshuffle noise observed
/// across the recipe PRs that preceded this measurement.
const RATCHET_SLACK: usize = 3;

/// Issue #55's criterion baseline, measured live against the pinned Legend
/// stack (`corpus/legend-stack/docker-compose.yml`): **17/64 total = recipe
/// 5/5 + exploration 12/59** (Phase 2, 2026-08-29 — unchanged from Phase 1's
/// own 12/59, which had itself ratcheted up from Phase 0's 7/59).
///
/// Phase 2's rules (mask-aware completion, S1's must-call veto, N7) are
/// strictly more precise than Phase 1's and closed every bucket-B walk they
/// were attested against — and this number still did not move. Any rule change
/// re-rolls the whole chained-SplitMix64 exploration stream, and the
/// intermediate measurements taken across Phase 2 (12 → 14 → 14 → 12, at
/// monotonically increasing precision) say plainly that this count is
/// reshuffle-dominated on `world_1` at single-walk granularity. The floor is
/// the gate; the count is a measurement, not a score.
///
/// `world_1`'s corpus-derived vocabulary realizes only five of the six recipe
/// shapes the eager generator offers; `recipe_walks` drops an unrealizable
/// shape rather than padding it, so the recipe partition is five walks wide
/// here and the exploration partition inherits the freed slot.
const CRITERION_BASELINE: Baseline = Baseline {
    db_id: CRITERION_DB,
    recipe_compiled: 5,
    exploration_compiled: 12,
};

/// The generalization guard's baseline, measured in the same run: **23/64
/// total = recipe 6/6 + exploration 17/58** (Phase 2, up from Phase 1's 18/64
/// = 6/6 + 12/58 — **+5** on a database none of Phase 2's rules was authored
/// against, while the criterion they *were* authored against stayed flat; see
/// [`CRITERION_BASELINE`] on why that asymmetry is reshuffle, not
/// generalization failure). `car_1` realizes all six eager recipe shapes, so
/// its partitions split one slot differently from the criterion's — which is
/// exactly why each floor is stated per database rather than as a single
/// cross-database number.
const GENERALIZATION_BASELINE: Baseline = Baseline {
    db_id: GENERALIZATION_DB,
    recipe_compiled: 6,
    exploration_compiled: 17,
};

/// Decode a walk's token ids back to its Pure text through `grammar`'s own
/// vocabulary.
fn walk_text(grammar: &CompiledGrammar, walk: &[u32]) -> String {
    walk.iter()
        .flat_map(|&id| {
            grammar
                .vocab()
                .bytes(id)
                .expect("real token")
                .iter()
                .copied()
        })
        .map(char::from)
        .collect()
}

/// Send every schema-aware walk (issue #59's generator) for `db_id` through
/// the real engine's two-call compile sequence, tallying the recipe and
/// exploration partitions separately.
fn measure_compile_rate(client: &LegendClient, db_id: &str) -> CompileRate {
    let pmcd = client
        .grammar_to_json_model(&full_model_text(db_id))
        .unwrap_or_else(|err| panic!("{db_id}: the assembled model must itself parse: {err}"));

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

    let set = generate_first_complete_schema_walk_set(&grammar, &schema);
    assert_eq!(
        set.walks().len(),
        WALK_COUNT,
        "{db_id}: the walk set's size is the denominator every floor is stated against"
    );

    let mut rate = CompileRate {
        recipe: Tally::default(),
        exploration: Tally::default(),
        failures: Vec::new(),
    };
    for (index, walk) in set.walks().iter().enumerate() {
        let is_recipe = index < set.recipe_len();
        let partition = if is_recipe { "recipe" } else { "exploration" };
        let tally = if is_recipe {
            &mut rate.recipe
        } else {
            &mut rate.exploration
        };
        tally.total += 1;
        let text = walk_text(&grammar, walk);
        match client.grammar_to_json_lambda(&text) {
            Err(err) => rate.failures.push(format!(
                "{db_id} {partition} walk {index}: PARSE: {err}\n  {text}"
            )),
            Ok(lambda_json) => match client
                .lambda_return_type(&lambda_json, &pmcd)
                .unwrap_or_else(|err| panic!("walk {index} ({text:?}) request failed: {err}"))
            {
                ReturnTypeOutcome::ReturnType(_) => tally.compiled += 1,
                ReturnTypeOutcome::CompileError(message) => rate.failures.push(format!(
                    "{db_id} {partition} walk {index}: COMPILE: {message}\n  {text}"
                )),
            },
        }
    }
    rate
}

/// Measure `baseline`'s database live and hold it to its floors.
///
/// This lane runs only under the opt-in `just test-legend` (nightly
/// `purecard-legend.yml`, plus dispatch), never `just ci`/`just ci-full`, so
/// the floors here are the nightly regression gate for issue #55's
/// compile-rate work. The per-partition numbers are printed on every run,
/// pass or fail: a phase's re-measure is read straight out of this line.
fn assert_live_compile_rate(baseline: &Baseline) {
    let client = LegendClient::new(ENGINE_BASE);
    client
        .health_wait(HEALTH_TIMEOUT)
        .expect("Legend engine must become healthy");

    let db_id = baseline.db_id;
    let rate = measure_compile_rate(&client, db_id);
    let compiled = rate.recipe.compiled + rate.exploration.compiled;
    let exploration_floor = baseline.exploration_compiled.saturating_sub(RATCHET_SLACK);
    eprintln!(
        "issue-55 live compile rate [{db_id}]: total {compiled}/{WALK_COUNT} \
         | recipe {} (floor {}) | exploration {} (floor {exploration_floor})\n{}",
        rate.recipe,
        baseline.recipe_compiled,
        rate.exploration,
        rate.failures.join("\n\n")
    );

    assert!(
        rate.recipe.compiled >= baseline.recipe_compiled,
        "{db_id}: recipe partition regressed to {} against its exact, deterministic \
         baseline of {} — a recipe walk that stops compiling is a broken oracle-verified \
         construct, not reshuffle noise",
        rate.recipe,
        baseline.recipe_compiled
    );
    assert!(
        rate.exploration.compiled >= exploration_floor,
        "{db_id}: exploration partition fell to {} , below the floor of {exploration_floor} \
         (baseline {} − {RATCHET_SLACK} slack). Fix the rule or take a re-scope to the \
         maintainer; never lower this floor (constitution §3, §7)",
        rate.exploration,
        baseline.exploration_compiled
    );
}

/// Issue #55's criterion arm: `world_1`'s schema-aware walks against a real
/// PMCD, reported and floored per partition.
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
/// against the same grammar this lane uses). What dominates the exploration
/// partition's residue is instead two causes neither of which
/// `schema_walker.rs` can fix by itself:
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
    assert_live_compile_rate(&CRITERION_BASELINE);
}

/// The generalization-guard arm: the identical measurement on `car_1`, built
/// from `car_1`'s own gold-corpus vocabulary slice, its own committed schema
/// fixture, and its own assembled PMCD.
///
/// Issue #55's criterion names `world_1`, and every decoder rule in the
/// series is designed against `world_1`'s failure taxonomy. A second,
/// independent fixture measured the same way is what keeps that from becoming
/// overfitting: a rule that only closes `world_1`'s specific degenerate walks
/// leaves this arm flat, and one that over-masks reddens it outright.
#[test]
fn schema_aware_walks_compile_against_a_real_pmcd_for_the_generalization_guard() {
    assert_live_compile_rate(&GENERALIZATION_BASELINE);
}
