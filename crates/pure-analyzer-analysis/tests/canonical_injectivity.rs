//! Proof suite for canonical-emission injectivity and round-trip stability
//! over a broad, generated slice of the supported normal-form subset
//! (issue #245).
//!
//! Three properties are proven over every case the generator in
//! `tests/support/canonical_strategy.rs` reaches within the supported
//! subset (an "emitted" [`CanonicalEmissionOutcome`]):
//!
//! 1. **Injectivity** — two normal forms with different
//!    [`pure_analyzer_analysis::EquivalenceKey`]s (the allocation-independent
//!    semantic identity `comparison.rs`/`canonical_emission.rs` already rely
//!    on) never emit identical canonical text. `fmt --canonical` would be
//!    lossy/ambiguous if they did.
//! 2. **Idempotence** — reparsing, relowering, and renormalizing emitted text
//!    reaches the identical text again (a fixed point).
//! 3. **Verdict preservation** — the original and round-tripped queries
//!    always compare [`ComparisonOutcome::Equivalent`], the generic form of
//!    "preserves the committed M4a result" for every case this generator can
//!    reach; `canonical_comparison_corpus.rs` companion-checks this directly
//!    against the *committed* M4a corpus.
//!
//! `injectivity_check_rejects_a_hand_crafted_collision` and
//! `verdict_preservation_check_rejects_a_contradictory_verdict` are the
//! negative-test half of issue #245's acceptance criteria: they prove the
//! checking machinery itself, not the emitter, by feeding it a synthetic
//! collision/contradiction it must reject.
#![allow(clippy::disallowed_methods)]

use std::collections::{HashMap, HashSet};

use proptest::strategy::{Strategy, ValueTree};
use proptest::test_runner::TestRunner;
use pure_analyzer_analysis::{
    AnalysisInput, CanonicalEmissionOutcome, ComparisonOutcome, NormalizationOutcome,
    NormalizedQuery, RelationalOutcome, RelationalQuery, compare_relational_queries,
    emit_canonical_normal_form, lower_m3_query, normalize_relational_query,
};
use pure_analyzer_diagnostics::FileId;
use pure_analyzer_model::ModelGraph;
use pure_analyzer_parser::parse_query;

#[path = "support/canonical_strategy.rs"]
mod canonical_strategy;

use canonical_strategy::{arbitrary_join_query_source, arbitrary_single_table_query, model};

/// Generated single-table draws per run. Large enough to reach a
/// meaningfully diverse slice of the generator's shape space (measured: see
/// this PR's own verification evidence) while keeping `just ci`'s workspace
/// test run fast — each case is a small in-memory parse/lower/normalize, not
/// an I/O- or engine-bound operation.
const SINGLE_TABLE_CASES: usize = 4_000;
/// The join family has far fewer independent shapes (two selector orders ×
/// two sort directions per key), so far fewer draws already saturate it.
const JOIN_CASES: usize = 400;
/// Floor on how many generated draws must land in the supported,
/// successfully-emitted subset — catches a generator or emitter regression
/// that silently collapses the reachable shape space instead of only
/// slightly narrowing it.
const MIN_SUPPORTED_CASES: usize = 3_000;
/// Floor on how many *distinct* semantic normal forms (by
/// [`pure_analyzer_analysis::EquivalenceKey`]) those supported cases must
/// resolve to — distinguishes genuine shape diversity from many draws
/// collapsing onto a handful of normal forms.
const MIN_DISTINCT_NORMAL_FORMS: usize = 1_000;

const SOURCE_FILE: u32 = 501;
const ROUND_TRIP_FILE: u32 = 502;

struct SupportedCase {
    source: String,
    equivalence_key: String,
    emitted: String,
}

/// Parse, lower, and normalize `source` against `model`; `None` covers both
/// an expected generator miss (a shape outside the supported core) and a
/// genuine parse/lowering rejection — neither is a bug in this generator,
/// which deliberately explores past the supported boundary rather than only
/// inside it (see the module doc of `canonical_strategy.rs`).
fn try_normalize(source: &str, model: &ModelGraph, file: u32) -> Option<NormalizedQuery> {
    let parsed = parse_query(source, FileId::new(file)).ok()?;
    if !parsed.diagnostics.is_empty() {
        return None;
    }
    let RelationalOutcome::Supported(query) = lower_m3_query(AnalysisInput::new(
        FileId::new(file),
        source,
        &parsed.green,
        &parsed.diagnostics,
        Some(model),
    )) else {
        return None;
    };
    match normalize_relational_query(&query) {
        NormalizationOutcome::Normalized(normalized) => Some(*normalized),
        NormalizationOutcome::Indecisive(_) => None,
    }
}

/// The pre-normalization lowered query, for `compare_relational_queries`
/// (which normalizes internally on each side).
fn lower_for_comparison(source: &str, model: &ModelGraph, file: u32) -> RelationalQuery {
    let parsed = parse_query(source, FileId::new(file)).expect("generated source must parse");
    assert!(
        parsed.diagnostics.is_empty(),
        "generated source must parse cleanly: {source}\n{:#?}",
        parsed.diagnostics
    );
    let RelationalOutcome::Supported(query) = lower_m3_query(AnalysisInput::new(
        FileId::new(file),
        source,
        &parsed.green,
        &parsed.diagnostics,
        Some(model),
    )) else {
        panic!("generated source must lower through the supported core: {source}");
    };
    *query
}

fn generate_supported_cases(
    strategy: impl Strategy<Value = String>,
    draws: usize,
    model: &ModelGraph,
) -> Vec<SupportedCase> {
    let mut runner = TestRunner::default();
    let mut cases = Vec::new();
    for _ in 0..draws {
        let source = strategy
            .new_tree(&mut runner)
            .expect("strategy must produce a value")
            .current();
        let Some(normalized) = try_normalize(&source, model, SOURCE_FILE) else {
            continue;
        };
        let CanonicalEmissionOutcome::Emitted(emitted) = emit_canonical_normal_form(&normalized)
        else {
            continue;
        };
        cases.push(SupportedCase {
            source,
            equivalence_key: normalized.equivalence_key().as_str().to_owned(),
            emitted: emitted.into_string(),
        });
    }
    cases
}

/// Assert that no two records with different `equivalence_key`s share the
/// same `emitted` text. `records` is `(equivalence_key, source, emitted)`.
fn assert_injective_emission<'record>(
    records: impl IntoIterator<Item = (&'record str, &'record str, &'record str)>,
) {
    let mut by_text: HashMap<&'record str, (&'record str, &'record str)> = HashMap::new();
    for (key, source, text) in records {
        match by_text.get(text) {
            Some((seen_key, seen_source)) => assert_eq!(
                *seen_key, key,
                "canonical emission is not injective: {seen_source:?} (key {seen_key:?}) and \
                 {source:?} (key {key:?}) both emit {text:?}"
            ),
            None => {
                by_text.insert(text, (key, source));
            }
        }
    }
}

#[test]
fn generated_supported_normal_forms_are_injective_idempotent_and_verdict_preserving() {
    let model = model();
    let mut cases =
        generate_supported_cases(arbitrary_single_table_query(), SINGLE_TABLE_CASES, &model);
    cases.extend(generate_supported_cases(
        arbitrary_join_query_source(),
        JOIN_CASES,
        &model,
    ));

    assert!(
        cases.len() >= MIN_SUPPORTED_CASES,
        "generator reached only {} supported/emitted cases, below the {MIN_SUPPORTED_CASES} floor \
         — a generator or emitter regression narrowed the reachable shape space",
        cases.len()
    );
    let distinct_normal_forms: HashSet<&str> = cases
        .iter()
        .map(|case| case.equivalence_key.as_str())
        .collect();
    assert!(
        distinct_normal_forms.len() >= MIN_DISTINCT_NORMAL_FORMS,
        "generator reached only {} distinct normal forms, below the {MIN_DISTINCT_NORMAL_FORMS} \
         floor",
        distinct_normal_forms.len()
    );

    // Property 1: injectivity.
    assert_injective_emission(cases.iter().map(|case| {
        (
            case.equivalence_key.as_str(),
            case.source.as_str(),
            case.emitted.as_str(),
        )
    }));

    // Properties 2 and 3: idempotence and verdict preservation.
    for case in &cases {
        let round_tripped =
            try_normalize(&case.emitted, &model, ROUND_TRIP_FILE).unwrap_or_else(|| {
                panic!(
                    "emitted canonical text must itself re-lower and re-normalize: {:?} \
                     (from source {:?})",
                    case.emitted, case.source
                )
            });
        let CanonicalEmissionOutcome::Emitted(re_emitted) =
            emit_canonical_normal_form(&round_tripped)
        else {
            panic!(
                "a round-tripped normal form must remain in the supported subset: {:?}",
                case.emitted
            );
        };
        assert_eq!(
            re_emitted.as_str(),
            case.emitted,
            "canonical emission must reach a fixed point for {:?} (from source {:?})",
            case.emitted,
            case.source
        );

        let original = lower_for_comparison(&case.source, &model, SOURCE_FILE);
        let round_tripped_query = lower_for_comparison(&case.emitted, &model, ROUND_TRIP_FILE);
        assert_eq!(
            compare_relational_queries(&original, &round_tripped_query),
            ComparisonOutcome::Equivalent,
            "a canonical round trip must never change the comparison verdict: source {:?}, \
             emitted {:?}",
            case.source,
            case.emitted
        );
    }
}

/// Negative test (issue #245 acceptance criterion 4): the injectivity
/// checker itself must reject a hand-crafted collision — two different
/// normal forms (different `equivalence_key`s) claiming the same emitted
/// text. This is not exercised by the generator above (the real emitter is
/// injective over everything it currently reaches), so it is proven directly
/// against `assert_injective_emission`. Verified by temporarily disabling
/// the `assert_eq!` inside `assert_injective_emission` and confirming this
/// test fails before restoring it.
#[test]
#[should_panic(expected = "canonical emission is not injective")]
fn injectivity_check_rejects_a_hand_crafted_collision() {
    let records = [
        ("normal-form-a", "source-a", "shared-canonical-text"),
        ("normal-form-b", "source-b", "shared-canonical-text"),
    ];

    assert_injective_emission(records.iter().copied());
}

/// Assert that `comparison` matches whatever a committed record declares.
/// Factored out so the negative test below can exercise it directly against
/// a synthetic contradiction.
fn assert_matches_declared_outcome(
    comparison: &ComparisonOutcome,
    declared_equivalent: bool,
    context: &str,
) {
    let observed_equivalent = matches!(comparison, ComparisonOutcome::Equivalent);
    assert_eq!(
        observed_equivalent, declared_equivalent,
        "{context}: a declared verdict contradicts the observed M4a comparison"
    );
}

/// Negative test (issue #245 acceptance criterion 4): a committed
/// "equivalent" verdict that contradicts the real, freshly proven comparison
/// must be rejected. Verified by temporarily disabling the `assert_eq!`
/// inside `assert_matches_declared_outcome` and confirming this test fails
/// before restoring it.
#[test]
#[should_panic(expected = "a declared verdict contradicts the observed M4a comparison")]
fn verdict_preservation_check_rejects_a_contradictory_verdict() {
    let model = model();
    let base = lower_for_comparison("model::Person.all()", &model, 601);
    let renamed_schema = lower_for_comparison(
        "model::Person.all()->project(~[label: p | $p.name])",
        &model,
        602,
    );

    let comparison = compare_relational_queries(&base, &renamed_schema);
    assert_ne!(
        comparison,
        ComparisonOutcome::Equivalent,
        "fixture must be a genuine refutation for this negative test to be meaningful"
    );

    assert_matches_declared_outcome(&comparison, true, "synthetic-contradiction");
}
