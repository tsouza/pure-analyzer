//! End-to-end contracts for canonical-emission presentation.

use libpure::{
    AnalysisDriver, CanonicalEmissionOutcome, CanonicalEmissionRequest, ModelInput, SourceInput,
    SourceStore,
};
use pure_analyzer_render::{
    CanonicalEmissionOriginRole, CanonicalEmissionRenderInput, HumanOptions, RenderError,
    render_canonical_emission_human, render_canonical_emission_json,
};
use serde_json::Value;

const MODEL: &str = r#"
Class model::Person
{
  name: String[0..1];
}
"#;
const EMITTED_QUERY: &str = "/* layout comments do not survive */ model::Person.all()->project(~[label: person | $person.name])";
const INDECISIVE_QUERY: &str = "model::Person.all()->project(~[label: person | $person.missing])";

fn emit(query: &str) -> libpure::CanonicalEmissionOutput {
    AnalysisDriver
        .emit_canonical(&CanonicalEmissionRequest::new(
            SourceInput::in_memory("query.pure", query),
            [ModelInput::pure(SourceInput::in_memory(
                "model.pure",
                MODEL,
            ))],
        ))
        .expect("canonical-emission fixture must load and lower")
}

fn render_human(output: &libpure::CanonicalEmissionOutput) -> String {
    render_canonical_emission_human(
        CanonicalEmissionRenderInput::new(output.sources(), output.outcome()),
        HumanOptions::default(),
    )
    .expect("canonical-emission human output renders")
}

fn render_json(output: &libpure::CanonicalEmissionOutput) -> (String, Value) {
    let json = render_canonical_emission_json(CanonicalEmissionRenderInput::new(
        output.sources(),
        output.outcome(),
    ))
    .expect("canonical-emission JSON output renders");
    let document = serde_json::from_str(&json).expect("canonical-emission output is valid JSON");
    (json, document)
}

#[test]
fn emitted_normal_form_is_explicit_and_preserves_its_exact_text_in_json() {
    let output = emit(EMITTED_QUERY);
    assert!(matches!(
        output.outcome(),
        CanonicalEmissionOutcome::Emitted(_)
    ));
    let expected = "model::Person.all()->project(~[label: v0 | $v0.name])";

    assert_eq!(
        render_human(&output),
        format!("emitted\n  text: {expected}\n")
    );
    let (json, document) = render_json(&output);
    assert_eq!(document["version"], "1.0");
    assert_eq!(document["outcome"], "emitted");
    assert_eq!(document["text"], expected);
    assert!(document.get("reason").is_none());
    assert!(document.get("origin").is_none());
    assert!(json.contains(expected));
}

#[test]
fn indecision_preserves_the_registered_reason_and_its_origin() {
    let output = emit(INDECISIVE_QUERY);
    assert!(matches!(
        output.outcome(),
        CanonicalEmissionOutcome::Indecisive(_)
    ));

    let human = render_human(&output);
    assert!(human.starts_with("indecisive\n"));
    assert!(human.contains("reason: IND_UNRESOLVED_SCHEMA"));
    assert!(human.contains("origin:\n"));
    assert!(human.contains("query.pure"));
    assert!(human.contains("model.pure"));

    let (json, document) = render_json(&output);
    assert_eq!(document["outcome"], "indecisive");
    assert_eq!(document["reason"]["id"], "IND_UNRESOLVED_SCHEMA");
    assert_canonical_origin(&document["origin"]);
    assert!(!json.contains("witness"));
}

#[test]
fn renderer_rejects_a_refusal_when_its_retained_source_snapshot_is_missing() {
    let output = emit(INDECISIVE_QUERY);
    let empty = SourceStore::load(std::iter::empty::<SourceInput>())
        .expect("empty replacement source store is valid");

    let error =
        render_canonical_emission_json(CanonicalEmissionRenderInput::new(&empty, output.outcome()))
            .expect_err("renderer must not emit a refusal with a stale source origin");
    assert!(matches!(
        error,
        RenderError::UnknownCanonicalEmissionFile {
            role: CanonicalEmissionOriginRole::Indecision,
            ..
        }
    ));
}

fn assert_canonical_origin(origin: &Value) {
    let source = &origin["source"];
    assert_eq!(source["file"]["name"], "query.pure");
    assert_eq!(source["file"]["origin"], "memory");
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
