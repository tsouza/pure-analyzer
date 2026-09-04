//! Canonical round-trip verdict preservation against the *committed* M4a
//! comparison corpus (issue #245).
//!
//! `comparison_corpus.rs` already proves the frozen `comparison.jsonl`
//! witnesses replay to their committed `equivalent`/`not_equivalent` verdict
//! and match their bounded Legend oracle — that is the M4a proof itself, not
//! this file's job. What this file adds: for every decisive witness whose
//! `left`/`right` sides both happen to fall inside the canonical-emission
//! supported subset, emit each side, reparse/relower/renormalize the emitted
//! text, and re-run the *same* structural comparison on the round-tripped
//! pair — asserting it reaches the identical `equivalent`/`not_equivalent`
//! classification `comparison.jsonl` already committed to. This is the
//! generic verdict-preservation property `canonical_injectivity.rs` proves
//! over generated pairs, checked here against real, previously-committed
//! evidence instead.
//!
//! Deliberately a lightweight reader, not `comparison_corpus.rs`'s full
//! schema validator: this file only needs `id`/`model`/`left.source`/
//! `right.source`/`outcome`, and that full validation is already
//! `comparison_corpus.rs`'s own job — re-doing it here would just be
//! duplicated, driftable logic proving nothing new.
#![allow(clippy::disallowed_methods)]

use pure_analyzer_analysis::{
    AnalysisInput, CanonicalEmissionOutcome, ComparisonOutcome, NormalizationOutcome,
    RelationalOutcome, RelationalQuery, compare_relational_queries, emit_canonical_normal_form,
    lower_m3_query, normalize_relational_query,
};
use pure_analyzer_diagnostics::FileId;
use pure_analyzer_model::{ModelGraph, PureDocument, load_pure_documents};
use pure_analyzer_parser::parse_query;
use serde_json::Value;

const CORPUS_PATH: &str = "legend-4.113.0/comparison.jsonl";
const CASES: &str = include_str!("../corpus/legend-4.113.0/comparison.jsonl");
const EQUIVALENT: &str = "equivalent";
const NOT_EQUIVALENT: &str = "not_equivalent";
const LEFT_FILE: u32 = 271;
const RIGHT_FILE: u32 = 272;
const ROUND_TRIP_LEFT_FILE: u32 = 273;
const ROUND_TRIP_RIGHT_FILE: u32 = 274;

fn load_model(id: &str, model_source: &str) -> ModelGraph {
    let label = format!("canonical-comparison-corpus-{id}.pure");
    load_pure_documents(&[PureDocument::new(&label, model_source)]).unwrap_or_else(|error| {
        panic!("{CORPUS_PATH}:{id}: model {label:?} must load:\n{model_source}\n{error:#}")
    })
}

/// Lower `source`, returning `None` when it falls outside the supported
/// relational core — not every M4a witness needs to also be a supported
/// canonical-emission fixture.
fn try_lower(source: &str, model: &ModelGraph, file: u32) -> Option<RelationalQuery> {
    let parsed = parse_query(source, FileId::new(file)).ok()?;
    if !parsed.diagnostics.is_empty() {
        return None;
    }
    match lower_m3_query(AnalysisInput::new(
        FileId::new(file),
        source,
        &parsed.green,
        &parsed.diagnostics,
        Some(model),
    )) {
        RelationalOutcome::Supported(query) => Some(*query),
        RelationalOutcome::Opaque(_) => None,
    }
}

/// Emit `query`'s normal form, returning `None` when normalization or
/// emission declines (outside the canonical-emission supported subset).
fn try_emit(query: &RelationalQuery) -> Option<String> {
    let NormalizationOutcome::Normalized(normalized) = normalize_relational_query(query) else {
        return None;
    };
    match emit_canonical_normal_form(&normalized) {
        CanonicalEmissionOutcome::Emitted(emitted) => Some(emitted.into_string()),
        CanonicalEmissionOutcome::Indecisive(_) => None,
    }
}

/// Round-trip one side: emit its normal form, then reparse/relower the
/// emitted text against the same model.
fn round_trip(
    source: &str,
    model: &ModelGraph,
    source_file: u32,
    round_trip_file: u32,
) -> Option<RelationalQuery> {
    let query = try_lower(source, model, source_file)?;
    let emitted = try_emit(&query)?;
    try_lower(&emitted, model, round_trip_file)
}

#[test]
fn decisive_m4a_witnesses_preserve_their_verdict_across_a_canonical_round_trip() {
    let mut exercised = 0usize;

    for (index, line) in CASES.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let path = format!("{CORPUS_PATH}:{}", index + 1);
        let value: Value = serde_json::from_str(line)
            .unwrap_or_else(|error| panic!("{path}: invalid comparison JSON: {error}"));
        let outcome = value
            .get("outcome")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("{path}: missing outcome"));
        let declared_equivalent = match outcome {
            EQUIVALENT => true,
            NOT_EQUIVALENT => false,
            _ => continue, // Indecisive: no committed verdict to preserve.
        };
        let id = value
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("{path}: missing id"));
        let model_source = value
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("{path}: missing model"));
        let left_source = value
            .pointer("/left/source")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("{path}: missing left.source"));
        let right_source = value
            .pointer("/right/source")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("{path}: missing right.source"));

        let model = load_model(id, model_source);
        let (Some(left), Some(right)) = (
            round_trip(left_source, &model, LEFT_FILE, ROUND_TRIP_LEFT_FILE),
            round_trip(right_source, &model, RIGHT_FILE, ROUND_TRIP_RIGHT_FILE),
        ) else {
            continue; // Outside the canonical-emission supported subset.
        };

        let round_tripped_comparison = compare_relational_queries(&left, &right);
        let round_tripped_equivalent =
            matches!(round_tripped_comparison, ComparisonOutcome::Equivalent);
        assert_eq!(
            round_tripped_equivalent, declared_equivalent,
            "{path} ({id}): a canonical round trip changed the committed M4a verdict \
             (declared {outcome}, round-tripped comparison: {round_tripped_comparison:#?})"
        );
        exercised += 1;
    }

    assert!(
        exercised > 0,
        "{CORPUS_PATH} must contain at least one decisive witness inside the \
         canonical-emission supported subset"
    );
}
