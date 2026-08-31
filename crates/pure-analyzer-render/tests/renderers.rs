//! End-to-end contracts for each diagnostic representation.

use pure_analyzer_diagnostics::{
    DiagCode, Diagnostic, FileId, Fix, Label, ReasonCode, Severity, TextEdit, TextRange, Verdict,
};
use pure_analyzer_render::{
    HumanOptions, RenderError, RenderInput, SpanKind, render_human, render_json, render_sarif,
};
use serde_json::Value;

use libpure::{SourceInput, SourceStore};

fn fixture_sources() -> SourceStore {
    SourceStore::load([
        SourceInput::in_memory("queries/α.pure", "let α = value;\n"),
        SourceInput::in_memory("models/β.pure", "Class β {}\n"),
    ])
    .expect("fixture sources load")
}

fn range(start: u32, end: u32) -> TextRange {
    TextRange::new(start.into(), end.into())
}

fn rich_diagnostic() -> Diagnostic {
    let primary = Label::with_note(FileId::new(0), range(4, 6), "query symbol");
    let fix = Fix::single_arity_proven(
        "replace the query symbol",
        vec![TextEdit {
            file: FileId::new(0),
            span: range(4, 6),
            new_text: "γ\n".to_owned(),
        }],
    );
    Diagnostic::builder(
        DiagCode::UnknownProperty,
        Severity::Error,
        "unknown \"α\" \\ path",
        primary,
    )
    .secondary(Label::with_note(
        FileId::new(1),
        range(6, 8),
        "declared here",
    ))
    .fix(fix)
    .verdict(Verdict::NotEquivalent {
        witness: "#>{db::T}#".to_owned(),
    })
    .reason(ReasonCode::IndOpaquePredicate)
    .url("https://example.invalid/PUR2002?x=1&y=2")
    .build()
}

fn later_diagnostic() -> Diagnostic {
    Diagnostic::builder(
        DiagCode::ModelMergeConflict,
        Severity::Warning,
        "a later finding",
        Label::with_note(FileId::new(1), range(0, 5), "model declaration"),
    )
    .build()
}

#[test]
fn rich_fixture_cross_format_contract() {
    let sources = fixture_sources();
    let diagnostics = vec![rich_diagnostic()];
    let input = RenderInput::new(&sources, &diagnostics);

    let human = render_human(input, HumanOptions::default()).expect("human renders");
    let json = render_json(input).expect("json renders");
    let sarif = render_sarif(input).expect("sarif renders");

    assert_eq!(human, include_str!("golden/rich.human"));
    assert_eq!(json, include_str!("golden/rich.json"));
    assert_eq!(sarif, include_str!("golden/rich.sarif"));

    assert_human_contract(&human);
    assert_json_contract(&json);
    assert_sarif_contract(&sarif);
}

fn assert_human_contract(human: &str) {
    assert!(human.contains("primary: query symbol"));
    assert!(human.contains("secondary: declared here"));
    assert!(human.contains("replace the query symbol"));
    assert!(human.contains("docs: https://example.invalid/PUR2002?x=1&y=2"));
}

fn assert_json_contract(json: &str) {
    let document: Value = serde_json::from_str(json).expect("valid JSON envelope");
    assert_eq!(document["version"], "1.0");
    assert_eq!(document["files"][0]["name"], "queries/α.pure");
    assert_eq!(document["files"][0]["origin"], "memory");
    assert_eq!(
        document["diagnostics"][0]["primary"]["range"]["start"]["byte"],
        4
    );
    assert_eq!(
        document["diagnostics"][0]["primary"]["range"]["start"]["line"],
        1
    );
    assert_eq!(
        document["diagnostics"][0]["primary"]["range"]["start"]["column"],
        5
    );
    assert_eq!(
        document["diagnostics"][0]["fix"]["edits"][0]["replacement"],
        "γ\n"
    );
    assert_eq!(document["summary"]["errors"], 1);
}

fn assert_sarif_contract(sarif: &str) {
    let log: Value = serde_json::from_str(sarif).expect("valid SARIF JSON");
    assert_eq!(
        log["$schema"],
        "https://json.schemastore.org/sarif-2.1.0.json"
    );
    assert_eq!(log["version"], "2.1.0");
    assert_eq!(
        log["runs"][0]["tool"]["driver"]["rules"][0]["id"],
        "PUR2002"
    );
    assert_eq!(
        log["runs"][0]["tool"]["driver"]["rules"][0]["shortDescription"]["text"],
        "unknown property"
    );
    assert_eq!(log["runs"][0]["results"][0]["level"], "error");
    assert_eq!(log["runs"][0]["results"][0]["relatedLocations"][0]["id"], 1);
    assert_eq!(
        log["runs"][0]["results"][0]["fixes"][0]["artifactChanges"][0]["replacements"][0]["insertedContent"]
            ["text"],
        "γ\n"
    );
}

#[test]
fn renderer_order_is_independent_of_input_order() {
    let sources = fixture_sources();
    let diagnostics = vec![later_diagnostic(), rich_diagnostic()];
    let reversed = diagnostics.iter().cloned().rev().collect::<Vec<_>>();

    let input = RenderInput::new(&sources, &diagnostics);
    let reversed_input = RenderInput::new(&sources, &reversed);
    let human = render_human(input, HumanOptions::default()).expect("human renders");
    let json = render_json(input).expect("json renders");
    let sarif = render_sarif(input).expect("sarif renders");
    assert_eq!(
        human,
        render_human(reversed_input, HumanOptions::default()).expect("reversed human renders")
    );
    assert_eq!(
        json,
        render_json(reversed_input).expect("reversed JSON renders")
    );
    assert_eq!(
        sarif,
        render_sarif(reversed_input).expect("reversed SARIF renders")
    );

    let json_codes = codes_from_json(&json, &["diagnostics"]);
    let sarif_codes = codes_from_json(&sarif, &["runs", "0", "results"]);
    assert_eq!(json_codes, ["PUR2002", "PUR9000"]);
    assert_eq!(sarif_codes, json_codes);
    assert!(
        human.find("PUR2002").expect("first code") < human.find("PUR9000").expect("second code")
    );
}

#[test]
fn resolved_color_choice_controls_ansi_sequences() {
    let sources = fixture_sources();
    let diagnostics = vec![rich_diagnostic()];
    let input = RenderInput::new(&sources, &diagnostics);

    let plain = render_human(input, HumanOptions { color: false }).expect("plain renders");
    let colored = render_human(input, HumanOptions { color: true }).expect("colored renders");
    assert!(!plain.contains("\x1b["));
    assert!(colored.contains("\x1b[1;31m"));
    assert!(colored.contains("error[PUR2002]"));
    assert!(colored.contains("\x1b[0m"));
}

#[test]
fn human_renderer_marks_zero_width_and_multiline_spans() {
    let sources = SourceStore::load([SourceInput::in_memory("spans.pure", "one\ntwo\n")])
        .expect("source loads");
    let diagnostics = vec![
        Diagnostic::builder(
            DiagCode::BadToken,
            Severity::Error,
            "multiline",
            Label::with_note(FileId::new(0), range(1, 5), "crosses lines"),
        )
        .build(),
        Diagnostic::builder(
            DiagCode::BadToken,
            Severity::Error,
            "insertion",
            Label::with_note(FileId::new(0), range(3, 3), "insertion point"),
        )
        .build(),
    ];
    let output = render_human(
        RenderInput::new(&sources, &diagnostics),
        HumanOptions::default(),
    )
    .expect("spans render");

    assert!(output.contains("spans.pure:1:2..2:2 (primary)"));
    assert!(output.contains("spans.pure:1:4..1:4 (primary)"));
    assert!(output.contains("^^ primary: crosses lines"));
    assert!(output.contains("^ primary: insertion point"));
}

#[test]
fn invalid_unicode_boundary_is_an_internal_error_in_every_format() {
    let sources =
        SourceStore::load([SourceInput::in_memory("unicode.pure", "α")]).expect("source loads");
    let diagnostics = vec![
        Diagnostic::builder(
            DiagCode::BadToken,
            Severity::Error,
            "bad token",
            Label::new(FileId::new(0), range(1, 2)),
        )
        .build(),
    ];
    let input = RenderInput::new(&sources, &diagnostics);

    for result in [
        render_human(input, HumanOptions::default()),
        render_json(input),
        render_sarif(input),
    ] {
        assert!(matches!(result, Err(RenderError::InvalidSpan { .. })));
    }
}

#[test]
fn unknown_source_is_an_internal_error_in_every_format() {
    let sources = fixture_sources();
    let diagnostics = vec![
        Diagnostic::builder(
            DiagCode::BadToken,
            Severity::Error,
            "bad token",
            Label::new(FileId::new(99), range(0, 0)),
        )
        .build(),
    ];
    let input = RenderInput::new(&sources, &diagnostics);

    for result in all_formats(input) {
        assert!(matches!(
            result,
            Err(RenderError::UnknownFile {
                kind: SpanKind::Primary,
                ..
            })
        ));
    }
}

#[test]
fn stale_secondary_and_fix_spans_are_internal_errors_in_every_format() {
    let sources =
        SourceStore::load([SourceInput::in_memory("stale.pure", "ok")]).expect("source loads");
    let secondary = Diagnostic::builder(
        DiagCode::BadToken,
        Severity::Error,
        "bad token",
        Label::new(FileId::new(0), range(0, 1)),
    )
    .secondary(Label::new(FileId::new(0), range(1, 3)))
    .build();
    let fix = Diagnostic::builder(
        DiagCode::BadToken,
        Severity::Error,
        "bad token",
        Label::new(FileId::new(0), range(0, 1)),
    )
    .fix(Fix::model_dependent(
        "stale edit",
        vec![TextEdit {
            file: FileId::new(0),
            span: range(1, 3),
            new_text: "x".to_owned(),
        }],
    ))
    .build();

    assert_invalid_span_kind(&sources, vec![secondary], SpanKind::Secondary(0));
    assert_invalid_span_kind(&sources, vec![fix], SpanKind::FixEdit(0));
}

fn assert_invalid_span_kind(sources: &SourceStore, diagnostics: Vec<Diagnostic>, kind: SpanKind) {
    let input = RenderInput::new(sources, &diagnostics);
    for result in all_formats(input) {
        assert!(matches!(
            result,
            Err(RenderError::InvalidSpan { kind: actual, .. }) if actual == kind
        ));
    }
}

fn all_formats(input: RenderInput<'_>) -> [Result<String, RenderError>; 3] {
    [
        render_human(input, HumanOptions::default()),
        render_json(input),
        render_sarif(input),
    ]
}

fn codes_from_json(output: &str, path: &[&str]) -> Vec<String> {
    let document: Value = serde_json::from_str(output).expect("renderer output is JSON");
    let diagnostics = path.iter().fold(&document, |value, segment| {
        if let Ok(index) = segment.parse::<usize>() {
            &value[index]
        } else {
            &value[*segment]
        }
    });
    diagnostics
        .as_array()
        .expect("diagnostic list")
        .iter()
        .map(|diagnostic| {
            diagnostic[if path == ["diagnostics"] {
                "code"
            } else {
                "ruleId"
            }]
            .as_str()
            .expect("diagnostic code")
            .to_owned()
        })
        .collect()
}
