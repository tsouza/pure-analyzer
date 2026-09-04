//! Deterministic replay of pinned canonical-emission fixtures against exact
//! Legend 4.113.0 evidence (issue #245).
//!
//! Each fixture's `source` is a canonical text `canonical.rs` actually
//! emitted for one supported normal form, refreshed against a live pinned
//! engine by `scripts/analysis-canonical-corpus.mjs --refresh`. This test is
//! hermetic (no engine contacted): it re-derives the same normal form from
//! `source` and proves three things per fixture, with full source/model
//! provenance on any failure:
//!
//! 1. **Idempotence** — re-lowering, re-normalizing, and re-emitting `source`
//!    reproduces `source` exactly (the fixed point `canonical_injectivity.rs`
//!    proves generically; this pins it against real, previously-observed
//!    text instead of only generated text).
//! 2. **Oracle correspondence** — the fixture's bounded [`Oracle`] structurally
//!    matches the lowered query's root shape (see `legend_oracle.rs`'s doc
//!    for exactly what this does and does not prove).
//! 3. **Provenance** — the normal form's origin still carries both the query
//!    file and the model it was lowered against.
//!
//! Root shapes without an existing bounded-oracle kind (`distinct`,
//! `distinct(~[...])`, `sort`, `join`) are deliberately out of this pinned
//! corpus's scope — see the module doc of `canonical_strategy.rs` and this
//! crate's PR body for what remains. They are still proven hermetically (no
//! live engine needed) by `canonical_injectivity.rs`'s generator, including
//! as *embedded* (non-root) stages here (`distinct-then-project`).
#![allow(clippy::disallowed_methods)]

use pure_analyzer_analysis::{
    AnalysisInput, CanonicalEmissionOutcome, IrOrigin, NormalizationOutcome, RelationalOutcome,
    RelationalQuery, emit_canonical_normal_form, lower_m3_query, normalize_relational_query,
};
use pure_analyzer_diagnostics::FileId;
use pure_analyzer_model::{ModelGraph, PureDocument, load_pure_documents};
use pure_analyzer_parser::parse_query;
use serde_json::Value;

#[path = "support/legend_oracle.rs"]
mod legend_oracle;

use legend_oracle::{
    Oracle, assert_exact_fields, assert_oracle_matches_query, non_empty_string, object,
    parse_oracle, required_value,
};

const CORPUS_PATH: &str = "legend-4.113.0/canonical.jsonl";
const CASES: &str = include_str!("../corpus/legend-4.113.0/canonical.jsonl");
const QUERY_FILE: u32 = 281;

struct CanonicalFixture<'source> {
    id: &'source str,
    model: &'source str,
    source: &'source str,
    oracle: Oracle,
}

fn parse_fixture<'source>(value: &'source Value, path: &str) -> CanonicalFixture<'source> {
    let fixture = object(value, path);
    assert_exact_fields(
        fixture,
        &["id", "model", "source", "oracle", "lambda", "result"],
        path,
    );
    let id = non_empty_string(fixture, "id", path);
    let model = non_empty_string(fixture, "model", path);
    let source = non_empty_string(fixture, "source", path);
    let oracle = parse_oracle(
        required_value(fixture, "oracle", path),
        &format!("{path}:oracle"),
    );
    let lambda = non_empty_string(fixture, "lambda", path);
    assert_eq!(
        lambda,
        oracle.lambda(),
        "{path}: lambda must exactly render its bounded oracle"
    );
    required_value(fixture, "result", path);
    CanonicalFixture {
        id,
        model,
        source,
        oracle,
    }
}

fn context(fixture: &CanonicalFixture<'_>) -> String {
    format!(
        "{CORPUS_PATH}:{}\nmodel:\n{}\nsource:\n{}",
        fixture.id, fixture.model, fixture.source
    )
}

fn assert_origin_has_query_and_model_provenance(
    context: &str,
    origin: &IrOrigin,
    file: FileId,
    model: &ModelGraph,
) {
    assert_eq!(
        origin.source().file(),
        file,
        "{context}\ncanonical fixture origin refers to unexpected query file"
    );
    let model_source = model
        .sources()
        .first()
        .unwrap_or_else(|| panic!("{context}\nfixture model has no recorded source"));
    assert!(
        origin
            .model_origins()
            .iter()
            .any(|model_origin| model_origin.definition().source() == model_source.id()),
        "{context}\ncanonical fixture origin lost model provenance from {}",
        model_source.label(),
    );
}

fn lower_fixture(fixture: &CanonicalFixture<'_>, model: &ModelGraph) -> RelationalQuery {
    let context = context(fixture);
    let parsed = parse_query(fixture.source, FileId::new(QUERY_FILE))
        .unwrap_or_else(|error| panic!("{context}\npinned source must parse: {error}"));
    assert!(
        parsed.diagnostics.is_empty(),
        "{context}\npinned source parser diagnostics: {:#?}",
        parsed.diagnostics
    );
    let RelationalOutcome::Supported(query) = lower_m3_query(AnalysisInput::new(
        FileId::new(QUERY_FILE),
        fixture.source,
        &parsed.green,
        &parsed.diagnostics,
        Some(model),
    )) else {
        panic!("{context}\npinned source must lower through the supported core");
    };
    *query
}

fn assert_fixture(fixture: &CanonicalFixture<'_>) {
    let context = context(fixture);
    let label = format!("canonical-corpus-{}.pure", fixture.id);
    let model = load_pure_documents(&[PureDocument::new(&label, fixture.model)])
        .unwrap_or_else(|error| panic!("{context}\nmodel {label:?} must load: {error:#}"));

    let query = lower_fixture(fixture, &model);
    assert_origin_has_query_and_model_provenance(
        &context,
        query.root().origin(),
        FileId::new(QUERY_FILE),
        &model,
    );
    assert_oracle_matches_query(&fixture.oracle, &query, &context);

    let NormalizationOutcome::Normalized(normalized) = normalize_relational_query(&query) else {
        panic!("{context}\npinned source must normalize");
    };
    let CanonicalEmissionOutcome::Emitted(emitted) = emit_canonical_normal_form(&normalized) else {
        panic!("{context}\npinned source must stay in the canonical-emission supported subset");
    };
    assert_eq!(
        emitted.as_str(),
        fixture.source,
        "{context}\npinned canonical fixture drifted from what the emitter actually produces \
         for its own normal form — refresh via `just analysis-canonical-corpus-refresh` if this \
         is an intentional emitter change, otherwise this is exactly the fixed point issue #245 \
         requires"
    );
}

#[test]
fn frozen_canonical_fixtures_stay_at_their_fixed_point_and_match_pinned_evidence() {
    let mut count = 0;
    for (index, line) in CASES.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let path = format!("{CORPUS_PATH}:{}", index + 1);
        let value: Value = serde_json::from_str(line)
            .unwrap_or_else(|error| panic!("{path}: invalid canonical fixture JSON: {error}"));
        let fixture = parse_fixture(&value, &path);
        assert_fixture(&fixture);
        count += 1;
    }
    assert!(count > 0, "{CORPUS_PATH} must contain canonical fixtures");
}

/// Negative test (issue #245 acceptance criterion 4): a fixture whose pinned
/// `source` has drifted from what the emitter actually produces for its own
/// normal form must be rejected — the exact "stale fixture" class the
/// acceptance criteria name. `x` never survives to canonical text (the
/// emitter always renumbers binders to `v0`, `v1`, ... and always
/// parenthesizes an equality), so this can never coincidentally equal its own
/// fixed point. Verified by temporarily disabling the `assert_eq!` inside
/// `assert_fixture` and confirming this test fails before restoring it.
#[test]
#[should_panic(expected = "drifted from what the emitter actually produces")]
fn a_fixture_whose_source_drifted_from_its_own_fixed_point_is_rejected() {
    let stale = serde_json::json!({
        "id": "stale-drift-regression",
        "model": "Class test::Row\n{\n  name: String[1];\n  email: String[1];\n}\n",
        "source": "test::Row.all()->filter(x| $x.name == 'Ada')",
        "oracle": {"kind": "literal_filter", "values": ["Ada", "Grace"], "value": "Ada"},
        "lambda": "|['Ada', 'Grace']->filter(x: String[1]|$x == 'Ada')",
        "result": ["Ada"],
    });
    let fixture = parse_fixture(&stale, "synthetic-stale-fixture");

    assert_fixture(&fixture);
}
