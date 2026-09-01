#![forbid(unsafe_code)]
#![deny(missing_docs)]

//! Release-candidate process-boundary contracts for the v0.1 CLI.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::Value;

const EXIT_ACTIONABLE: i32 = 1;
#[cfg(not(any(target_os = "linux", target_vendor = "apple")))]
const EXIT_INTERNAL: i32 = 4;
const RENDERERS: [&str; 3] = ["human", "json", "sarif"];
const SEQUENTIAL_JOBS: &str = "1";
const PARALLEL_JOBS: &str = "4";
const FORMAT_INPUT: &str = "model::Person . all ( )";
const FORMATTED_INPUT: &str = "model::Person.all()\n";
const SECOND_FORMAT_INPUT: &str = "model::Order . all ( )";
const SECOND_FORMATTED_INPUT: &str = "model::Order.all()\n";
const FORMAT_DIFF: &str = concat!(
    "--- query.pure\n",
    "+++ query.pure (formatted)\n",
    "-model::Person . all ( )\n",
    "+model::Person.all()\n",
    "--- second.pure\n",
    "+++ second.pure (formatted)\n",
    "-model::Order . all ( )\n",
    "+model::Order.all()\n",
);
const RECOVERY_INPUT: &str = "[a,]";
const FORMATTED_RECOVERY_INPUT: &str = "[a,]\n";
const BAD_TOKEN_CODE: &str = "PUR0102";
const BAD_TOKEN_MESSAGE: &str = "unrecognized token";
const MALFORMED_SYNTAX_CODE: &str = "PUR1200";
const RECOVERY_DIAGNOSTIC_MESSAGE: &str = "expected an expression after `,`";
const MISSING_MEMBER_CODE: &str = "PUR2002";
const MISSING_MEMBER_MESSAGE: &str = "not declared";
const FIRST_BROKEN_FILE: &str = "first-broken.pure";
const SECOND_BROKEN_FILE: &str = "second-broken.pure";
const INHERITANCE_FILE: &str = "inheritance.pure";
const ASSOCIATION_FILE: &str = "association.pure";
const VALIDATE_DIAGNOSTIC_COUNT: usize = 2;
const NAVIGATION_MISSING_MEMBER_COUNT: usize = 2;
const FIX_MISSING_MEMBER_COUNT: usize = 1;
const PMCD_MODEL_SUFFIX: &str = "model.json";
const PURE_MODEL_SUFFIX: &str = "model.pure";
const NORMALIZED_MODEL_NAME_FIELD: &str = "\"name\": \"model\"";
const INHERITANCE_QUERY: &str =
    "model::Child.all()->filter(child| $child.inherited; $child.missing)";
const ASSOCIATION_QUERY: &str =
    "model::Person.all()->filter(person| $person.manager.name; $person.missing)";
const FIX_QUERY: &str = "model::Source.all()->filter(x| $x.point(); $x.missing)";
const FIXED_QUERY: &str = "model::Source.all()->filter(x| $x.point(%latest); $x.missing)";
const SECOND_FIX_QUERY: &str = "model::Source.all()->filter(x| $x.point(/* second */); $x.missing)";
const SECOND_FIXED_QUERY: &str =
    "model::Source.all()->filter(x| $x.point(/* second */%latest); $x.missing)";

const PMCD_NAVIGATION_MODEL: &str = r#"{
  "_type": "data",
  "elements": [
    {
      "_type": "class",
      "package": "model",
      "name": "Base",
      "stereotypes": [],
      "superTypes": [],
      "properties": [{
        "name": "inherited",
        "genericType": {"rawType": "String", "typeArguments": []},
        "multiplicity": {"lowerBound": 0, "upperBound": 1}
      }],
      "qualifiedProperties": []
    },
    {
      "_type": "class",
      "package": "model",
      "name": "Child",
      "stereotypes": [],
      "superTypes": ["model::Base"],
      "properties": [],
      "qualifiedProperties": []
    },
    {
      "_type": "class",
      "package": "model",
      "name": "Person",
      "stereotypes": [],
      "superTypes": [],
      "properties": [],
      "qualifiedProperties": []
    },
    {
      "_type": "class",
      "package": "model",
      "name": "Manager",
      "stereotypes": [],
      "superTypes": [],
      "properties": [{
        "name": "name",
        "genericType": {"rawType": "String", "typeArguments": []},
        "multiplicity": {"lowerBound": 0, "upperBound": 1}
      }],
      "qualifiedProperties": []
    },
    {
      "_type": "association",
      "package": "model",
      "name": "Person_Manager",
      "stereotypes": [],
      "properties": [
        {
          "name": "manager",
          "genericType": {"rawType": "model::Manager", "typeArguments": []},
          "multiplicity": {"lowerBound": 0, "upperBound": 1}
        },
        {
          "name": "reports",
          "genericType": {"rawType": "model::Person", "typeArguments": []},
          "multiplicity": {"lowerBound": 0, "upperBound": null}
        }
      ]
    }
  ]
}"#;

const PURE_NAVIGATION_MODEL: &str = r#"
Class model::Base
{
  inherited: String[0..1];
}

Class model::Child extends model::Base
{
}

Class model::Person
{
}

Class model::Manager
{
  name: String[0..1];
}

Association model::Person_Manager
{
  manager: model::Manager[0..1];
  reports: model::Person[*];
}
"#;

const PMCD_FIX_MODEL: &str = r#"{
  "_type": "data",
  "elements": [
    {
      "_type": "class",
      "package": "model",
      "name": "TemporalTarget",
      "stereotypes": [{
        "profile": "meta::pure::profiles::temporal",
        "value": "processingtemporal"
      }],
      "superTypes": [],
      "properties": [],
      "qualifiedProperties": []
    },
    {
      "_type": "class",
      "package": "model",
      "name": "Source",
      "stereotypes": [],
      "superTypes": [],
      "properties": [],
      "qualifiedProperties": [{
        "name": "point",
        "returnGenericType": {"rawType": "model::TemporalTarget", "typeArguments": []},
        "returnMultiplicity": {"lowerBound": 0, "upperBound": 1},
        "stereotypes": [{
          "profile": "meta::pure::profiles::milestoning",
          "value": "generatedmilestoningproperty"
        }],
        "parameters": []
      }]
    }
  ]
}"#;

const PURE_FIX_MODEL: &str = r#"
Class <<temporal.processingtemporal>> model::TemporalTarget
{
}

Class model::Source
{
  <<milestoning.generatedmilestoningproperty>>
  point(): model::TemporalTarget[0..1] {};
}
"#;

#[test]
fn release_candidate_validate_renderers_are_process_deterministic() {
    let fixture = Fixture::new("release-candidate-validate");
    fixture.write_bytes(FIRST_BROKEN_FILE, b"\0");
    fixture.write(SECOND_BROKEN_FILE, RECOVERY_INPUT);

    for renderer in RENDERERS {
        let output = run_deterministically(
            &fixture.root,
            &[
                "validate",
                FIRST_BROKEN_FILE,
                SECOND_BROKEN_FILE,
                "--format",
                renderer,
            ],
        );

        assert_eq!(output.status.code(), Some(EXIT_ACTIONABLE));
        assert!(!output.stdout.is_empty(), "{renderer} omitted diagnostics");
        assert!(
            output.stderr.is_empty(),
            "{renderer} wrote validation diagnostics to stderr"
        );
        assert_renderer_document(renderer, &output.stdout);
        assert_renderer_finding(renderer, &output.stdout, BAD_TOKEN_CODE, BAD_TOKEN_MESSAGE);
        assert_renderer_finding(
            renderer,
            &output.stdout,
            MALFORMED_SYNTAX_CODE,
            RECOVERY_DIAGNOSTIC_MESSAGE,
        );
        assert_renderer_diagnostic_count(renderer, &output.stdout, VALIDATE_DIAGNOSTIC_COUNT);
        assert_renderer_mentions_file(renderer, &output.stdout, FIRST_BROKEN_FILE);
        assert_renderer_mentions_file(renderer, &output.stdout, SECOND_BROKEN_FILE);
    }
}

#[test]
fn release_candidate_lint_matches_pmcd_and_pure_navigation_semantics() {
    let fixture = Fixture::new("release-candidate-lint");
    fixture.write(INHERITANCE_FILE, INHERITANCE_QUERY);
    fixture.write(ASSOCIATION_FILE, ASSOCIATION_QUERY);
    fixture.write("model.json", PMCD_NAVIGATION_MODEL);
    fixture.write("model.pure", PURE_NAVIGATION_MODEL);

    for renderer in RENDERERS {
        let pmcd = run_deterministically(
            &fixture.root,
            &[
                "lint",
                INHERITANCE_FILE,
                ASSOCIATION_FILE,
                "--model",
                "model.json",
                "--format",
                renderer,
            ],
        );
        let pure = run_deterministically(
            &fixture.root,
            &[
                "lint",
                INHERITANCE_FILE,
                ASSOCIATION_FILE,
                "--model",
                "model.pure",
                "--format",
                renderer,
            ],
        );

        assert_eq!(pmcd.status.code(), Some(EXIT_ACTIONABLE));
        assert_eq!(pure.status.code(), Some(EXIT_ACTIONABLE));
        assert!(pmcd.stderr.is_empty());
        assert!(pure.stderr.is_empty());
        assert_renderer_document(renderer, &pmcd.stdout);
        assert_renderer_document(renderer, &pure.stdout);
        assert_equivalent_rendering(renderer, &pmcd.stdout, &pure.stdout);
        assert_missing_member_findings_for_renderer(
            renderer,
            &pmcd.stdout,
            NAVIGATION_MISSING_MEMBER_COUNT,
        );
        assert_missing_member_findings_for_renderer(
            renderer,
            &pure.stdout,
            NAVIGATION_MISSING_MEMBER_COUNT,
        );
        for file in [INHERITANCE_FILE, ASSOCIATION_FILE] {
            assert_renderer_mentions_file(renderer, &pmcd.stdout, file);
            assert_renderer_mentions_file(renderer, &pure.stdout, file);
        }
    }
}

#[test]
fn release_candidate_lint_fix_matches_models_and_persists_deterministically() {
    let previews = Fixture::new("release-candidate-lint-fix-preview");
    previews.write("query.pure", FIX_QUERY);
    previews.write("model.json", PMCD_FIX_MODEL);
    previews.write("model.pure", PURE_FIX_MODEL);

    for renderer in RENDERERS {
        let pmcd = run_deterministically(
            &previews.root,
            &[
                "lint",
                "query.pure",
                "--model",
                "model.json",
                "--fix",
                "--stdout",
                "--format",
                renderer,
            ],
        );
        let pure = run_deterministically(
            &previews.root,
            &[
                "lint",
                "query.pure",
                "--model",
                "model.pure",
                "--fix",
                "--stdout",
                "--format",
                renderer,
            ],
        );

        assert_eq!(pmcd.status.code(), Some(EXIT_ACTIONABLE));
        assert_eq!(pure.status.code(), Some(EXIT_ACTIONABLE));
        assert_eq!(pmcd.stdout, FIXED_QUERY.as_bytes());
        assert_eq!(pure.stdout, FIXED_QUERY.as_bytes());
        assert_renderer_document(renderer, &pmcd.stderr);
        assert_renderer_document(renderer, &pure.stderr);
        assert_equivalent_rendering(renderer, &pmcd.stderr, &pure.stderr);
        assert_missing_member_findings_for_renderer(
            renderer,
            &pmcd.stderr,
            FIX_MISSING_MEMBER_COUNT,
        );
        assert_missing_member_findings_for_renderer(
            renderer,
            &pure.stderr,
            FIX_MISSING_MEMBER_COUNT,
        );
        assert_eq!(previews.read("query.pure"), FIX_QUERY);
    }

    for (label, model_name, model) in [
        ("pmcd", "model.json", PMCD_FIX_MODEL),
        ("pure", "model.pure", PURE_FIX_MODEL),
    ] {
        assert_lint_fix_write_contract(label, model_name, model);
    }
}

#[test]
fn release_candidate_formatter_keeps_write_and_recovery_contracts() {
    let fixture = Fixture::new("release-candidate-format");
    fixture.write("query.pure", FORMAT_INPUT);
    fixture.write("second.pure", SECOND_FORMAT_INPUT);
    assert_format_preview_contract(&fixture);
    assert_format_write_contract();

    for renderer in RENDERERS {
        assert_format_recovery_contract(renderer);
    }
}

fn assert_format_preview_contract(fixture: &Fixture) {
    let stdout = run_deterministically(&fixture.root, &["fmt", "query.pure", "--stdout"]);
    assert!(stdout.status.success());
    assert_eq!(stdout.stdout, FORMATTED_INPUT.as_bytes());
    assert!(stdout.stderr.is_empty());
    assert_eq!(fixture.read("query.pure"), FORMAT_INPUT);

    let diff = run_deterministically(
        &fixture.root,
        &["fmt", "query.pure", "second.pure", "--diff"],
    );
    assert_eq!(diff.status.code(), Some(EXIT_ACTIONABLE));
    assert_eq!(diff.stdout, FORMAT_DIFF.as_bytes());
    assert!(diff.stderr.is_empty());
    assert_eq!(fixture.read("query.pure"), FORMAT_INPUT);
    assert_eq!(fixture.read("second.pure"), SECOND_FORMAT_INPUT);
}

fn assert_format_recovery_contract(renderer: &str) {
    let recovery = Fixture::new(&format!("release-candidate-format-recovery-{renderer}"));
    recovery.write("recovery.pure", RECOVERY_INPUT);
    let output = run_deterministically(
        &recovery.root,
        &["fmt", "recovery.pure", "--stdout", "--format", renderer],
    );

    assert_eq!(output.status.code(), Some(EXIT_ACTIONABLE));
    assert_eq!(output.stdout, FORMATTED_RECOVERY_INPUT.as_bytes());
    assert_renderer_document(renderer, &output.stderr);
    assert_renderer_finding(
        renderer,
        &output.stderr,
        MALFORMED_SYNTAX_CODE,
        RECOVERY_DIAGNOSTIC_MESSAGE,
    );
    assert_eq!(recovery.read("recovery.pure"), RECOVERY_INPUT);
}

fn assert_lint_fix_write_contract(label: &str, model_name: &str, model: &str) {
    let fixture = Fixture::new(&format!("release-candidate-lint-fix-{label}"));
    fixture.write("first.pure", FIX_QUERY);
    fixture.write("second.pure", SECOND_FIX_QUERY);
    fixture.write(model_name, model);
    let sequential_output = run_with_jobs(
        &fixture.root,
        &[
            "lint",
            "first.pure",
            "second.pure",
            "--model",
            model_name,
            "--fix",
            "--format",
            "json",
        ],
        SEQUENTIAL_JOBS,
    );
    let sequential_first = fixture.read("first.pure");
    let sequential_second = fixture.read("second.pure");

    fixture.write("first.pure", FIX_QUERY);
    fixture.write("second.pure", SECOND_FIX_QUERY);
    let parallel_output = run_with_jobs(
        &fixture.root,
        &[
            "lint",
            "first.pure",
            "second.pure",
            "--model",
            model_name,
            "--fix",
            "--format",
            "json",
        ],
        PARALLEL_JOBS,
    );

    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    {
        assert_eq!(sequential_output.status.code(), Some(EXIT_ACTIONABLE));
        assert_same_process_output(&sequential_output, &parallel_output);
        assert!(sequential_output.stdout.is_empty());
        assert_renderer_document("json", &sequential_output.stderr);
        assert_renderer_code("json", &sequential_output.stderr, MISSING_MEMBER_CODE);
        assert_eq!(sequential_first, FIXED_QUERY);
        assert_eq!(sequential_second, SECOND_FIXED_QUERY);
        assert_eq!(fixture.read("first.pure"), FIXED_QUERY);
        assert_eq!(fixture.read("second.pure"), SECOND_FIXED_QUERY);

        let repeated = run_with_jobs(
            &fixture.root,
            &[
                "lint",
                "first.pure",
                "second.pure",
                "--model",
                model_name,
                "--fix",
                "--format",
                "json",
            ],
            PARALLEL_JOBS,
        );
        assert_same_process_output(&sequential_output, &repeated);
        assert_eq!(fixture.read("first.pure"), FIXED_QUERY);
        assert_eq!(fixture.read("second.pure"), SECOND_FIXED_QUERY);
        fixture.assert_no_writer_artifacts();
    }

    #[cfg(not(any(target_os = "linux", target_vendor = "apple")))]
    {
        assert_eq!(sequential_output.status.code(), Some(EXIT_INTERNAL));
        assert_same_process_output(&sequential_output, &parallel_output);
        assert_eq!(sequential_first, FIX_QUERY);
        assert_eq!(sequential_second, SECOND_FIX_QUERY);
        assert_eq!(fixture.read("first.pure"), FIX_QUERY);
        assert_eq!(fixture.read("second.pure"), SECOND_FIX_QUERY);
        fixture.assert_no_writer_artifacts();
    }
}

fn assert_format_write_contract() {
    let fixture = Fixture::new("release-candidate-format-write");
    fixture.write("first.pure", FORMAT_INPUT);
    fixture.write("second.pure", SECOND_FORMAT_INPUT);
    let sequential_output = run_with_jobs(
        &fixture.root,
        &["fmt", "first.pure", "second.pure"],
        SEQUENTIAL_JOBS,
    );
    let sequential_first = fixture.read("first.pure");
    let sequential_second = fixture.read("second.pure");

    fixture.write("first.pure", FORMAT_INPUT);
    fixture.write("second.pure", SECOND_FORMAT_INPUT);
    let parallel_output = run_with_jobs(
        &fixture.root,
        &["fmt", "first.pure", "second.pure"],
        PARALLEL_JOBS,
    );

    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    {
        assert!(sequential_output.status.success());
        assert_same_process_output(&sequential_output, &parallel_output);
        assert!(sequential_output.stdout.is_empty());
        assert!(sequential_output.stderr.is_empty());
        assert_eq!(sequential_first, FORMATTED_INPUT);
        assert_eq!(sequential_second, SECOND_FORMATTED_INPUT);
        assert_eq!(fixture.read("first.pure"), FORMATTED_INPUT);
        assert_eq!(fixture.read("second.pure"), SECOND_FORMATTED_INPUT);

        let repeated = run_with_jobs(
            &fixture.root,
            &["fmt", "first.pure", "second.pure"],
            PARALLEL_JOBS,
        );
        assert_same_process_output(&sequential_output, &repeated);
        assert_eq!(fixture.read("first.pure"), FORMATTED_INPUT);
        assert_eq!(fixture.read("second.pure"), SECOND_FORMATTED_INPUT);
        fixture.assert_no_writer_artifacts();
    }

    #[cfg(not(any(target_os = "linux", target_vendor = "apple")))]
    {
        assert_eq!(sequential_output.status.code(), Some(EXIT_INTERNAL));
        assert_same_process_output(&sequential_output, &parallel_output);
        assert_eq!(sequential_first, FORMAT_INPUT);
        assert_eq!(sequential_second, SECOND_FORMAT_INPUT);
        assert_eq!(fixture.read("first.pure"), FORMAT_INPUT);
        assert_eq!(fixture.read("second.pure"), SECOND_FORMAT_INPUT);
        fixture.assert_no_writer_artifacts();
    }
}

fn assert_renderer_document(renderer: &str, bytes: &[u8]) {
    match renderer {
        "human" => assert!(
            !utf8(bytes).is_empty(),
            "human renderer omitted diagnostics"
        ),
        "json" => {
            let document: Value =
                serde_json::from_slice(bytes).expect("valid JSON renderer output");
            assert_eq!(document["version"], "1.0");
        }
        "sarif" => {
            let document: Value =
                serde_json::from_slice(bytes).expect("valid SARIF renderer output");
            assert_eq!(document["version"], "2.1.0");
        }
        _ => panic!("unsupported renderer {renderer}"),
    }
}

fn assert_renderer_code(renderer: &str, bytes: &[u8], code: &str) {
    match renderer {
        "human" => assert!(utf8(bytes).contains(code), "human output omitted {code}"),
        "json" => {
            let document: Value =
                serde_json::from_slice(bytes).expect("valid JSON renderer output");
            assert!(
                json_diagnostics(&document)
                    .iter()
                    .any(|diagnostic| diagnostic["code"] == code),
                "JSON output omitted {code}: {document:#?}"
            );
        }
        "sarif" => {
            let document: Value =
                serde_json::from_slice(bytes).expect("valid SARIF renderer output");
            assert!(
                sarif_results(&document)
                    .iter()
                    .any(|result| result["ruleId"] == code),
                "SARIF output omitted {code}: {document:#?}"
            );
        }
        _ => panic!("unsupported renderer {renderer}"),
    }
}

fn assert_renderer_finding(renderer: &str, bytes: &[u8], code: &str, message: &str) {
    assert_renderer_code(renderer, bytes, code);
    match renderer {
        "human" => assert!(
            utf8(bytes).contains(message),
            "human output omitted {message}"
        ),
        "json" => {
            let document: Value =
                serde_json::from_slice(bytes).expect("valid JSON renderer output");
            assert!(
                json_diagnostics(&document).iter().any(|diagnostic| {
                    diagnostic["code"] == code
                        && diagnostic["message"]
                            .as_str()
                            .is_some_and(|actual| actual.contains(message))
                }),
                "JSON output omitted {code}: {document:#?}"
            );
        }
        "sarif" => {
            let document: Value =
                serde_json::from_slice(bytes).expect("valid SARIF renderer output");
            assert!(
                sarif_results(&document).iter().any(|result| {
                    result["ruleId"] == code
                        && result["message"]["text"]
                            .as_str()
                            .is_some_and(|actual| actual.contains(message))
                }),
                "SARIF output omitted {code}: {document:#?}"
            );
        }
        _ => panic!("unsupported renderer {renderer}"),
    }
}

fn assert_renderer_diagnostic_count(renderer: &str, bytes: &[u8], expected: usize) {
    match renderer {
        "human" => assert_eq!(utf8(bytes).matches("error[").count(), expected),
        "json" => {
            let document: Value =
                serde_json::from_slice(bytes).expect("valid JSON renderer output");
            assert_eq!(json_diagnostics(&document).len(), expected);
        }
        "sarif" => {
            let document: Value =
                serde_json::from_slice(bytes).expect("valid SARIF renderer output");
            assert_eq!(sarif_results(&document).len(), expected);
        }
        _ => panic!("unsupported renderer {renderer}"),
    }
}

fn assert_renderer_mentions_file(renderer: &str, bytes: &[u8], file: &str) {
    assert!(
        utf8(bytes).contains(file),
        "{renderer} output omitted {file}"
    );
}

fn assert_missing_member_findings_for_renderer(renderer: &str, bytes: &[u8], expected: usize) {
    assert_renderer_finding(renderer, bytes, MISSING_MEMBER_CODE, MISSING_MEMBER_MESSAGE);
    assert_renderer_diagnostic_count(renderer, bytes, expected);
    match renderer {
        "human" => assert_eq!(utf8(bytes).matches(MISSING_MEMBER_CODE).count(), expected),
        "json" => {
            let document: Value =
                serde_json::from_slice(bytes).expect("valid JSON renderer output");
            assert!(json_diagnostics(&document).iter().all(|diagnostic| {
                diagnostic["code"] == MISSING_MEMBER_CODE
                    && diagnostic["message"]
                        .as_str()
                        .is_some_and(|message| message.contains(MISSING_MEMBER_MESSAGE))
            }));
        }
        "sarif" => {
            let document: Value =
                serde_json::from_slice(bytes).expect("valid SARIF renderer output");
            let results = sarif_results(&document);
            assert!(results.iter().all(|result| {
                result["ruleId"] == MISSING_MEMBER_CODE
                    && result["message"]["text"]
                        .as_str()
                        .is_some_and(|message| message.contains(MISSING_MEMBER_MESSAGE))
            }));
        }
        _ => panic!("unsupported renderer {renderer}"),
    }
}

fn assert_equivalent_rendering(renderer: &str, left: &[u8], right: &[u8]) {
    match renderer {
        "human" | "sarif" => assert_eq!(left, right),
        "json" => assert_eq!(
            normalized_json_model_file_name(left, PMCD_MODEL_SUFFIX),
            normalized_json_model_file_name(right, PURE_MODEL_SUFFIX),
            "JSON differed beyond the model input name"
        ),
        _ => panic!("unsupported renderer {renderer}"),
    }
}

fn normalized_json_model_file_name(bytes: &[u8], model_suffix: &str) -> String {
    let text = utf8(bytes);
    let document: Value = serde_json::from_str(text).expect("valid JSON renderer output");
    let model_name = document["files"]
        .as_array()
        .expect("JSON source files")
        .iter()
        .filter_map(|file| file["name"].as_str())
        .filter(|name| name.ends_with(model_suffix))
        .collect::<Vec<_>>();
    assert_eq!(
        model_name.len(),
        1,
        "JSON model manifest must contain one model source"
    );
    let model_name = model_name[0];
    let model_name_field = format!("\"name\": \"{model_name}\"");
    assert_eq!(
        text.matches(&model_name_field).count(),
        1,
        "JSON model manifest must render its model path once"
    );
    text.replacen(&model_name_field, NORMALIZED_MODEL_NAME_FIELD, 1)
}

fn json_diagnostics(document: &Value) -> &[Value] {
    document["diagnostics"]
        .as_array()
        .expect("JSON diagnostics array")
}

fn sarif_results(document: &Value) -> &[Value] {
    document["runs"]
        .as_array()
        .and_then(|runs| runs.first())
        .and_then(|run| run["results"].as_array())
        .expect("SARIF results array")
}

fn utf8(bytes: &[u8]) -> &str {
    std::str::from_utf8(bytes).expect("UTF-8 process output")
}

fn run_deterministically(root: &Path, arguments: &[&str]) -> Output {
    let sequential = run_with_jobs(root, arguments, SEQUENTIAL_JOBS);
    let parallel = run_with_jobs(root, arguments, PARALLEL_JOBS);
    let repeated = run_with_jobs(root, arguments, PARALLEL_JOBS);
    assert_same_process_output(&sequential, &parallel);
    assert_same_process_output(&parallel, &repeated);
    sequential
}

fn run_with_jobs(root: &Path, arguments: &[&str], jobs: &str) -> Output {
    let mut arguments = arguments.to_vec();
    arguments.extend(["--jobs", jobs, "--no-config"]);
    run(root, &arguments)
}

fn assert_same_process_output(left: &Output, right: &Output) {
    assert_eq!(left.status.code(), right.status.code());
    assert_eq!(left.stdout, right.stdout);
    assert_eq!(left.stderr, right.stderr);
}

fn run(root: &Path, arguments: &[&str]) -> Output {
    analyzer(root)
        .args(arguments)
        .output()
        .expect("run pure-analyzer")
}

fn analyzer(root: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_pure-analyzer"));
    command.current_dir(root).env_remove("RUST_LOG");
    for name in [
        "PURE_ANALYZER_JOBS",
        "PURE_ANALYZER_FORMAT",
        "PURE_ANALYZER_COLOR",
        "PURE_ANALYZER_QUIET",
        "PURE_ANALYZER_SELECT",
        "PURE_ANALYZER_IGNORE",
        "PURE_ANALYZER_DENY",
        "PURE_ANALYZER_WARN",
        "PURE_ANALYZER_STRICT",
        "PURE_ANALYZER_FMT_LINE_WIDTH",
        "PURE_ANALYZER_MODEL_PATHS",
    ] {
        command.env_remove(name);
    }
    command
}

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new(label: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "pure-analyzer-release-candidate-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).expect("create fixture directory");
        Self { root }
    }

    fn write(&self, relative: &str, text: &str) {
        self.write_bytes(relative, text.as_bytes());
    }

    fn write_bytes(&self, relative: &str, bytes: &[u8]) {
        fs::write(self.root.join(relative), bytes).expect("write fixture");
    }

    fn read(&self, relative: &str) -> String {
        fs::read_to_string(self.root.join(relative)).expect("read fixture")
    }

    fn assert_no_writer_artifacts(&self) {
        for entry in fs::read_dir(&self.root).expect("read fixture directory") {
            let entry = entry.expect("read fixture entry");
            assert!(
                !entry
                    .file_name()
                    .to_string_lossy()
                    .contains("pure-analyzer"),
                "writer artifact remained at {}",
                entry.path().display()
            );
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
