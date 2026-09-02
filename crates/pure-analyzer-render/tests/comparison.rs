//! End-to-end contracts for M4a comparison presentation.

use libpure::{AnalysisDriver, ComparisonOutcome, ComparisonRequest, ModelInput, SourceInput};
use pure_analyzer_render::{
    ComparisonRenderInput, HumanOptions, render_comparison_human, render_comparison_json,
};
use serde_json::Value;

const MODEL: &str = r#"
Class model::Person
{
  name: String[0..1];
}
"#;
const EQUIVALENT_QUERY: &str = "model::Person.all()";
const STRUCTURALLY_DISTINCT_QUERY: &str = "model::Person.all()->map(person| $person.name)";
const INDECISIVE_QUERY: &str = "model::Person.all()->filter(person| false)";

fn compare(left: &str, right: &str) -> libpure::ComparisonOutput {
    AnalysisDriver
        .compare(&ComparisonRequest::new(
            SourceInput::in_memory("left.pure", left),
            SourceInput::in_memory("right.pure", right),
            [ModelInput::pure(SourceInput::in_memory(
                "model.pure",
                MODEL,
            ))],
        ))
        .expect("comparison fixture must load and lower")
}

fn render_human(output: &libpure::ComparisonOutput) -> String {
    render_comparison_human(
        ComparisonRenderInput::new(output.sources(), output.outcome()),
        HumanOptions::default(),
    )
    .expect("comparison human output renders")
}

fn render_json(output: &libpure::ComparisonOutput) -> (String, Value) {
    let json = render_comparison_json(ComparisonRenderInput::new(
        output.sources(),
        output.outcome(),
    ))
    .expect("comparison JSON output renders");
    let document = serde_json::from_str(&json).expect("comparison output is valid JSON");
    (json, document)
}

#[test]
fn equivalent_comparison_has_a_witness_free_minimal_representation() {
    let output = compare(EQUIVALENT_QUERY, EQUIVALENT_QUERY);
    assert!(matches!(output.outcome(), ComparisonOutcome::Equivalent));

    assert_eq!(render_human(&output), "equivalent\n");
    let (json, document) = render_json(&output);
    assert_eq!(document["version"], "1.0");
    assert_eq!(document["outcome"], "equivalent");
    assert!(document.get("difference").is_none());
    assert!(document.get("reason").is_none());
    assert!(!json.contains("witness"));
}

#[test]
fn structural_refutation_preserves_schema_detail_and_both_canonical_origins() {
    let output = compare(EQUIVALENT_QUERY, STRUCTURALLY_DISTINCT_QUERY);
    assert!(matches!(
        output.outcome(),
        ComparisonOutcome::NotEquivalent(_)
    ));

    assert_structural_human(&render_human(&output));
    let (json, document) = render_json(&output);
    assert_structural_json(&json, &document);
}

fn assert_structural_human(human: &str) {
    assert!(human.starts_with("not_equivalent\n"));
    assert!(human.contains("difference: output_column\n"));
    assert!(human.contains("index: 0\n"));
    assert!(human.contains("field: name\n"));
    assert!(human.contains("primary_origin:\n"));
    assert!(human.contains("secondary_origin:\n"));
    assert!(human.contains("left.pure"));
    assert!(human.contains("right.pure"));
    assert!(human.contains("model.pure"));
    assert!(!human.contains("witness"));
}

fn assert_structural_json(json: &str, document: &Value) {
    assert_eq!(document["outcome"], "not_equivalent");
    assert_eq!(document["difference"]["kind"], "output_column");
    assert_eq!(document["difference"]["index"], 0);
    assert_eq!(document["difference"]["field"], "name");
    assert_comparison_origin(&document["difference"]["primary_origin"]);
    assert_comparison_origin(&document["difference"]["secondary_origin"]);
    assert!(!json.contains("witness"));
}

#[test]
fn indecision_preserves_the_registered_reason_and_its_origin() {
    let output = compare(EQUIVALENT_QUERY, INDECISIVE_QUERY);
    assert!(matches!(output.outcome(), ComparisonOutcome::Indecisive(_)));

    let human = render_human(&output);
    assert!(human.starts_with("indecisive\n"));
    assert!(human.contains("reason: IND_MISSING_REWRITE"));
    assert!(human.contains("origin:\n"));
    assert!(human.contains("left.pure") || human.contains("right.pure"));

    let (json, document) = render_json(&output);
    assert_eq!(document["outcome"], "indecisive");
    assert_eq!(document["reason"]["id"], "IND_MISSING_REWRITE");
    assert!(
        document["reason"]["blurb"]
            .as_str()
            .is_some_and(|blurb| !blurb.is_empty())
    );
    assert_comparison_origin(&document["origin"]);
    assert!(!json.contains("witness"));
}

fn assert_comparison_origin(origin: &Value) {
    let source = &origin["source"];
    assert!(source["file"]["id"].is_u64());
    assert!(source["file"]["name"].is_string());
    assert!(source["file"]["origin"].is_string());
    assert_json_range(&source["range"]);

    let model_origins = origin["model_origins"]
        .as_array()
        .expect("origin model anchors are an array");
    assert!(!model_origins.is_empty());
    let model = &model_origins[0];
    assert_eq!(model["file"]["name"], "model.pure");
    assert!(model.get("range").is_some());
}

fn assert_json_range(range: &Value) {
    for endpoint in ["start", "end"] {
        assert!(range[endpoint]["byte"].is_u64());
        assert!(range[endpoint]["line"].is_u64());
        assert!(range[endpoint]["column"].is_u64());
    }
}
