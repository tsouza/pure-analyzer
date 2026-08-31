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
const FORMAT_INPUT: &str = "model::Person . all ( )";
const FORMATTED_INPUT: &str = "model::Person.all()\n";
const RECOVERY_INPUT: &str = "[a,]";
const NAVIGATION_QUERY: &str = "model::Child.all()->filter(child| $child.inherited); model::Person.all()->filter(person| $person.manager.name; $person.missing)";
const FIX_QUERY: &str = "model::Source.all()->filter(x| $x.point(); $x.missing)";
const FIXED_QUERY: &str = "model::Source.all()->filter(x| $x.point(%latest); $x.missing)";

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
    fixture.write_bytes("broken.pure", b"\0");

    for renderer in RENDERERS {
        let output = run_deterministically(
            &fixture.root,
            &["validate", "broken.pure", "--format", renderer],
        );

        assert_eq!(output.status.code(), Some(EXIT_ACTIONABLE));
        assert!(!output.stdout.is_empty(), "{renderer} omitted diagnostics");
        assert!(
            output.stderr.is_empty(),
            "{renderer} wrote validation diagnostics to stderr"
        );
        assert_renderer_document(renderer, &output.stdout);
    }
}

#[test]
fn release_candidate_lint_matches_pmcd_and_pure_navigation_semantics() {
    let fixture = Fixture::new("release-candidate-lint");
    fixture.write("query.pure", NAVIGATION_QUERY);
    fixture.write("model.json", PMCD_NAVIGATION_MODEL);
    fixture.write("model.pure", PURE_NAVIGATION_MODEL);

    for renderer in RENDERERS {
        let pmcd = run_deterministically(
            &fixture.root,
            &[
                "lint",
                "query.pure",
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
                "query.pure",
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
        if renderer == "json" {
            assert_single_missing_member(&pmcd.stdout);
            assert_single_missing_member(&pure.stdout);
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
        assert_single_missing_member_for_renderer(renderer, &pmcd.stderr);
        assert_single_missing_member_for_renderer(renderer, &pure.stderr);
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

    let stdout = run_deterministically(&fixture.root, &["fmt", "query.pure", "--stdout"]);
    assert!(stdout.status.success());
    assert_eq!(stdout.stdout, FORMATTED_INPUT.as_bytes());
    assert!(stdout.stderr.is_empty());
    assert_eq!(fixture.read("query.pure"), FORMAT_INPUT);

    let diff = run_deterministically(&fixture.root, &["fmt", "query.pure", "--diff"]);
    assert_eq!(diff.status.code(), Some(EXIT_ACTIONABLE));
    assert!(String::from_utf8_lossy(&diff.stdout).contains("--- query.pure"));
    assert!(String::from_utf8_lossy(&diff.stdout).contains("+++ query.pure (formatted)"));
    assert!(diff.stderr.is_empty());
    assert_eq!(fixture.read("query.pure"), FORMAT_INPUT);

    assert_format_write_contract();

    for renderer in RENDERERS {
        let recovery = Fixture::new(&format!("release-candidate-format-recovery-{renderer}"));
        recovery.write("recovery.pure", RECOVERY_INPUT);
        let output = run_deterministically(
            &recovery.root,
            &["fmt", "recovery.pure", "--stdout", "--format", renderer],
        );

        assert_eq!(output.status.code(), Some(EXIT_ACTIONABLE));
        assert!(!output.stdout.is_empty());
        assert_renderer_document(renderer, &output.stderr);
        assert_eq!(recovery.read("recovery.pure"), RECOVERY_INPUT);
    }
}

fn assert_lint_fix_write_contract(label: &str, model_name: &str, model: &str) {
    let sequential = Fixture::new(&format!("release-candidate-lint-fix-{label}-sequential"));
    sequential.write("query.pure", FIX_QUERY);
    sequential.write(model_name, model);
    let sequential_output = run_with_jobs(
        &sequential.root,
        &[
            "lint",
            "query.pure",
            "--model",
            model_name,
            "--fix",
            "--quiet",
        ],
        "1",
    );

    let parallel = Fixture::new(&format!("release-candidate-lint-fix-{label}-parallel"));
    parallel.write("query.pure", FIX_QUERY);
    parallel.write(model_name, model);
    let parallel_output = run_with_jobs(
        &parallel.root,
        &[
            "lint",
            "query.pure",
            "--model",
            model_name,
            "--fix",
            "--quiet",
        ],
        "4",
    );

    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    {
        assert_eq!(sequential_output.status.code(), Some(EXIT_ACTIONABLE));
        assert_same_process_output(&sequential_output, &parallel_output);
        assert!(sequential_output.stdout.is_empty());
        assert!(sequential_output.stderr.is_empty());
        assert_eq!(sequential.read("query.pure"), FIXED_QUERY);
        assert_eq!(parallel.read("query.pure"), FIXED_QUERY);

        let repeated = run_with_jobs(
            &sequential.root,
            &[
                "lint",
                "query.pure",
                "--model",
                model_name,
                "--fix",
                "--quiet",
            ],
            "4",
        );
        assert_eq!(repeated.status.code(), Some(EXIT_ACTIONABLE));
        assert!(repeated.stdout.is_empty());
        assert!(repeated.stderr.is_empty());
        assert_eq!(sequential.read("query.pure"), FIXED_QUERY);
        sequential.assert_no_writer_artifacts();
        parallel.assert_no_writer_artifacts();
    }

    #[cfg(not(any(target_os = "linux", target_vendor = "apple")))]
    {
        assert_eq!(sequential_output.status.code(), Some(EXIT_INTERNAL));
        assert_same_process_output(&sequential_output, &parallel_output);
        assert_eq!(sequential.read("query.pure"), FIX_QUERY);
        assert_eq!(parallel.read("query.pure"), FIX_QUERY);
        sequential.assert_no_writer_artifacts();
        parallel.assert_no_writer_artifacts();
    }
}

fn assert_format_write_contract() {
    let sequential = Fixture::new("release-candidate-format-write-sequential");
    sequential.write("query.pure", FORMAT_INPUT);
    let sequential_output = run_with_jobs(&sequential.root, &["fmt", "query.pure"], "1");

    let parallel = Fixture::new("release-candidate-format-write-parallel");
    parallel.write("query.pure", FORMAT_INPUT);
    let parallel_output = run_with_jobs(&parallel.root, &["fmt", "query.pure"], "4");

    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    {
        assert!(sequential_output.status.success());
        assert_same_process_output(&sequential_output, &parallel_output);
        assert!(sequential_output.stdout.is_empty());
        assert!(sequential_output.stderr.is_empty());
        assert_eq!(sequential.read("query.pure"), FORMATTED_INPUT);
        assert_eq!(parallel.read("query.pure"), FORMATTED_INPUT);

        let repeated = run_with_jobs(&sequential.root, &["fmt", "query.pure"], "4");
        assert!(repeated.status.success());
        assert!(repeated.stdout.is_empty());
        assert!(repeated.stderr.is_empty());
        assert_eq!(sequential.read("query.pure"), FORMATTED_INPUT);
        sequential.assert_no_writer_artifacts();
        parallel.assert_no_writer_artifacts();
    }

    #[cfg(not(any(target_os = "linux", target_vendor = "apple")))]
    {
        assert_eq!(sequential_output.status.code(), Some(EXIT_INTERNAL));
        assert_same_process_output(&sequential_output, &parallel_output);
        assert_eq!(sequential.read("query.pure"), FORMAT_INPUT);
        assert_eq!(parallel.read("query.pure"), FORMAT_INPUT);
        sequential.assert_no_writer_artifacts();
        parallel.assert_no_writer_artifacts();
    }
}

fn assert_renderer_document(renderer: &str, bytes: &[u8]) {
    match renderer {
        "human" => assert!(!bytes.is_empty(), "human renderer omitted diagnostics"),
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

fn assert_single_missing_member(bytes: &[u8]) {
    let document: Value = serde_json::from_slice(bytes).expect("valid JSON lint output");
    let diagnostics = document["diagnostics"]
        .as_array()
        .expect("JSON diagnostics array");
    assert_eq!(
        diagnostics.len(),
        1,
        "model navigation emitted extra findings"
    );
    assert_eq!(diagnostics[0]["code"], "PUR2002");
    assert!(
        diagnostics[0]["message"]
            .as_str()
            .is_some_and(|message| message.contains("not declared")),
        "expected the deliberately missing member: {diagnostics:#?}"
    );
}

fn assert_single_missing_member_for_renderer(renderer: &str, bytes: &[u8]) {
    if renderer == "json" {
        assert_single_missing_member(bytes);
    } else {
        assert!(!bytes.is_empty());
    }
}

fn assert_equivalent_rendering(renderer: &str, left: &[u8], right: &[u8]) {
    if renderer == "human" {
        assert_eq!(left, right);
        return;
    }

    let mut left: Value = serde_json::from_slice(left).expect("valid left renderer output");
    let mut right: Value = serde_json::from_slice(right).expect("valid right renderer output");
    normalize_model_references(&mut left);
    normalize_model_references(&mut right);
    assert_eq!(
        left, right,
        "{renderer} differed beyond the model input path"
    );
}

fn normalize_model_references(value: &mut Value) {
    match value {
        Value::Array(values) => {
            for value in values {
                normalize_model_references(value);
            }
        }
        Value::Object(values) => {
            for value in values.values_mut() {
                normalize_model_references(value);
            }
        }
        Value::String(text) => {
            *text = text
                .replace("model.json", "model")
                .replace("model.pure", "model");
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

fn run_deterministically(root: &Path, arguments: &[&str]) -> Output {
    let sequential = run_with_jobs(root, arguments, "1");
    let parallel = run_with_jobs(root, arguments, "4");
    let repeated = run_with_jobs(root, arguments, "4");
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
