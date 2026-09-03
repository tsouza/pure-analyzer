//! End-to-end contracts for each diagnostic representation.

use std::{
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};

use pure_analyzer_diagnostics::{
    Applicability, DiagCode, Diagnostic, FileId, Fix, FixProvenance, Label, ReasonCode, Severity,
    TextEdit, TextRange, Verdict,
};
use pure_analyzer_render::{
    ColorPolicy, HumanOptions, RenderError, RenderInput, SpanKind, render_human, render_json,
    render_sarif,
};
use serde_json::Value;

use libpure::{SourceInput, SourceStore};

const TEMP_FILE_PREFIX: &str = "pure-analyzer-render-test";
// The bidi-control suffix (RLO, an LRI isolate, and the LRM mark) covers the
// Trojan Source class (CVE-2021-42574): non-`char::is_control()` code points
// that can silently reorder how surrounding text *displays* in a terminal.
// See the Unicode `Bidi_Control` property in `PropList.txt`.
const TERMINAL_CONTROLS: &str =
    "\0\t\x1b]8;;https://example.invalid\x07\r\u{009b}β\u{202e}\u{2066}\u{200e}";
const ESCAPED_TERMINAL_CONTROLS: &str =
    r"\0\t\u{1b}]8;;https://example.invalid\u{7}\r\u{9b}β\u{202e}\u{2066}\u{200e}";
const TERMINAL_CONTROL_TARGET: &str = "target";

static TEMP_FILE_COUNTER: AtomicUsize = AtomicUsize::new(0);

struct FileFixture {
    path: PathBuf,
}

impl FileFixture {
    fn new(name: &str, text: &str) -> Self {
        let counter = TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "{TEMP_FILE_PREFIX}-{}-{counter}-{name}",
            std::process::id()
        ));
        std::fs::write(&path, text).expect("write file fixture");
        Self { path }
    }
}

impl Drop for FileFixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

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

fn nested_labels_diagnostic(
    reverse_labels: bool,
    first_note: &str,
    second_note: &str,
) -> Diagnostic {
    let mut secondary = vec![
        Label::with_note(FileId::new(0), range(0, 3), first_note),
        Label::with_note(FileId::new(1), range(6, 8), second_note),
    ];
    if reverse_labels {
        secondary.reverse();
    }

    let mut builder = Diagnostic::builder(
        DiagCode::UnknownProperty,
        Severity::Error,
        "same primary finding",
        Label::new(FileId::new(0), range(4, 6)),
    );
    for label in secondary {
        builder = builder.secondary(label);
    }
    builder.build()
}

fn fix_edits_diagnostic(reverse_edits: bool) -> Diagnostic {
    let mut edits = vec![
        TextEdit {
            file: FileId::new(1),
            span: range(0, 5),
            new_text: "Model".to_owned(),
        },
        TextEdit {
            file: FileId::new(0),
            span: range(0, 3),
            new_text: "query".to_owned(),
        },
    ];
    if reverse_edits {
        edits.reverse();
    }

    Diagnostic::builder(
        DiagCode::UnknownProperty,
        Severity::Error,
        "ordered edits",
        Label::new(FileId::new(0), range(4, 6)),
    )
    .fix(Fix::model_dependent("replace both", edits))
    .build()
}

fn ordered_fix_diagnostic(applicability: Applicability, provenance: FixProvenance) -> Diagnostic {
    Diagnostic::builder(
        DiagCode::UnknownProperty,
        Severity::Error,
        "same fix metadata except provenance",
        Label::new(FileId::new(0), range(4, 6)),
    )
    .fix(Fix {
        title: "replace the query symbol".to_owned(),
        applicability,
        provenance,
        edits: vec![TextEdit {
            file: FileId::new(0),
            span: range(4, 6),
            new_text: "γ".to_owned(),
        }],
    })
    .build()
}

fn edit_order_diagnostic(replacement: &str) -> Diagnostic {
    Diagnostic::builder(
        DiagCode::UnknownProperty,
        Severity::Error,
        "same fix metadata except replacement text",
        Label::new(FileId::new(0), range(4, 6)),
    )
    .fix(Fix {
        title: "replace the query symbol".to_owned(),
        applicability: Applicability::Suggested,
        provenance: FixProvenance::SyntaxOnly,
        edits: vec![TextEdit {
            file: FileId::new(0),
            span: range(4, 6),
            new_text: replacement.to_owned(),
        }],
    })
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
    assert!(human.contains("error[PUR2002]: unknown \"α\" \\ path"));
    assert!(human.contains("queries/α.pure:1:5..1:7 (primary)"));
    assert!(human.contains("models/β.pure:1:7..1:9 (secondary)"));
    assert!(human.contains("primary: query symbol"));
    assert!(human.contains("secondary: declared here"));
    assert!(human.contains("replace the query symbol"));
    assert!(human.contains(r#"with "γ\n""#));
    assert!(human.contains("not_equivalent; witness: #>{db::T}#"));
    assert!(human.contains("IND_OPAQUE_PREDICATE"));
    assert!(human.contains("docs: https://example.invalid/PUR2002?x=1&y=2"));
    assert!(human.contains("summary: 1 errors, 0 warnings, 0 info, 0 hints (1 total)"));
}

fn assert_json_contract(json: &str) {
    let document: Value = serde_json::from_str(json).expect("valid JSON envelope");
    assert_eq!(document["version"], "1.0");
    assert_eq!(document["files"][0]["name"], "queries/α.pure");
    assert_eq!(document["files"][0]["origin"], "memory");
    let diagnostic = &document["diagnostics"][0];
    assert_json_finding(diagnostic);
    assert_json_fix_and_metadata(diagnostic);
    assert_eq!(document["summary"]["errors"], 1);
}

fn assert_json_finding(diagnostic: &Value) {
    assert_eq!(diagnostic["code"], "PUR2002");
    assert_eq!(diagnostic["severity"], "error");
    assert_eq!(diagnostic["message"], "unknown \"α\" \\ path");
    assert_json_range(&diagnostic["primary"]["range"], (4, 1, 5), (6, 1, 7));
    assert_eq!(diagnostic["primary"]["note"], "query symbol");
    assert_json_range(&diagnostic["secondary"][0]["range"], (6, 1, 7), (8, 1, 9));
    assert_eq!(diagnostic["secondary"][0]["note"], "declared here");
}

fn assert_json_fix_and_metadata(diagnostic: &Value) {
    assert_eq!(diagnostic["fix"]["title"], "replace the query symbol");
    assert_eq!(diagnostic["fix"]["applicability"], "machine_applicable");
    assert_eq!(diagnostic["fix"]["provenance"], "single_arity_proven");
    assert_json_range(
        &diagnostic["fix"]["edits"][0]["range"],
        (4, 1, 5),
        (6, 1, 7),
    );
    assert_eq!(diagnostic["fix"]["edits"][0]["replacement"], "γ\n");
    assert_eq!(diagnostic["verdict"]["verdict"], "not_equivalent");
    assert_eq!(diagnostic["verdict"]["witness"], "#>{db::T}#");
    assert_eq!(diagnostic["reason"]["id"], "IND_OPAQUE_PREDICATE");
    assert_eq!(diagnostic["url"], "https://example.invalid/PUR2002?x=1&y=2");
}

fn assert_sarif_contract(sarif: &str) {
    let log: Value = serde_json::from_str(sarif).expect("valid SARIF JSON");
    assert_eq!(
        log["$schema"],
        "https://json.schemastore.org/sarif-2.1.0.json"
    );
    assert_eq!(log["version"], "2.1.0");
    assert_eq!(log["runs"][0]["columnKind"], "unicodeCodePoints");
    assert_sarif_rule(&log["runs"][0]["tool"]["driver"]["rules"][0]);
    assert_sarif_result(&log["runs"][0]["results"][0]);
}

#[test]
fn sarif_emits_properties_for_a_reason_without_verdict_or_documentation() {
    let sources = fixture_sources();
    let diagnostics = vec![
        Diagnostic::builder(
            DiagCode::UnknownProperty,
            Severity::Error,
            "reason-only finding",
            Label::new(FileId::new(0), range(4, 6)),
        )
        .reason(ReasonCode::IndOpaquePredicate)
        .build(),
    ];

    let sarif = render_sarif(RenderInput::new(&sources, &diagnostics)).expect("SARIF renders");
    let log: Value = serde_json::from_str(&sarif).expect("renderer output is SARIF JSON");
    let properties = &log["runs"][0]["results"][0]["properties"];

    assert_eq!(properties["reason"]["id"], "IND_OPAQUE_PREDICATE");
    assert!(properties.get("verdict").is_none());
    assert!(properties.get("documentationUrl").is_none());
}

fn assert_sarif_rule(rule: &Value) {
    assert_eq!(rule["id"], "PUR2002");
    assert_eq!(rule["shortDescription"]["text"], "unknown property");
    assert_eq!(
        rule["helpUri"],
        "https://github.com/tsouza/pure-analyzer/tree/main/docs"
    );
    assert_eq!(rule["defaultConfiguration"]["level"], "error");
}

fn assert_sarif_result(result: &Value) {
    assert_eq!(result["ruleId"], "PUR2002");
    assert_eq!(result["level"], "error");
    assert_eq!(result["message"]["text"], "unknown \"α\" \\ path");
    assert_sarif_locations(result);
    assert_sarif_fix_and_metadata(result);
}

fn assert_sarif_locations(result: &Value) {
    assert_eq!(
        result["locations"][0]["physicalLocation"]["artifactLocation"]["uri"],
        "queries/%CE%B1.pure"
    );
    assert_sarif_region(
        &result["locations"][0]["physicalLocation"]["region"],
        (4, 1, 5),
        (6, 1, 6),
    );
    assert_eq!(result["locations"][0]["message"]["text"], "query symbol");
    assert_eq!(result["relatedLocations"][0]["id"], 1);
    assert_eq!(
        result["relatedLocations"][0]["physicalLocation"]["artifactLocation"]["uri"],
        "models/%CE%B2.pure"
    );
    assert_sarif_region(
        &result["relatedLocations"][0]["physicalLocation"]["region"],
        (6, 1, 7),
        (8, 1, 8),
    );
    assert_eq!(
        result["relatedLocations"][0]["message"]["text"],
        "declared here"
    );
}

fn assert_sarif_fix_and_metadata(result: &Value) {
    assert_sarif_region(
        &result["fixes"][0]["artifactChanges"][0]["replacements"][0]["deletedRegion"],
        (4, 1, 5),
        (6, 1, 6),
    );
    assert_eq!(
        result["fixes"][0]["artifactChanges"][0]["replacements"][0]["insertedContent"]["text"],
        "γ\n"
    );
    assert_eq!(
        result["fixes"][0]["properties"]["applicability"],
        "machine_applicable"
    );
    assert_eq!(
        result["fixes"][0]["properties"]["provenance"],
        "single_arity_proven"
    );
    assert_eq!(result["properties"]["verdict"]["verdict"], "not_equivalent");
    assert_eq!(result["properties"]["reason"]["id"], "IND_OPAQUE_PREDICATE");
    assert_eq!(
        result["properties"]["documentationUrl"],
        "https://example.invalid/PUR2002?x=1&y=2"
    );
}

fn assert_json_range(range: &Value, start: (u32, u64, u64), end: (u32, u64, u64)) {
    assert_json_position(&range["start"], start);
    assert_json_position(&range["end"], end);
}

fn assert_json_position(position: &Value, expected: (u32, u64, u64)) {
    assert_eq!(position["byte"], expected.0);
    assert_eq!(position["line"], expected.1);
    assert_eq!(position["column"], expected.2);
}

fn assert_sarif_region(region: &Value, start: (u32, u64, u64), end: (u32, u64, u64)) {
    assert_eq!(region["byteOffset"], start.0);
    assert_eq!(region["byteLength"], end.0 - start.0);
    assert_eq!(region["startLine"], start.1);
    assert_eq!(region["startColumn"], start.2);
    assert_eq!(region["endLine"], end.1);
    assert_eq!(region["endColumn"], end.2);
}

#[test]
fn severity_mappings_and_counters_are_consistent_across_formats() {
    let sources =
        SourceStore::load([SourceInput::in_memory("severity.pure", "abcd")]).expect("source loads");
    let diagnostics = [
        (DiagCode::BadToken, Severity::Error, "error"),
        (DiagCode::MalformedSyntax, Severity::Warning, "warning"),
        (DiagCode::UnknownProperty, Severity::Info, "info"),
        (DiagCode::ModelMergeConflict, Severity::Hint, "hint"),
    ]
    .into_iter()
    .enumerate()
    .map(|(offset, (code, severity, message))| {
        let start = u32::try_from(offset).expect("fixture offset fits a span");
        Diagnostic::builder(
            code,
            severity,
            message,
            Label::new(FileId::new(0), range(start, start.saturating_add(1))),
        )
        .build()
    })
    .collect::<Vec<_>>();
    let input = RenderInput::new(&sources, &diagnostics);

    let human = render_human(input, HumanOptions::default()).expect("human renders");
    for header in [
        "error[PUR0102]: error",
        "warning[PUR1200]: warning",
        "info[PUR2002]: info",
        "hint[PUR9000]: hint",
    ] {
        assert!(
            human.contains(header),
            "human output must contain `{header}`"
        );
    }
    assert!(human.contains("summary: 1 errors, 1 warnings, 1 info, 1 hints (4 total)"));

    let json = render_json(input).expect("JSON renders");
    let document: Value = serde_json::from_str(&json).expect("renderer output is JSON");
    let json_severities = document["diagnostics"]
        .as_array()
        .expect("JSON diagnostics")
        .iter()
        .map(|diagnostic| diagnostic["severity"].as_str().expect("JSON severity"))
        .collect::<Vec<_>>();
    assert_eq!(json_severities, ["error", "warning", "info", "hint"]);
    assert_eq!(document["summary"]["errors"], 1);
    assert_eq!(document["summary"]["warnings"], 1);
    assert_eq!(document["summary"]["info"], 1);
    assert_eq!(document["summary"]["hints"], 1);
    assert_eq!(document["summary"]["total"], 4);

    let sarif = render_sarif(input).expect("SARIF renders");
    let log: Value = serde_json::from_str(&sarif).expect("renderer output is SARIF JSON");
    let run = &log["runs"][0];
    let result_levels = run["results"]
        .as_array()
        .expect("SARIF results")
        .iter()
        .map(|result| result["level"].as_str().expect("SARIF result level"))
        .collect::<Vec<_>>();
    assert_eq!(result_levels, ["error", "warning", "note", "none"]);
    let rule_levels = run["tool"]["driver"]["rules"]
        .as_array()
        .expect("SARIF rules")
        .iter()
        .map(|rule| {
            rule["defaultConfiguration"]["level"]
                .as_str()
                .expect("SARIF rule level")
        })
        .collect::<Vec<_>>();
    assert_eq!(rule_levels, ["error", "warning", "note", "none"]);
}

#[test]
fn json_lists_file_and_stdin_origins_with_their_display_names() {
    let file = FileFixture::new("json-origin.pure", "file()\n");
    let expected_file_name = file.path.display().to_string();
    let sources = SourceStore::load([
        SourceInput::file(&file.path),
        SourceInput::stdin("stdin()\n"),
    ])
    .expect("sources load");
    let diagnostics = Vec::<Diagnostic>::new();

    let output = render_json(RenderInput::new(&sources, &diagnostics)).expect("JSON renders");
    let document: Value = serde_json::from_str(&output).expect("renderer output is JSON");
    let files = document["files"].as_array().expect("JSON files");

    assert_eq!(files.len(), 2);
    assert_eq!(files[0]["id"], 0);
    assert_eq!(files[0]["name"].as_str(), Some(expected_file_name.as_str()));
    assert_eq!(files[0]["origin"], "file");
    assert_eq!(files[1]["id"], 1);
    assert_eq!(files[1]["name"], "<stdin>");
    assert_eq!(files[1]["origin"], "stdin");
}

#[test]
fn sarif_aggregates_duplicate_rule_ids_by_minimum_severity_in_code_order() {
    let sources =
        SourceStore::load([SourceInput::in_memory("rules.pure", "abc")]).expect("source loads");
    let diagnostics = vec![
        Diagnostic::builder(
            DiagCode::UnknownProperty,
            Severity::Hint,
            "late hint",
            Label::new(FileId::new(0), range(2, 3)),
        )
        .build(),
        Diagnostic::builder(
            DiagCode::UnknownProperty,
            Severity::Error,
            "duplicate error",
            Label::new(FileId::new(0), range(1, 2)),
        )
        .build(),
        Diagnostic::builder(
            DiagCode::BadToken,
            Severity::Warning,
            "early warning",
            Label::new(FileId::new(0), range(0, 1)),
        )
        .build(),
    ];

    let output = render_sarif(RenderInput::new(&sources, &diagnostics)).expect("SARIF renders");
    let log: Value = serde_json::from_str(&output).expect("renderer output is SARIF JSON");
    let run = &log["runs"][0];
    let rules = run["tool"]["driver"]["rules"]
        .as_array()
        .expect("SARIF rules");
    assert_eq!(rules.len(), 2);
    assert_eq!(rules[0]["id"], "PUR0102");
    assert_eq!(rules[0]["defaultConfiguration"]["level"], "warning");
    assert_eq!(rules[1]["id"], "PUR2002");
    assert_eq!(rules[1]["defaultConfiguration"]["level"], "error");

    let results = run["results"].as_array().expect("SARIF results");
    let result_ids = results
        .iter()
        .map(|result| result["ruleId"].as_str().expect("SARIF result rule ID"))
        .collect::<Vec<_>>();
    assert_eq!(result_ids, ["PUR0102", "PUR2002", "PUR2002"]);
    let result_levels = results
        .iter()
        .map(|result| result["level"].as_str().expect("SARIF result level"))
        .collect::<Vec<_>>();
    assert_eq!(result_levels, ["warning", "error", "none"]);
    let result_messages = results
        .iter()
        .map(|result| {
            result["message"]["text"]
                .as_str()
                .expect("SARIF result message")
        })
        .collect::<Vec<_>>();
    assert_eq!(
        result_messages,
        ["early warning", "duplicate error", "late hint"]
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
fn diagnostic_order_uses_canonical_nested_labels() {
    let sources = fixture_sources();
    let diagnostics = vec![
        nested_labels_diagnostic(false, "z-first", "a-second"),
        nested_labels_diagnostic(false, "a-first", "z-second"),
    ];
    let reversed_labels = vec![
        nested_labels_diagnostic(true, "z-first", "a-second"),
        nested_labels_diagnostic(true, "a-first", "z-second"),
    ];
    let input = RenderInput::new(&sources, &diagnostics);
    let reversed_input = RenderInput::new(&sources, &reversed_labels);

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

    assert!(
        human.find("secondary: a-first").expect("a-first label")
            < human.find("secondary: z-first").expect("z-first label")
    );
    let document: Value = serde_json::from_str(&json).expect("renderer output is JSON");
    assert_eq!(
        document["diagnostics"][0]["secondary"][0]["note"],
        "a-first"
    );
    assert_eq!(
        document["diagnostics"][1]["secondary"][0]["note"],
        "z-first"
    );
}

#[test]
fn fix_edits_use_canonical_order_in_every_format() {
    let sources = fixture_sources();
    let diagnostics = vec![fix_edits_diagnostic(false)];
    let reversed_edits = vec![fix_edits_diagnostic(true)];
    let input = RenderInput::new(&sources, &diagnostics);
    let reversed_input = RenderInput::new(&sources, &reversed_edits);

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

    assert!(
        human
            .find("replace queries/α.pure:1:1")
            .expect("query edit")
            < human.find("replace models/β.pure:1:1").expect("model edit")
    );
    let document: Value = serde_json::from_str(&json).expect("renderer output is JSON");
    assert_eq!(document["diagnostics"][0]["fix"]["edits"][0]["file"], 0);
    assert_eq!(document["diagnostics"][0]["fix"]["edits"][1]["file"], 1);
    let log: Value = serde_json::from_str(&sarif).expect("renderer output is SARIF JSON");
    assert_eq!(
        log["runs"][0]["results"][0]["fixes"][0]["artifactChanges"][0]["artifactLocation"]["uri"],
        "queries/%CE%B1.pure"
    );
    assert_eq!(
        log["runs"][0]["results"][0]["fixes"][0]["artifactChanges"][1]["artifactLocation"]["uri"],
        "models/%CE%B2.pure"
    );
}

#[test]
fn diagnostic_order_uses_fix_provenance_after_title_and_applicability() {
    let sources = fixture_sources();
    let diagnostics = vec![
        ordered_fix_diagnostic(Applicability::Suggested, FixProvenance::SingleArityProven),
        ordered_fix_diagnostic(Applicability::Suggested, FixProvenance::ModelDependent),
        ordered_fix_diagnostic(Applicability::Suggested, FixProvenance::SyntaxOnly),
    ];

    let json = render_json(RenderInput::new(&sources, &diagnostics)).expect("JSON renders");
    let document: Value = serde_json::from_str(&json).expect("renderer output is JSON");
    let provenances = document["diagnostics"]
        .as_array()
        .expect("diagnostics array")
        .iter()
        .map(|diagnostic| {
            diagnostic["fix"]["provenance"]
                .as_str()
                .expect("fix provenance")
        })
        .collect::<Vec<_>>();

    assert_eq!(
        provenances,
        ["syntax_only", "model_dependent", "single_arity_proven"]
    );
}

#[test]
fn diagnostic_order_uses_fix_applicability_after_title() {
    let sources = fixture_sources();
    let diagnostics = vec![
        ordered_fix_diagnostic(Applicability::Unsafe, FixProvenance::SyntaxOnly),
        ordered_fix_diagnostic(Applicability::Suggested, FixProvenance::SyntaxOnly),
        ordered_fix_diagnostic(Applicability::MachineApplicable, FixProvenance::SyntaxOnly),
    ];

    let json = render_json(RenderInput::new(&sources, &diagnostics)).expect("JSON renders");
    let document: Value = serde_json::from_str(&json).expect("renderer output is JSON");
    let applicability = document["diagnostics"]
        .as_array()
        .expect("diagnostics array")
        .iter()
        .map(|diagnostic| {
            diagnostic["fix"]["applicability"]
                .as_str()
                .expect("fix applicability")
        })
        .collect::<Vec<_>>();

    assert_eq!(applicability, ["machine_applicable", "suggested", "unsafe"]);
}

#[test]
fn diagnostic_order_uses_fix_replacement_after_other_fix_metadata() {
    let sources = fixture_sources();
    let diagnostics = vec![edit_order_diagnostic("z"), edit_order_diagnostic("a")];

    let json = render_json(RenderInput::new(&sources, &diagnostics)).expect("JSON renders");
    let document: Value = serde_json::from_str(&json).expect("renderer output is JSON");
    let replacements = document["diagnostics"]
        .as_array()
        .expect("diagnostics array")
        .iter()
        .map(|diagnostic| {
            diagnostic["fix"]["edits"][0]["replacement"]
                .as_str()
                .expect("fix replacement")
        })
        .collect::<Vec<_>>();

    assert_eq!(replacements, ["a", "z"]);
}

#[test]
fn sarif_declares_unicode_code_point_columns_for_locations_and_fixes() {
    let sources =
        SourceStore::load([SourceInput::in_memory("unicode.pure", "aβγ\n")]).expect("source loads");
    let diagnostics = vec![
        Diagnostic::builder(
            DiagCode::BadToken,
            Severity::Error,
            "unicode token",
            Label::new(FileId::new(0), range(3, 5)),
        )
        .fix(Fix::model_dependent(
            "replace unicode token",
            vec![TextEdit {
                file: FileId::new(0),
                span: range(3, 5),
                new_text: "δ".to_owned(),
            }],
        ))
        .build(),
    ];

    let sarif = render_sarif(RenderInput::new(&sources, &diagnostics)).expect("SARIF renders");
    let log: Value = serde_json::from_str(&sarif).expect("renderer output is SARIF JSON");
    assert_eq!(log["runs"][0]["columnKind"], "unicodeCodePoints");

    assert_sarif_region(
        &log["runs"][0]["results"][0]["locations"][0]["physicalLocation"]["region"],
        (3, 1, 3),
        (5, 1, 4),
    );
    assert_sarif_region(
        &log["runs"][0]["results"][0]["fixes"][0]["artifactChanges"][0]["replacements"][0]["deletedRegion"],
        (3, 1, 3),
        (5, 1, 4),
    );
}

#[test]
fn sarif_encodes_arbitrary_display_names_as_uri_paths() {
    let sources = SourceStore::load([SourceInput::in_memory(r"C:\work space\α#%.pure", "α")])
        .expect("source loads");
    let diagnostics = vec![
        Diagnostic::builder(
            DiagCode::BadToken,
            Severity::Error,
            "escaped artifact name",
            Label::new(FileId::new(0), range(0, 2)),
        )
        .fix(Fix::model_dependent(
            "replace unicode token",
            vec![TextEdit {
                file: FileId::new(0),
                span: range(0, 2),
                new_text: "β".to_owned(),
            }],
        ))
        .build(),
    ];

    let sarif = render_sarif(RenderInput::new(&sources, &diagnostics)).expect("SARIF renders");
    let log: Value = serde_json::from_str(&sarif).expect("renderer output is SARIF JSON");
    let artifact_uri = "C%3A/work%20space/%CE%B1%23%25.pure";
    assert_eq!(
        log["runs"][0]["results"][0]["locations"][0]["physicalLocation"]["artifactLocation"]["uri"],
        artifact_uri
    );
    assert_eq!(
        log["runs"][0]["results"][0]["fixes"][0]["artifactChanges"][0]["artifactLocation"]["uri"],
        artifact_uri
    );
}

#[test]
fn empty_label_notes_remain_empty_in_every_format() {
    let sources = fixture_sources();
    let diagnostics = vec![
        Diagnostic::builder(
            DiagCode::BadToken,
            Severity::Error,
            "empty notes",
            Label::new(FileId::new(0), range(0, 3)),
        )
        .secondary(Label::new(FileId::new(1), range(0, 5)))
        .build(),
    ];
    let input = RenderInput::new(&sources, &diagnostics);

    let human = render_human(input, HumanOptions::default()).expect("human renders");
    let json = render_json(input).expect("json renders");
    let sarif = render_sarif(input).expect("sarif renders");

    assert!(!human.contains("secondary location"));
    let document: Value = serde_json::from_str(&json).expect("renderer output is JSON");
    assert_eq!(document["diagnostics"][0]["primary"]["note"], "");
    assert_eq!(document["diagnostics"][0]["secondary"][0]["note"], "");
    let log: Value = serde_json::from_str(&sarif).expect("renderer output is SARIF JSON");
    assert!(
        log["runs"][0]["results"][0]["locations"][0]
            .get("message")
            .is_none()
    );
    assert!(
        log["runs"][0]["results"][0]["relatedLocations"][0]
            .get("message")
            .is_none()
    );
}

#[test]
fn color_policy_respects_tty_and_controls_ansi_sequences() {
    let sources = fixture_sources();
    let diagnostics = vec![rich_diagnostic()];
    let input = RenderInput::new(&sources, &diagnostics);

    for (policy, is_terminal, has_color) in [
        (ColorPolicy::Auto, false, false),
        (ColorPolicy::Auto, true, true),
        (ColorPolicy::Always, false, true),
        (ColorPolicy::Never, true, false),
    ] {
        let options = policy.resolve(is_terminal);
        assert_eq!(options.color, has_color);
        let output = render_human(input, options).expect("human renders");
        assert_eq!(
            output,
            if has_color {
                include_str!("golden/rich.color.human")
            } else {
                include_str!("golden/rich.human")
            }
        );
        if has_color {
            assert!(output.contains("\x1b[1;31m"));
            assert!(output.contains("error[PUR2002]"));
            assert!(output.contains("\x1b[0m"));
        }
    }
}

#[test]
fn human_renderer_escapes_untrusted_terminal_controls_and_bidi_overrides() {
    let (sources, diagnostics) = terminal_control_fixture();
    let input = RenderInput::new(&sources, &diagnostics);

    let plain = render_human(input, ColorPolicy::Never.resolve(true)).expect("human renders");
    assert_plain_human_output_escapes_terminal_controls(&plain);

    let colored = render_human(input, ColorPolicy::Always.resolve(false)).expect("human renders");
    assert_colored_human_output_has_only_renderer_ansi(&colored);
}

fn terminal_control_fixture() -> (SourceStore, Vec<Diagnostic>) {
    let source_text = format!("prefix{TERMINAL_CONTROLS} {TERMINAL_CONTROL_TARGET}\n");
    let target_start = u32::try_from(
        source_text
            .find(TERMINAL_CONTROL_TARGET)
            .expect("fixture contains the highlighted target"),
    )
    .expect("fixture offset fits a diagnostic span");
    let target_end = target_start
        .checked_add(
            u32::try_from(TERMINAL_CONTROL_TARGET.len())
                .expect("target length fits a diagnostic span"),
        )
        .expect("fixture span fits in u32");
    let sources = SourceStore::load([SourceInput::in_memory(
        format!("input{TERMINAL_CONTROLS}.pure"),
        source_text,
    )])
    .expect("source loads");
    let diagnostics = vec![
        Diagnostic::builder(
            DiagCode::BadToken,
            Severity::Error,
            format!("message{TERMINAL_CONTROLS}"),
            Label::with_note(
                FileId::new(0),
                range(target_start, target_end),
                format!("note{TERMINAL_CONTROLS}"),
            ),
        )
        .fix(Fix::model_dependent(
            format!("fix{TERMINAL_CONTROLS}"),
            vec![TextEdit {
                file: FileId::new(0),
                span: range(target_start, target_end),
                new_text: format!("replacement\"\\{TERMINAL_CONTROLS}"),
            }],
        ))
        .verdict(Verdict::NotEquivalent {
            witness: format!("witness{TERMINAL_CONTROLS}"),
        })
        .url(format!("https://example.invalid/{TERMINAL_CONTROLS}"))
        .build(),
    ];

    (sources, diagnostics)
}

fn assert_plain_human_output_escapes_terminal_controls(plain: &str) {
    assert!(plain.contains(&format!("input{ESCAPED_TERMINAL_CONTROLS}.pure:")));
    assert!(plain.contains(&format!(
        "error[PUR0102]: message{ESCAPED_TERMINAL_CONTROLS}"
    )));
    assert!(plain.contains(&format!(
        "prefix{ESCAPED_TERMINAL_CONTROLS} {TERMINAL_CONTROL_TARGET}"
    )));
    assert!(plain.contains(&format!("primary: note{ESCAPED_TERMINAL_CONTROLS}")));
    assert!(plain.contains(&format!("= fix: fix{ESCAPED_TERMINAL_CONTROLS}")));
    assert!(plain.contains(&format!(
        "with \"replacement\\\"\\\\{ESCAPED_TERMINAL_CONTROLS}\""
    )));
    assert!(plain.contains(&format!(
        "not_equivalent; witness: witness{ESCAPED_TERMINAL_CONTROLS}"
    )));
    assert!(plain.contains(&format!(
        "docs: https://example.invalid/{ESCAPED_TERMINAL_CONTROLS}"
    )));
    let escaped_prefix = format!("prefix{ESCAPED_TERMINAL_CONTROLS} ");
    assert!(plain.contains(&format!(
        "      | {}{} primary: note{ESCAPED_TERMINAL_CONTROLS}",
        " ".repeat(escaped_prefix.chars().count()),
        "^".repeat(TERMINAL_CONTROL_TARGET.len())
    )));
    assert_plain_human_output_leaks_no_raw_terminal_or_bidi_characters(plain);
}

fn assert_plain_human_output_leaks_no_raw_terminal_or_bidi_characters(plain: &str) {
    assert!(!plain.contains('\x1b'));
    assert!(!plain.contains('\x07'));
    assert!(!plain.contains('\r'));
    assert!(!plain.contains('\u{202e}'), "raw RLO override leaked");
    assert!(!plain.contains('\u{2066}'), "raw LRI isolate leaked");
    assert!(!plain.contains('\u{200e}'), "raw LRM mark leaked");
    assert!(
        plain
            .chars()
            .all(|character| character == '\n' || !character.is_control()),
        "plain output must contain only renderer-owned line breaks: {plain:?}"
    );
    assert!(
        plain.chars().all(|character| !is_bidi_control(character)),
        "plain output must not contain raw Unicode Bidi_Control code points: {plain:?}"
    );
}

/// Independent (test-side) check for the Unicode `Bidi_Control` property —
/// deliberately not reusing the renderer's own `is_bidi_control`, so this
/// assertion verifies behavior from outside rather than trusting the same
/// classification the implementation uses.
fn is_bidi_control(character: char) -> bool {
    matches!(
        character,
        '\u{061c}'
            | '\u{200e}'..='\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2066}'..='\u{2069}'
    )
}

fn assert_colored_human_output_has_only_renderer_ansi(colored: &str) {
    assert_eq!(colored.matches('\x1b').count(), 2);
    assert!(colored.contains("\x1b[1;31m"));
    assert!(colored.contains("\x1b[0m"));
    assert!(
        colored.chars().all(|character| {
            character == '\n' || character == '\x1b' || !character.is_control()
        })
    );
    assert!(
        colored.chars().all(|character| !is_bidi_control(character)),
        "colored output must not contain raw Unicode Bidi_Control code points: {colored:?}"
    );
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
fn human_renderer_annotates_non_first_line_from_its_actual_start() {
    let sources = SourceStore::load([SourceInput::in_memory("lines.pure", "one\ntwo\n")])
        .expect("source loads");
    let diagnostics = vec![
        Diagnostic::builder(
            DiagCode::BadToken,
            Severity::Error,
            "second-line token",
            Label::with_note(FileId::new(0), range(4, 7), "second line"),
        )
        .build(),
    ];

    let output = render_human(
        RenderInput::new(&sources, &diagnostics),
        HumanOptions::default(),
    )
    .expect("second-line label renders");

    assert!(output.contains(
        "    --> lines.pure:2:1..2:4 (primary)\n      |\n    2 | two\n      | ^^^ primary: second line\n"
    ));
}

#[test]
fn eof_labels_render_at_the_start_of_the_final_empty_line_in_every_format() {
    let source_text = "let query =\n";
    let eof = u32::try_from(source_text.len()).expect("fixture fits a diagnostic span");
    let sources =
        SourceStore::load([SourceInput::in_memory("eof.pure", source_text)]).expect("source loads");
    let diagnostics = vec![
        Diagnostic::builder(
            DiagCode::MalformedSyntax,
            Severity::Error,
            "expected expression",
            Label::with_note(
                FileId::new(0),
                range(eof, eof),
                "expression is required here",
            ),
        )
        .build(),
    ];
    let input = RenderInput::new(&sources, &diagnostics);

    let human = render_human(input, HumanOptions::default()).expect("EOF label renders for humans");
    let json = render_json(input).expect("EOF label renders as JSON");
    let sarif = render_sarif(input).expect("EOF label renders as SARIF");

    assert!(human.contains("eof.pure:2:1..2:1 (primary)"));
    assert!(human.contains("^ primary: expression is required here"));

    let document: Value = serde_json::from_str(&json).expect("renderer output is JSON");
    assert_json_range(
        &document["diagnostics"][0]["primary"]["range"],
        (eof, 2, 1),
        (eof, 2, 1),
    );

    let log: Value = serde_json::from_str(&sarif).expect("renderer output is SARIF JSON");
    assert_sarif_region(
        &log["runs"][0]["results"][0]["locations"][0]["physicalLocation"]["region"],
        (eof, 2, 1),
        (eof, 2, 1),
    );
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
fn stale_primary_spans_report_exact_internal_errors_in_every_format() {
    let sources = SourceStore::load([SourceInput::in_memory("stale-primary.pure", "ok")])
        .expect("source loads");
    let diagnostics = vec![
        Diagnostic::builder(
            DiagCode::BadToken,
            Severity::Error,
            "stale primary",
            Label::new(FileId::new(0), range(3, 3)),
        )
        .build(),
    ];
    let input = RenderInput::new(&sources, &diagnostics);

    for result in all_formats(input) {
        assert!(matches!(
            result,
            Err(RenderError::InvalidSpan {
                diagnostic_index: 0,
                kind: SpanKind::Primary,
                file,
                start: 3,
                end: 3,
            }) if file == FileId::new(0)
        ));
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

#[test]
fn span_kind_display_identifies_every_diagnostic_role() {
    assert_eq!(SpanKind::Primary.to_string(), "primary label");
    assert_eq!(SpanKind::Secondary(2).to_string(), "secondary label #2");
    assert_eq!(SpanKind::FixEdit(3).to_string(), "fix edit #3");
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
