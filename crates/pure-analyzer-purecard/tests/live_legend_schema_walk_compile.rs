//! Live Legend compile-rate proof for issue #55: send schema-aware
//! accepting walks (issue #59's generator) through the real engine's
//! two-call compile sequence — `grammarToJson/lambda` then
//! `lambdaReturnType` — instead of the placeholder lambda/model fixtures.
//!
//! Opt-in (`#[cfg(feature = "legend")]`): requires the pinned Legend stack
//! (`just test-legend` brings it up and tears it down).
#![cfg(feature = "legend")]
#![forbid(unsafe_code)]

use std::collections::BTreeMap;
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
use serde_json::Value;

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

fn seed_corpus_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("corpus/modern_dialect_seeds.jsonl")
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

/// The live attestation that retires §6.6 **T7** (issue #116): a
/// `project`/`groupBy` column/key lambda whose body is left at a **class** or a
/// **to-many collection** compiles against the pinned stack.
///
/// T7 proposed masking such a body's own closing delimiter. The premise is
/// false: `project`'s column lambda is declared over `Any`. §6.6 T7 carries the
/// full probe table and the reasoning; this is the executable half of it.
///
/// Pinned here rather than quoted in a PR description for the same reason
/// `every_fixture_gold_corpus_compiles_against_its_assembled_store_grammar`
/// is: an evidence claim that only ever ran once is not a gate. If a future
/// engine pin ever *does* reject one of these, this reddens and T7 can be
/// reopened on real evidence — which is exactly the trigger it always needed.
/// Its L2-side twin,
/// `l2_precision::t7_keeps_a_projection_lambda_closer_on_a_class_typed_or_to_many_body`,
/// runs without the stack and reddens if a T7 rule is ever implemented.
#[test]
fn a_non_scalar_projection_lambda_body_compiles_so_t7_stays_retired() {
    let client = LegendClient::new(ENGINE_BASE);
    client
        .health_wait(HEALTH_TIMEOUT)
        .expect("Legend engine must become healthy");

    const WORLD_COUNTRY: &str = "spider::world_1::model::default::Country";
    const WORLD_CITY: &str = "spider::world_1::model::default::City";
    const CAR_CONTINENTS: &str = "spider::car_1::model::default::Continents";
    const CAR_MODEL_LIST: &str = "spider::car_1::model::default::ModelList";

    // Both constants are the engine's own fully-qualified spelling, read off a
    // live run of these exact probes against the pinned stack — not the short
    // names the §6.6 T7 evidence table abbreviates to. This lane is
    // schedule/dispatch-only (`purecard-legend.yml`), so a wrong string here
    // would redden a nightly rather than a PR; they were re-verified live after
    // the assertions were tightened from `ReturnType(_)`.
    /// The arm-A projection return type, asserted rather than merely "some
    /// type" so a semantic drift is caught as loudly as an outright rejection.
    const TDS: &str = "meta::pure::tds::TabularDataSet";
    /// The arm-R (`~[…]`) projection return type, for the same reason.
    const RELATION: &str = "meta::pure::metamodel::relation::Relation";

    let probes: Vec<(&str, String, &str)> = vec![
        // Arm-A `project`, to-many class-typed association end (`[1..*]`).
        (
            CRITERION_DB,
            format!("|{WORLD_COUNTRY}.all()->project([x|$x.fk1DefaultCountrylanguage], ['col'])"),
            TDS,
        ),
        // Arm-A `project`, to-*one* class-typed association end (`[1]`).
        (
            CRITERION_DB,
            format!("|{WORLD_CITY}.all()->project([x|$x.fk0DefaultCountry], ['col'])"),
            TDS,
        ),
        // Arm-R `project(~[…])`, the relation-API spelling of the same position.
        (
            CRITERION_DB,
            format!("|{WORLD_COUNTRY}.all()->project(~[col: x|$x.fk1DefaultCountrylanguage])"),
            RELATION,
        ),
        // `groupBy`'s key lambda, class-typed body.
        (
            CRITERION_DB,
            format!(
                "|{WORLD_COUNTRY}.all()->groupBy([x|$x.fk1DefaultCountrylanguage], \
                 [agg(x|$x.gnp, y|$y->sum())], ['g','s'])"
            ),
            TDS,
        ),
        // The body left at the bare bound instance — the extreme of the same
        // claim: not even a navigation, just the class itself.
        (
            CRITERION_DB,
            format!("|{WORLD_COUNTRY}.all()->project([x|$x], ['col'])"),
            TDS,
        ),
        // The spec's own literal T7 counter-example, on the generalization DB.
        (
            GENERALIZATION_DB,
            format!("|{CAR_CONTINENTS}.all()->project([x|$x.fk0DefaultCountries], ['col'])"),
            TDS,
        ),
        // T7's other arm: a primitive mapped over a to-many step, so the body
        // is a `String[*]` collection rather than a class.
        (
            GENERALIZATION_DB,
            format!(
                "|{CAR_CONTINENTS}.all()->project([x|$x.fk0DefaultCountries.countryName], ['col'])"
            ),
            TDS,
        ),
        // A to-many/to-many association, the loosest multiplicity available in
        // any committed fixture.
        (
            GENERALIZATION_DB,
            format!("|{CAR_MODEL_LIST}.all()->project([x|$x.fk3DefaultCarNames], ['col'])"),
            TDS,
        ),
        // `agg`'s own map lambda, class-typed body.
        (
            GENERALIZATION_DB,
            format!(
                "|{CAR_CONTINENTS}.all()->groupBy([x|$x.continent], \
                 [agg(x|$x.fk0DefaultCountries, y|$y->count())], ['g','s'])"
            ),
            TDS,
        ),
    ];

    // Assembled per database, not per probe: the model text is identical for
    // every probe on a database, and parsing it is a live network round-trip.
    let mut models: BTreeMap<&str, Value> = BTreeMap::new();
    let mut failures: Vec<String> = Vec::new();
    for (db_id, text, expected) in &probes {
        let pmcd = models.entry(db_id).or_insert_with(|| {
            client
                .grammar_to_json_model(&full_model_text(db_id))
                .unwrap_or_else(|err| panic!("{db_id}: assembled model must parse: {err}"))
        });
        match client.grammar_to_json_lambda(text) {
            Err(err) => failures.push(format!("{db_id}: PARSE: {err}\n  {text}")),
            Ok(lambda_json) => match client
                .lambda_return_type(&lambda_json, pmcd)
                .unwrap_or_else(|err| panic!("{db_id} request failed: {err}"))
            {
                ReturnTypeOutcome::ReturnType(actual) if actual == *expected => {}
                ReturnTypeOutcome::ReturnType(actual) => failures.push(format!(
                    "{db_id}: returned {actual}, expected {expected}\n  {text}"
                )),
                ReturnTypeOutcome::CompileError(message) => {
                    failures.push(format!("{db_id}: COMPILE: {message}\n  {text}"));
                }
            },
        }
    }
    assert!(
        failures.is_empty(),
        "{}/{} non-scalar projection-lambda bodies failed to compile as expected \
         — T7's premise may have become true under a new engine pin:\n{}",
        failures.len(),
        probes.len(),
        failures.join("\n\n")
    );
}

/// Every seed in the modern-dialect corpus **parses against the pinned engine**.
///
/// The seed corpus (ADR-0007) is a soundness oracle: `modern_dialect_soundness`
/// asserts L1 accepts every row, and `docs/spec/grammar.md` §5 makes those rows
/// the *motivation* for the productions L1 grew to admit them. That only holds
/// if the rows are real Legend Pure — and until issue #55 Phase 7, two of them
/// were not. `gap-report/g2-latest:4` claimed `Class.all(%latestdate)` and
/// `:5` claimed `%latest` as a comparison operand; the pinned engine rejects
/// both outright ("no viable alternative at input '.all(%latestdate'"), and a
/// grammar widened to admit them widened past the language. No gate could see
/// it: L1 accepting more than the engine is exactly §5.10's documented
/// over-approximation, so the soundness lane stayed green while the oracle was
/// wrong.
///
/// This is that missing gate, and it is the constitution §5 half of the fix (the
/// two rows themselves are corrected to live-attested shapes, `issue-55/…`).
/// A **parse**, not a compile: the seeds name `finos::trade::Trade` and other
/// elements no fixture model defines, so `grammarToJson/lambda` is the whole of
/// what the engine can adjudicate about them — and it is exactly the layer L1
/// transcribes.
#[test]
fn every_modern_dialect_seed_parses_against_the_pinned_engine() {
    let client = LegendClient::new(ENGINE_BASE);
    client
        .health_wait(HEALTH_TIMEOUT)
        .expect("Legend engine must become healthy");

    let mut total = 0usize;
    let mut failures: Vec<String> = Vec::new();
    for record in load_gold(&seed_corpus_path()).expect("open the modern-dialect seed corpus") {
        let record = record.expect("the modern-dialect seed corpus parses");
        total += 1;
        if let Err(err) = client.grammar_to_json_lambda(&record.pure_text) {
            failures.push(format!(
                "seed {}: PARSE: {err}\n  {}",
                record.source_id, record.pure_text
            ));
        }
    }
    assert!(
        total > 0,
        "the modern-dialect seed corpus must not be empty"
    );
    assert!(
        failures.is_empty(),
        "{}/{total} modern-dialect seeds are not real Legend Pure — a production \
         seeded by one of these is motivated by a string the engine cannot parse:\n{}",
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
///
/// **Open question, deliberately not answered here (issue #55 Phase 9).** This
/// slack was sized against *recipe* PRs. An L1 production change reshuffles the
/// exploration stream end to end, and Phase 9 measured that class of noise
/// directly: two implementations accepting the **byte-identical language** — they
/// differ only in which byte an already-dead path dies at, which cannot change
/// any accept/reject verdict — moved `car_1` by −8 and by −2, and `world_1` by
/// +5 and by −1. The observed spread is wider than 3, so this constant is
/// undersized for L1 work specifically. Raising it repo-wide is a re-scope, not
/// a phase edit: a larger slack weakens the guard for *every* change, including
/// the over-masking regressions it exists to catch. Left where it is, and raised
/// as a scope question on the issue instead.
const RATCHET_SLACK: usize = 3;

/// Issue #55's criterion baseline, measured live against the pinned Legend
/// stack (`corpus/legend-stack/docker-compose.yml`): **39/64 total = recipe
/// 5/5 + exploration 34/59** (Phase 3, 2026-08-29 — up from Phase 2's
/// 17/64 = 5/5 + 12/59). Two consecutive runs were bit-identical.
///
/// Phase 3 ships N3c, the pipeline-source continuation rule. A re-taxonomy of
/// Phase 2's 47 exploration failures found 34 of them (72%) sharing one shape:
/// the walker arrowed a method straight off a **bare class path**, never
/// emitting the `.all()` that turns the `Class<T>[1]` metatype into the `T[*]`
/// extent — so every such call mismatched its signature by construction. A
/// further 7 misused the store path the mirror way (`Db.all()`). Neither shape
/// occurs even once in the 5034-query gold corpus. Masking both moved this
/// number for the first time since Phase 1.
///
/// Recorded honestly: the count is **net of five walks that previously
/// compiled and no longer can** (two here, three on the generalization guard) —
/// `->pair('US Territory')`, `->limit(1930)` and friends, which only ever
/// type-checked because a loose builtin signature accepts a `Class<T>[1]` and
/// hands it straight back. Losing a compile that worked only because the
/// receiver type was wrong is the precision win, not a regression; they are
/// frozen as must-be-masked fixtures in `l2_precision.rs`.
///
/// `world_1`'s corpus-derived vocabulary realizes only five of the six recipe
/// shapes the eager generator offers; `recipe_walks` drops an unrealizable
/// shape rather than padding it, so the recipe partition is five walks wide
/// here and the exploration partition inherits the freed slot.
///
/// **Phase 4 re-measure (2026-08-29): 38/64 = recipe 5/5 + exploration 33/59**,
/// bit-identical across two consecutive runs. The baseline below is *not*
/// lowered to it — floors ratchet upward only (constitution §3/§7) — and the
/// floor stays 34 − [`RATCHET_SLACK`] = 31, which the re-measure clears.
///
/// Recorded honestly, because the headline number is not where Phase 4's work
/// landed: the phase halved the L1 **parse**-failure residue on both databases
/// (`world_1` 15 → 7, `car_1` 20 → 10 of their exploration failures), and the
/// probability mass that freed up refilled with builtin-signature and
/// multiplicity type errors (`world_1` 10 → 19, `car_1` 4 → 16) — the original
/// taxonomy's buckets D and E, which no L1 tightening can reach and which this
/// phase did not target. The exploration stream reshuffles end to end on every
/// mask change, so a closed failure class frees a slot for a fresh draw rather
/// than converting to a compile; the count moved −1 / −2 while the failure
/// *set* changed shape substantially.
///
/// **Phase 5 (2026-08-29) ratchets this to 45/64 = recipe 5/5 + exploration
/// 40/59**, bit-identical across two consecutive runs — the largest single move
/// since Phase 3, and the first time in four phases the headline number tracked
/// the taxonomy. N3f, the extent's receiver-category rule, masks the method
/// names whose every overload demands a relation or primitive-scalar receiver a
/// `T[*]` class extent can never present. Bucket D — the wrong-signature call on
/// an extent, and the largest residual bucket on both databases after Phase 4 —
/// went **5 → 1** here and **3 → 1** on the generalization guard, live-verified
/// per walk; the survivors are an arity failure (`isEmpty('…')`, whose name is
/// legal niladic) and a `groupBy` argument-shape failure, neither of which a
/// receiver-category rule reaches.
///
/// **Phase 7 (2026-08-29) ratchets this to 50/64 = recipe 5/5 + exploration
/// 45/59**, bit-identical across two consecutive runs. The phase is four L1
/// tightenings, each live-attested: the symbolic milestoning literal is now the
/// `%latest` keyword rather than any `%<lowercase>+` run; a numeric date literal
/// must open on a digit, not on a `-`/`T`/`:` separator; a binder colon's
/// right-hand side is a classpath, then its `[mult]`, then exactly one pipe; and
/// a `[` no longer follows an arbitrary name, only a binder's type — the arm
/// that admitted it was left dead by the third change and CI's mutation shard
/// caught it as an unkillable mutant.
///
/// Live **parse** failures went 15 → 13 across the two databases with both
/// named sub-shapes closed outright — zero `%<not-latest>` walks (was 5) and
/// zero wrong-continuation typed binders (was 4). The count moves by less than
/// the classes closed because the exploration stream refills a freed slot from
/// whatever bucket is next-largest rather than converting it to a compile.
///
/// **Phase 6 (2026-08-29) ratcheted this to 49/64 = recipe 5/5 + exploration
/// 44/59**, bit-identical across two consecutive runs. Four rules land together,
/// all of them answers to "what may follow a term whose type is already
/// decided": N3g (a receiver-only builtin's arrow call takes no argument), N4a
/// (no operator applies to a store method's `Table[1]` result), N4b (a `&&`/`||`
/// operand cannot be a mismatched literal) and N4c (`-`/`*`/`/` cannot take a
/// string literal as their left operand). Bucket E — operator and multiplicity
/// type errors, the largest bucket on both databases after Phase 5 — went
/// **9 → 2** across the two databases, with both sub-shapes the phase targeted
/// closed outright: zero operator-on-a-store-result failures remain (was 5) and
/// zero string-literal-arithmetic ones (was 3). Bucket D's arity half is closed
/// too (`->isEmpty('…')` is gone; no receiver-only builtin is called with an
/// argument on either database).
///
/// **Issue #116 T6 re-measure (2026-08-29): unchanged at 50/64 = recipe 5/5 +
/// exploration 45/59**, measured on the same stack with the T6 commits' own
/// sources checked out to their parent and back. T6 is a soundness/precision
/// rule, not a walk-count push: it clears four tokens at one anchor, so it
/// removes illegal walks from the *admissible* set rather than converting a
/// failing walk into a compiling one. The generalization guard did not move
/// either — see [`GENERALIZATION_BASELINE`].
///
/// **T4 re-measure (2026-08-29, #116): 50/64 = recipe 5/5 + exploration 45/59**,
/// bit-identical across two consecutive runs — identical to Phase 7 and T6 on
/// both partitions. `world_1`'s corpus-derived vocabulary has no `toOne` token, so
/// the new `recipe_collapsed_navigation_predicate` finds nothing to build here
/// and the recipe partition stays five walks wide; T4's own position (a `->` on
/// a receiver the overlay has typed) is one this database's exploration stream
/// does not reach, so no movement was expected or forced. Reported because the
/// standing regime measures both arms on every rule, not only the ones a rule
/// was designed to move.
///
/// **Phase 8 (2026-08-29) re-measures at 49/64 = recipe 5/5 + exploration
/// 44/59**, bit-identical across two consecutive runs — one below the standing
/// record, so this baseline is *not* raised (floors ratchet upward only) and not
/// lowered either. Four L1 tightenings land, each a case of L1 spelling a
/// construct exactly as the pinned engine does: a date literal's fractional
/// seconds belong to its time half and it ends on a digit; a `(` at a value
/// position opens a parenthesised *group*, which has no `,` to separate; a lambda
/// binder pipe binds to a name; and a binder type that has taken a `::` owes its
/// multiplicity. Live parse failures across both databases fell **13 → 9**, and
/// the guard arm — where this phase's gain landed — moved 48/64 → 50/64.
///
/// **Phase 9 (2026-08-30) ratchets this to 54/64 = recipe 5/5 + exploration
/// 49/59**, bit-identical across two consecutive runs, re-measured on this
/// branch against its own before-run (49/64 = 5/5 + 44/59) on the same stack —
/// a new record and the largest single move since Phase 5. The phase is the one
/// L1 tightening Phase 8 wrote, attested and escalated rather than merged: a
/// `::` binds to a term-start name or a string literal, and to nothing else.
///
/// **Phase 10 (2026-08-31) re-measures at 54/64 = recipe 5/5 + exploration
/// 49/59**, bit-identical across two consecutive runs and level with Phase 9 on
/// both partitions, so this baseline is *not* raised. The phase ships N3h, whose
/// two closed classes (`tableToTDS(String[1])` and `restrict(Boolean[1],…)`)
/// both sat in `car_1`'s residue and neither in this one — this arm's
/// exploration stream reaches no relation method on a scalar receiver at all.
/// Reported because the standing regime measures both arms on every rule, not
/// only the one a rule was designed to move; the failure *set* here is
/// unchanged in kind, ten failures across the same six categories.
const CRITERION_BASELINE: Baseline = Baseline {
    db_id: CRITERION_DB,
    recipe_compiled: 5,
    exploration_compiled: 49,
};

/// The generalization guard's baseline, measured in the same run: **40/64
/// total = recipe 6/6 + exploration 34/58** (Phase 3, up from Phase 2's 23/64
/// = 6/6 + 17/58 — **+17**). N3c was designed against `world_1`'s re-taxonomy
/// and `car_1` carried the identical disease in the same proportions (28 of its
/// 41 exploration failures were the bare-class arrow, 7 the store misuse), so
/// this arm moving by nearly as much as the criterion is the generalization
/// evidence, not a coincidence. `car_1` realizes all six eager recipe shapes,
/// so its partitions split one slot differently from the criterion's — which is
/// exactly why each floor is stated per database rather than as a single
/// cross-database number.
///
/// **Phase 4 re-measure (2026-08-29): 38/64 = recipe 6/6 + exploration 32/58**,
/// bit-identical across two consecutive runs, clearing the 34 − 3 = 31 floor.
/// Not lowered, for the reason [`CRITERION_BASELINE`] states; the parse-failure
/// halving it records is, if anything, larger here.
///
/// **Phase 5 (2026-08-29) ratchets this to 41/64 = recipe 6/6 + exploration
/// 35/58**, bit-identical across two consecutive runs. N3f was built from an
/// engine-attested name set rather than from `world_1`'s taxonomy, so it is not
/// a rule that *could* be tuned to one database — and the guard moves with the
/// criterion (bucket D 3 → 1 here). It moves less in raw count because `car_1`'s
/// residue is more heavily parse- and operator-shaped to begin with.
///
/// **Phase 7 (2026-08-29) re-measures at 48/64 = recipe 6/6 + exploration
/// 42/58**, bit-identical across two consecutive runs — level with Phase 6, so
/// this baseline is *not* raised (floors ratchet upward only, and a re-measure
/// that does not beat the record does not move it).
///
/// Recorded honestly, because the guard is where the phase's one regression
/// landed. The first three tightenings put this arm at 49/64; removing the dead
/// `AfterName` `[` arm the third had orphaned — a real L1 over-acceptance, since
/// Legend has no positional index and answers "Bracket operation is not
/// supported" — cost it back. A dead arm is not kept to protect a number
/// (constitution §4/§7); the criterion arm held its +1 and the parse-failure
/// count fell on both.
///
/// **Phase 6 (2026-08-29) ratcheted this to 48/64 = recipe 6/6 + exploration
/// 42/58**, bit-identical across two consecutive runs. Every rule in the phase
/// was built from an engine-printed overload set and a three-corpus frequency
/// check rather than from either database's taxonomy, and the guard moves by
/// **+7** against the criterion's +4 — the arm that is not the design target
/// moving further is the generalization evidence.
///
/// **Issue #116 T6 re-measure (2026-08-29): unchanged at 48/64 = recipe 6/6 +
/// exploration 42/58**, measured on the same stack with the T6 commits' own
/// sources checked out to their parent and back, so this baseline is not
/// raised. Recorded because the honest number changed under the rule's feet:
/// against the *pre*-Phase-7 base T6 measured +1 here (42 → 43), and the note
/// claiming that ratchet was withdrawn once Phase 7 landed underneath and the
/// re-measure came back level. A one-walk move that a neighbouring phase
/// reshuffles away was exploration noise, not the rule's doing — which is what
/// the criterion arm holding still through both measurements already said.
///
/// **T4 re-measure (2026-08-29, #116): 48/64 = recipe 7/7 + exploration 41/57**,
/// bit-identical across two consecutive runs; the total is unchanged from T6's
/// own re-measure and one walk moves across the partition boundary. The recipe floor is
/// **ratcheted 6 → 7**: `car_1`'s vocabulary has the `toOne` token, so this
/// database realizes the new `recipe_collapsed_navigation_predicate`
/// (`CarsData.all()->filter(a|$a.cylinders->toOne() < 1)`, live-verified against
/// a real PMCD before it shipped) and that partition is exact and
/// deterministic — a compiling recipe walk becomes part of the floor. The
/// exploration partition gave up the slot the new recipe took (58 → 57) and
/// reshuffled; 41 clears its 42 − [`RATCHET_SLACK`] = 39 floor, which is
/// therefore left where Phase 7 set it and T6 confirmed.
///
/// **Phase 8 (2026-08-29) ratchets this to 50/64 = recipe 7/7 + exploration
/// 43/57**, bit-identical across two consecutive runs, measured over T4's own
/// partition split. None of the phase's four tightenings was designed against
/// either database's taxonomy — each was read off the pinned engine's answers to
/// a probe set on the branch — and the arm that is not the design target is the
/// one that moved (+2 on the total, +2 on the exploration record), which is the
/// generalization evidence.
///
/// **This shipment LOWERS this to 42/64 = recipe 7/7 + exploration 35/57 —
/// a maintainer-authorized lowering, not a ratchet.** Say it plainly: 35 is
/// eight below the 43 Phase 8 recorded, and the floor it sets (32) is eight
/// below the floor it replaces (40). The constitution (§3, §7) reserves that
/// move to a human: it is issue #55's "Decision 1", raised by Phase 8, posed
/// to the maintainer in the 2026-08-30 decision memo, and **explicitly
/// approved by the maintainer in the 2026-08-30 decision-ruling comment on
/// #55** (`issuecomment-5470222076`) — not inferred from context, not
/// assumed from a prior "continue", a direct answer to a direct question
/// putting this exact tradeoff to them. A prior attempt to ship this
/// (#153) asserted this authorization before it had actually been given;
/// that was false, #153 was reverted for exactly that reason (#157), and
/// this paragraph is the corrected record — cite the decision-ruling
/// comment, never a memo or a "continue" alone, as what actually
/// authorizes this number.
///
/// The evidence put to that decision, all of it re-derived live on this branch:
///
/// 1. **The rule is sound.** All 5,034 gold queries, both seed corpora, the
///    differential replay and `spec_equivalence` are green, so it rejects
///    nothing real. The shape it forbids has **zero** corpus support anywhere:
///    every `::` in the gold corpus (912 of them in `car_1` alone) is preceded
///    by an identifier byte. Thirteen hand-built probes agree with the pinned
///    4.113.0 engine both ways — `mpg::getInteger`, `meta::pure::tds::TDSRow`,
///    `'europe'::makeId` and `mpg ::getInteger` parse; a `::` off a `)`, a `]`,
///    a number, a date, a `$x`, a `.property` or a `->`-called name is each "no
///    viable alternative at input '…::'".
/// 2. **The −8 is reshuffle noise, and that was proven rather than asserted.** A
///    second implementation was built that accepts the **byte-identical**
///    language — it differs only in which byte an already-dead path dies at,
///    which cannot change any accept/reject verdict — and it *also* swung this
///    arm, by −2, while moving `world_1` by −1. A guard that moves when the
///    language does not is measuring its own sample, not the decoder.
/// 3. **The parse column, which does not resample, moves the right way.** Live
///    parse failures across both databases fell 9 → 7, and the class this rule
///    names went 3 → 0 here.
///
/// See [`RATCHET_SLACK`] for the systemic half of this, left open on purpose:
/// the slack that would have absorbed a swing of this size is a repo-wide
/// constant whose value is its own decision.
///
/// **Phase 10 (2026-08-31) ratchets this to 44/64 = recipe 7/7 + exploration
/// 37/57**, bit-identical across two consecutive runs, measured on this branch
/// against its own before-run (42/64 = 7/7 + 35/57) on the same stack. The floor
/// rises 32 → 34. The phase ships N3h, and the +2 is the two failure *classes*
/// it names leaving the residue outright: `tableToTDS(String[1])` and
/// `restrict(Boolean[1],String[1])` are both gone, and nothing that was
/// compiling stopped. `groupBy(String[1],String[1])` deliberately survives — it
/// is an arity error on a receiver-generic name, which N3h does not claim.
const GENERALIZATION_BASELINE: Baseline = Baseline {
    db_id: GENERALIZATION_DB,
    recipe_compiled: 7,
    exploration_compiled: 37,
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
/// against the same grammar this lane uses).
///
/// Phase 10 ships N3h: a relation/store builtin is dead at a `->` whose receiver
/// the overlay has typed a scalar primitive. What remains, across 10 `world_1` /
/// 20 `car_1` exploration failures, taxonomised per walk and summing to exactly
/// 30:
///
/// - **L1 parse over-approximation — 7.** Two are the arm-R carve-out (a `:`
///   that needs a slot-initial bare name, and a binder with no multiplicity),
///   which needs the `~` sigil tracked as *mutable per-frame* state — a
///   `Step`/`Action` the declarative spec (ADR-0010) has no V1 form for. One is
///   the milestoning slot's content, and four are argument-shape garbage with no
///   labelled token.
/// - **A bare word or `::` classpath in a value position — 7.** Unchanged in
///   kind (one fewer than Phase 9's 8, on the stream's own reshuffle rather than
///   on anything this phase did): closing them means narrowing a *name* against the
///   schema's own element set at a value position, the product question about how
///   much of a closed model the decoder may assume. **Declined by the maintainer**
///   in the 2026-08-30 decision-ruling comment on #55 (Decision 3), on the
///   ground that a real host's novel identifiers must stay legal. Permanent
///   residue, not a backlog item.
/// - **A receiver / signature one category over — 8.** N3h took two of the ten
///   Phase 9 recorded — `tableToTDS(String[1])` and
///   `restrict(Boolean[1],String[1])`, the whole scalar-receiver class. What is
///   left splits in two, and the split is what a Phase 11 would work from:
///   **arity on a receiver-generic name** (`groupBy(Countrylanguage[*],Boolean[1])`,
///   `groupBy(CarMakers[*],String[1])`, `limit(CarMakers[*],String[1])`,
///   `limit(ModelList[*],String[1])` — the extent-method argument *shape*, which
///   N3h explicitly does not claim), and **operator operands**
///   (`and(Integer[*],String[1])`, `lessThan(CarMakers[1],Integer[1])`,
///   `or(Boolean[1],LambdaFunction<…>[1])` and one more).
/// - **A corpus binder name called as a method — 1.** `row1(ModelList[*],
///   Integer[1])` — `row1` is a lambda binder name the gold corpus's join
///   lambdas bind 2378 times, not a schema column and not a builtin. Closing it
///   needs a *permit* set of legal builtin names at the extent-method position,
///   which §6.5 N3f rules out on its own evidence: eleven collection builtins
///   compile there and appear in no corpus, so any corpus-derived allow-list
///   over-masks. Filed with Decision 3's bucket in spirit — it is the same
///   closed-vocabulary question, at a method name instead of a value.
/// - **A property on a `Table` or on an inferred primitive — 4.** `Edispl_t1` and
///   `Year_T2_2` on `meta::relational::metamodel::relation::Table`, `fullName` on
///   a `DateTime`, `LifeExpectancy_T1` on a `String`. The `Table` two are **not**
///   the cheap rule they look like: the receiver type is statically known, but
///   `Table`'s own property set is rich and overlaps the schema's member
///   vocabulary — `.name`, `.schema`, `.columns`, `.primaryKey`, `.milestoning`,
///   `.temporaryTable`, `.setColumns`, `.elementOverride` and
///   `.classifierGenericType` all compile live, quoted or bare, and `name` is a
///   member name in five of the eight fixture schemas. Denying the schema's
///   member vocabulary there would mask a legal navigation; an allow-list would
///   have to enumerate a metamodel class the engine exposes no listing for.
///   These stay with #116's blocked type inference.
/// - **Bucket E's remainder — 1.** A `[*]` collection element where `[1]` is
///   required, under `greaterThanEqual`/`times`. Needs left-operand reasoning.
/// - **Two engine-internal rejections** — `RuntimeException: Not possible!` and
///   an `IndexOutOfBoundsException`. Neither states a candidate set, so neither
///   gives the overlay anything to encode. **Irreducible.**
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
