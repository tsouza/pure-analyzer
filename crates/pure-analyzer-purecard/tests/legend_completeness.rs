//! Engine-backed completeness lane — opt-in (`docs/spec/testing.md` §8.2, §14.4).
//!
//! Compiled and run ONLY under `--features legend` (via `just test-legend`)
//! against a live Legend stack. The entire compilation unit is absent from the
//! default build graph, so the hermetic `just ci` gate never collects it — no
//! ignore attribute, no runtime skip, no weakened assertion.
//!
//! The doc comment sits above `#![cfg(feature = "legend")]` deliberately: an
//! inner attribute cfg's out everything textually after it in the same file,
//! including a `//!` that came later — with the feature off (the default),
//! that left this crate root with no doc comment at all, tripping this
//! workspace's `#![deny(missing_docs)]` (inherited via `[lints] workspace =
//! true`, which purecard's own pre-migration lint setup didn't apply this
//! broadly to test binaries).
#![cfg(feature = "legend")]

use std::path::Path;
use std::time::Duration;

// The client + classifier live in the oracle harness (ADR-0003). Pull the module
// in as a crate-local sibling; `--features legend` compiles its `LegendClient`.
#[path = "support/legend.rs"]
mod legend;
// The seeded accepting-walk generator (T8). Shared with the hermetic
// `completeness_walks` self-test so the engine sees the same committed corpus.
#[path = "support/walker.rs"]
mod walker;

use legend::{LegendClient, ReturnTypeOutcome};
use walker::{WALK_COUNT, generate_walks};

/// The engine API base the pinned `corpus/legend-stack` exposes.
const ENGINE_BASE: &str = "http://localhost:6300/api";
/// Health-wait budget (compose sets the engine `start_period` to 60s).
const HEALTH_TIMEOUT: Duration = Duration::from_secs(120);

fn fixture(name: &str) -> serde_json::Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    let text = std::fs::read_to_string(&path).expect("read fixture");
    serde_json::from_str(&text).expect("fixture is valid json")
}

#[test]
fn engine_client_reaches_lambda_return_type_endpoint() {
    // ponytail: the committed fixtures are provisional placeholders, so the
    // placeholder lambda always fails to compile — the live stack returns HTTP
    // 500 with a non-empty error body, which the client classifies as a
    // `CompileError`. That is exactly the §10 done-criterion clause 2: we can
    // POST a query and *read a classified result* off a live engine. Asserting a
    // specific `ReturnType` remains future work, pending regeneration of the
    // fixtures from a real §14.2 grammarToJson -> lambdaReturnType roundtrip
    // (spec R4 — still open; the committed fixtures are placeholders).
    let client = LegendClient::new(ENGINE_BASE);
    client
        .health_wait(HEALTH_TIMEOUT)
        .expect("Legend engine healthy");
    let outcome = client
        .lambda_return_type(&fixture("lambda.json"), &fixture("model.json"))
        .expect("POST lambdaReturnType and read a classified result");
    let ReturnTypeOutcome::CompileError(message) = outcome else {
        panic!("placeholder fixtures must fail to compile, got: {outcome:?}");
    };
    assert!(
        !message.is_empty(),
        "a compile error must carry a diagnosable, non-empty body"
    );
}

/// The completeness (G3/T8) engine lane: every seeded PDA-accepting walk is driven
/// to the live engine and a classified result is read back for each.
///
/// What is asserted here (the currently-attainable half of the §10
/// done-criterion): the generated walk corpus is non-trivial and *every* walk
/// round-trips to a live engine and comes back classified — the walk→engine
/// plumbing is real and the generator feeds it.
///
/// What remains OUTSTANDING (documented, not silently skipped): the *100%
/// compile-rate* clause of G3. Of the two pieces it needs, the L2 schema overlay
/// now exists (M3), so a schema-constrained walker can name classes and
/// properties that resolve; the remaining gap is (1) the §14.2 `grammarToJson`
/// lowering that turns a raw emitted-Pure string into the engine's
/// `ValueSpecification` JSON (spec risk R4 — still open, the fixtures are still
/// placeholders) and a schema-constrained walk generator wired onto it. Until
/// that lands, a compile-rate assertion would be dishonest over the current
/// placeholder fixtures, so this lane asserts reachability + classification and
/// leaves the rate to that follow-on work.
#[test]
fn seeded_walks_round_trip_to_the_engine_and_are_classified() {
    let walks = generate_walks();
    assert_eq!(
        walks.len(),
        WALK_COUNT,
        "the generator must feed the engine a full walk set"
    );

    let client = LegendClient::new(ENGINE_BASE);
    client
        .health_wait(HEALTH_TIMEOUT)
        .expect("Legend engine healthy");
    let model = fixture("model.json");
    let lambda_template = fixture("lambda.json");

    for walk in &walks {
        let rendered = String::from_utf8_lossy(walk).into_owned();
        // Best-effort payload: the raw walk text carried on the placeholder lambda
        // envelope. To be replaced by a real `grammarToJson` lowering (R4, still
        // open); until then the engine classifies it (a compile error is expected
        // and fine — the point is that the round-trip and classification work).
        let mut lambda = lambda_template.clone();
        lambda["_purecardWalk"] = serde_json::Value::String(rendered.clone());
        let outcome = client
            .lambda_return_type(&lambda, &model)
            .unwrap_or_else(|err| panic!("walk {rendered:?} failed to reach the engine: {err}"));
        // A classified result — either arm — proves the plumbing; the compile-rate
        // clause is the follow-on work (R4 lowering + schema-constrained walker).
        match outcome {
            ReturnTypeOutcome::ReturnType(_) | ReturnTypeOutcome::CompileError(_) => {}
        }
    }
}
