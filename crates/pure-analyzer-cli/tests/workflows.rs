#![forbid(unsafe_code)]
#![deny(missing_docs)]

//! Process-boundary coverage for v0.1 validation, linting, formatting, and completion workflows.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::Value;

const EXIT_ACTIONABLE: i32 = 1;
const EXIT_USAGE: i32 = 3;

#[test]
fn command_surface_completion_and_config_independence_are_stable() {
    let fixture = Fixture::new("help");
    let help = run(&fixture.root, &["--help"]);
    assert!(help.status.success());
    assert!(help.stderr.is_empty());
    let help = utf8(&help.stdout);
    for command in ["validate", "lint", "fmt", "completions"] {
        assert!(help.contains(command), "help omitted {command}: {help}");
    }
    for unavailable in ["eq", "diff", "explain"] {
        let output = run(&fixture.root, &[unavailable]);
        assert_eq!(output.status.code(), Some(EXIT_USAGE));
        assert!(output.stdout.is_empty());
        assert!(
            utf8(&output.stderr).contains("unrecognized subcommand"),
            "{unavailable} unexpectedly remains a supported command"
        );
    }

    let mut command = analyzer(&fixture.root);
    command
        .args(["completions", "bash"])
        .env("PURE_ANALYZER_JOBS", "invalid-on-purpose");
    let completion = command.output().expect("generate Bash completion");
    assert!(completion.status.success());
    assert!(completion.stderr.is_empty());
    assert_eq!(completion.stdout, include_bytes!("golden/completions.bash"));
}

#[test]
fn validate_stdin_renders_json_and_quiet_keeps_the_exit_result() {
    let fixture = Fixture::new("validate-stdin");
    let success = run_with_stdin(
        &fixture.root,
        &["validate", "-", "--format", "json", "--no-config"],
        "model::Person.all()",
    );
    assert!(success.status.success());
    assert!(success.stderr.is_empty());
    let document: Value = serde_json::from_slice(&success.stdout).expect("valid JSON output");
    assert_eq!(document["files"][0]["origin"], "stdin");
    assert_eq!(document["summary"]["errors"], 0);

    let finding = run_with_stdin(
        &fixture.root,
        &["validate", "-", "--format", "json", "--no-config"],
        "\0",
    );
    assert_eq!(finding.status.code(), Some(EXIT_ACTIONABLE));
    assert!(finding.stderr.is_empty());
    let document: Value = serde_json::from_slice(&finding.stdout).expect("valid JSON output");
    assert_eq!(document["diagnostics"][0]["code"], "PUR0102");
    assert_eq!(document["summary"]["errors"], 1);

    let quiet = run_with_stdin(
        &fixture.root,
        &[
            "validate",
            "-",
            "--format",
            "json",
            "--quiet",
            "--no-config",
        ],
        "\0",
    );
    assert_eq!(quiet.status.code(), Some(EXIT_ACTIONABLE));
    assert!(quiet.stdout.is_empty());
    assert!(quiet.stderr.is_empty());
}

#[test]
fn validate_warn_policy_changes_exit_and_severity_without_changing_code() {
    let fixture = Fixture::new("validate-warn");
    let warned = run_with_stdin(
        &fixture.root,
        &[
            "validate",
            "-",
            "--warn",
            "PUR0102",
            "--format",
            "json",
            "--no-config",
        ],
        "\0",
    );

    assert!(warned.status.success());
    assert!(warned.stderr.is_empty());
    let document: Value = serde_json::from_slice(&warned.stdout).expect("valid JSON output");
    assert_eq!(document["diagnostics"][0]["code"], "PUR0102");
    assert_eq!(document["diagnostics"][0]["severity"], "warning");
    assert_eq!(document["summary"]["errors"], 0);
    assert_eq!(document["summary"]["warnings"], 1);
    assert_eq!(document["summary"]["total"], 1);
}

#[test]
fn glob_order_and_parallel_output_are_deterministic() {
    let fixture = Fixture::new("glob-order");
    fixture.write("b.pure", "\0");
    fixture.write("a.pure", "\0");
    let arguments = |jobs| {
        vec![
            "validate",
            "*.pure",
            "--format",
            "json",
            "--jobs",
            jobs,
            "--no-config",
        ]
    };
    let sequential = run(&fixture.root, &arguments("1"));
    let parallel = run(&fixture.root, &arguments("4"));
    assert_eq!(sequential.status.code(), Some(EXIT_ACTIONABLE));
    assert_eq!(parallel.status.code(), Some(EXIT_ACTIONABLE));
    assert_eq!(sequential.stdout, parallel.stdout);
    let document: Value = serde_json::from_slice(&sequential.stdout).expect("valid JSON output");
    assert_eq!(document["files"][0]["name"], "a.pure");
    assert_eq!(document["files"][1]["name"], "b.pure");
}

#[test]
fn configuration_environment_and_cli_precedence_crosses_the_process_boundary() {
    let fixture = Fixture::new("config-precedence");
    fixture.write("query.pure", "\0");
    fixture.write(
        ".pure-analyzer.toml",
        "version = 1\njobs = 1\n\n[output]\nformat = \"human\"\ncolor = \"never\"\nquiet = false\n",
    );

    let mut environment = analyzer(&fixture.root);
    environment
        .args(["validate", "query.pure"])
        .env("PURE_ANALYZER_FORMAT", "sarif");
    let environment = environment.output().expect("run environment override");
    assert_eq!(environment.status.code(), Some(EXIT_ACTIONABLE));
    let document: Value = serde_json::from_slice(&environment.stdout).expect("valid SARIF output");
    assert_eq!(document["version"], "2.1.0");

    let mut cli = analyzer(&fixture.root);
    cli.args(["validate", "query.pure", "--format", "json"])
        .env("PURE_ANALYZER_FORMAT", "sarif");
    let cli = cli.output().expect("run CLI override");
    assert_eq!(cli.status.code(), Some(EXIT_ACTIONABLE));
    let document: Value = serde_json::from_slice(&cli.stdout).expect("valid JSON output");
    assert_eq!(document["version"], "1.0");
}

#[test]
fn human_color_choice_controls_process_output_bytes() {
    let fixture = Fixture::new("human-color");
    fixture.write("broken.pure", "\0");

    let always = run(
        &fixture.root,
        &[
            "validate",
            "broken.pure",
            "--format",
            "human",
            "--color",
            "always",
            "--no-config",
        ],
    );
    let never = run(
        &fixture.root,
        &[
            "validate",
            "broken.pure",
            "--format",
            "human",
            "--color",
            "never",
            "--no-config",
        ],
    );

    assert_eq!(always.status.code(), Some(EXIT_ACTIONABLE));
    assert_eq!(never.status.code(), Some(EXIT_ACTIONABLE));
    assert!(always.stderr.is_empty());
    assert!(never.stderr.is_empty());
    assert!(always.stdout.windows(2).any(|bytes| bytes == b"\x1b["));
    assert!(!never.stdout.windows(2).any(|bytes| bytes == b"\x1b["));
    assert_ne!(always.stdout, never.stdout);
}

#[test]
fn print_config_dominates_completion_generation() {
    let fixture = Fixture::new("print-config-completions");
    let output = run(
        &fixture.root,
        &["--print-config", "completions", "bash", "--no-config"],
    );

    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let document = utf8(&output.stdout)
        .parse::<toml::Table>()
        .expect("resolved configuration TOML");
    assert_eq!(document["version"].as_integer(), Some(1));
    assert!(!utf8(&output.stdout).contains("_pure_analyzer"));
}

#[test]
fn lint_loads_repeatable_models_and_applies_severity_policy() {
    let fixture = Fixture::new("lint-model");
    fixture.write("query.pure", "model::Person.all()->filter(x| $x.missing)");
    fixture.write("model.json", &person_model());
    fixture.write("extra.pure", "Class model::Extra {}\n");

    let finding = run(
        &fixture.root,
        &[
            "lint",
            "query.pure",
            "--model",
            "model.json",
            "--model",
            "extra.pure",
            "--format",
            "json",
            "--no-config",
        ],
    );
    assert_eq!(finding.status.code(), Some(EXIT_ACTIONABLE));
    let document: Value = serde_json::from_slice(&finding.stdout).expect("valid JSON output");
    assert!(
        document["diagnostics"]
            .as_array()
            .is_some_and(|diagnostics| {
                diagnostics
                    .iter()
                    .any(|diagnostic| diagnostic["code"] == "PUR2002")
            })
    );

    let warned = run(
        &fixture.root,
        &[
            "lint",
            "query.pure",
            "--model",
            "model.json",
            "--warn",
            "PUR2002",
            "--format",
            "json",
            "--no-config",
        ],
    );
    assert!(warned.status.success());
    let document: Value = serde_json::from_slice(&warned.stdout).expect("valid JSON output");
    assert_eq!(document["diagnostics"][0]["code"], "PUR2002");
    assert_eq!(document["diagnostics"][0]["severity"], "warning");
    assert_eq!(document["summary"]["errors"], 0);
    assert_eq!(document["summary"]["warnings"], 1);
    assert_eq!(document["summary"]["total"], 1);

    let ignored = run(
        &fixture.root,
        &[
            "lint",
            "query.pure",
            "--model",
            "model.json",
            "--ignore",
            "PUR2002",
            "--format",
            "json",
            "--no-config",
        ],
    );
    assert!(ignored.status.success());
    let document: Value = serde_json::from_slice(&ignored.stdout).expect("valid JSON output");
    assert_eq!(document["summary"]["total"], 0);
}

#[test]
fn lint_model_merge_warning_can_be_denied_without_changing_its_code() {
    let fixture = Fixture::new("lint-model-policy");
    fixture.write("query.pure", "model::Person.all()");
    let model = person_model();
    fixture.write("first.json", &model);
    fixture.write("second.json", &model);
    let arguments = |deny: bool| {
        let mut arguments = vec![
            "lint",
            "query.pure",
            "--model",
            "first.json",
            "--model",
            "second.json",
        ];
        if deny {
            arguments.extend(["--deny", "PUR9000"]);
        }
        arguments.extend(["--format", "json", "--no-config"]);
        arguments
    };

    let default = run(&fixture.root, &arguments(false));
    assert!(default.status.success());
    let document: Value = serde_json::from_slice(&default.stdout).expect("valid JSON output");
    assert_eq!(document["diagnostics"][0]["code"], "PUR9000");
    assert_eq!(document["diagnostics"][0]["severity"], "warning");
    assert_eq!(document["summary"]["errors"], 0);
    assert_eq!(document["summary"]["warnings"], 1);
    assert_eq!(document["summary"]["total"], 1);

    let denied = run(&fixture.root, &arguments(true));
    assert_eq!(denied.status.code(), Some(EXIT_ACTIONABLE));
    let document: Value = serde_json::from_slice(&denied.stdout).expect("valid JSON output");
    assert_eq!(document["diagnostics"][0]["code"], "PUR9000");
    assert_eq!(document["diagnostics"][0]["severity"], "error");
    assert_eq!(document["summary"]["errors"], 1);
    assert_eq!(document["summary"]["warnings"], 0);
    assert_eq!(document["summary"]["total"], 1);
}

#[test]
fn lint_fix_with_a_model_is_safe_when_no_applicable_fix_exists() {
    let fixture = Fixture::new("lint-fix");
    let query = "model::Person.all()->filter(x| $x.missing)";
    fixture.write("query.pure", query);
    fixture.write("model.json", &person_model());

    let output = run(
        &fixture.root,
        &[
            "lint",
            "query.pure",
            "--model",
            "model.json",
            "--fix",
            "--quiet",
            "--no-config",
        ],
    );
    assert_eq!(output.status.code(), Some(EXIT_ACTIONABLE));
    assert!(output.stdout.is_empty());
    assert_eq!(fixture.read("query.pure"), query);
}

#[test]
fn formatter_check_diff_stdout_and_atomic_write_have_distinct_behavior() {
    let fixture = Fixture::new("format-modes");
    let original = "model::Person . all ( )";
    fixture.write("query.pure", original);

    let check = run(
        &fixture.root,
        &["fmt", "query.pure", "--check", "--no-config"],
    );
    assert_eq!(check.status.code(), Some(EXIT_ACTIONABLE));
    assert!(check.stdout.is_empty());
    assert_eq!(fixture.read("query.pure"), original);

    let diff = run(
        &fixture.root,
        &["fmt", "query.pure", "--diff", "--no-config"],
    );
    assert_eq!(diff.status.code(), Some(EXIT_ACTIONABLE));
    assert!(utf8(&diff.stdout).contains("--- query.pure"));
    assert!(utf8(&diff.stdout).contains("+++ query.pure (formatted)"));
    assert_eq!(fixture.read("query.pure"), original);

    let stdout = run(
        &fixture.root,
        &["fmt", "query.pure", "--stdout", "--no-config"],
    );
    assert!(stdout.status.success());
    let formatted = utf8(&stdout.stdout).to_owned();
    assert_ne!(formatted, original);
    assert_eq!(fixture.read("query.pure"), original);

    let write = run(&fixture.root, &["fmt", "query.pure", "--no-config"]);
    assert!(write.status.success());
    assert!(write.stdout.is_empty());
    assert_eq!(fixture.read("query.pure"), formatted);
    assert!(
        fixture
            .entries()
            .iter()
            .all(|name| !name.contains("pure-analyzer-tmp")
                && !name.contains("pure-analyzer-backup"))
    );
}

#[test]
fn formatter_stdout_requires_one_resolved_input_and_preserves_stable_text() {
    let fixture = Fixture::new("format-stdout-inputs");
    let stable = "model::Person.all()\n";
    fixture.write("a.pure", stable);
    fixture.write("b.pure", stable);

    let multiple = run(&fixture.root, &["fmt", "*.pure", "--stdout", "--no-config"]);
    assert_eq!(multiple.status.code(), Some(EXIT_USAGE));
    assert!(multiple.stdout.is_empty());
    assert!(utf8(&multiple.stderr).contains("fmt --stdout requires exactly one resolved input"));
    assert_eq!(fixture.read("a.pure"), stable);
    assert_eq!(fixture.read("b.pure"), stable);

    let one = run(&fixture.root, &["fmt", "a.pure", "--stdout", "--no-config"]);
    assert!(one.status.success());
    assert!(one.stderr.is_empty());
    assert_eq!(one.stdout, stable.as_bytes());
    assert_eq!(fixture.read("a.pure"), stable);
}

#[test]
fn formatter_diff_is_exact_ordered_and_never_writes() {
    let fixture = Fixture::new("format-diff-order");
    let a_before = "model::Person . all ( )";
    let b_before = "model::Order . all ( )";
    fixture.write("b.pure", b_before);
    fixture.write("a.pure", a_before);
    let arguments = |jobs| vec!["fmt", "*.pure", "--diff", "--jobs", jobs, "--no-config"];
    let expected = concat!(
        "--- a.pure\n",
        "+++ a.pure (formatted)\n",
        "-model::Person . all ( )\n",
        "+model::Person.all()\n",
        "--- b.pure\n",
        "+++ b.pure (formatted)\n",
        "-model::Order . all ( )\n",
        "+model::Order.all()\n",
    );

    let sequential = run(&fixture.root, &arguments("1"));
    let parallel = run(&fixture.root, &arguments("4"));
    assert_eq!(sequential.status.code(), Some(EXIT_ACTIONABLE));
    assert_eq!(parallel.status.code(), Some(EXIT_ACTIONABLE));
    assert_eq!(sequential.stdout, expected.as_bytes());
    assert_eq!(parallel.stdout, expected.as_bytes());
    assert!(sequential.stderr.is_empty());
    assert!(parallel.stderr.is_empty());
    assert_eq!(fixture.read("a.pure"), a_before);
    assert_eq!(fixture.read("b.pure"), b_before);
}

#[test]
fn formatter_line_width_uses_config_and_accepts_a_cli_override() {
    let fixture = Fixture::new("format-width");
    fixture.write(
        "query.pure",
        "function(firstArgument,secondArgument,thirdArgument)",
    );
    fixture.write(
        ".pure-analyzer.toml",
        "version = 1\n\n[fmt]\nline-width = 30\n",
    );

    let configured = run(&fixture.root, &["fmt", "query.pure", "--stdout"]);
    assert!(configured.status.success());
    assert_eq!(
        utf8(&configured.stdout),
        "function(firstArgument,\n        secondArgument,\n        thirdArgument)\n"
    );

    let overridden = run(
        &fixture.root,
        &["fmt", "query.pure", "--stdout", "--line-width", "80"],
    );
    assert!(overridden.status.success());
    assert_eq!(
        utf8(&overridden.stdout),
        "function(firstArgument, secondArgument, thirdArgument)\n"
    );
}

#[test]
fn formatter_line_width_environment_applies_with_no_config() {
    let fixture = Fixture::new("format-width-environment");
    fixture.write(
        "query.pure",
        "function(firstArgument,secondArgument,thirdArgument)",
    );

    let mut command = analyzer(&fixture.root);
    command
        .args(["fmt", "query.pure", "--stdout", "--no-config"])
        .env("PURE_ANALYZER_FMT_LINE_WIDTH", "30");
    let output = command.output().expect("run environment width override");

    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    assert_eq!(
        utf8(&output.stdout),
        "function(firstArgument,\n        secondArgument,\n        thirdArgument)\n"
    );
}

#[test]
fn formatter_stdin_is_machine_clean_and_parse_errors_never_write() {
    let fixture = Fixture::new("format-stdin-error");
    let stdin = run_with_stdin(
        &fixture.root,
        &["fmt", "-", "--no-config"],
        "model::Person . all ( )",
    );
    assert!(stdin.status.success());
    assert!(!stdin.stdout.is_empty());
    assert!(stdin.stderr.is_empty());
    let stable = run_with_stdin(
        &fixture.root,
        &["fmt", "-", "--no-config"],
        utf8(&stdin.stdout),
    );
    assert!(stable.status.success());
    assert_eq!(stable.stdout, stdin.stdout);

    let check = run_with_stdin(
        &fixture.root,
        &["fmt", "-", "--check", "--no-config"],
        utf8(&stdin.stdout),
    );
    assert!(check.status.success());
    assert!(check.stdout.is_empty());

    let diff = run_with_stdin(
        &fixture.root,
        &["fmt", "-", "--diff", "--no-config"],
        "model::Person . all ( )",
    );
    assert_eq!(diff.status.code(), Some(EXIT_ACTIONABLE));
    assert!(utf8(&diff.stdout).contains("--- <stdin>"));

    fixture.write("broken.pure", "a  +  \0");
    let before = fixture.read("broken.pure");
    let broken = run(&fixture.root, &["fmt", "broken.pure", "--no-config"]);
    assert_eq!(broken.status.code(), Some(EXIT_ACTIONABLE));
    assert!(broken.stdout.is_empty());
    assert!(utf8(&broken.stderr).contains("PUR0102"));
    assert_eq!(fixture.read("broken.pure"), before);
}

#[test]
fn duplicate_standard_input_is_a_usage_error_without_normal_output() {
    let fixture = Fixture::new("duplicate-stdin");
    let output = run_with_stdin(
        &fixture.root,
        &["validate", "-", "-", "--no-config"],
        "model::Person.all()",
    );

    assert_eq!(output.status.code(), Some(EXIT_USAGE));
    assert!(output.stdout.is_empty());
    assert!(utf8(&output.stderr).contains("standard input may be supplied only once"));
}

#[test]
fn formatter_applies_global_policy_to_recovery_diagnostics_on_stderr() {
    let fixture = Fixture::new("format-policy-diagnostic");
    fixture.write("broken.pure", "\0");

    let output = run(
        &fixture.root,
        &[
            "fmt",
            "broken.pure",
            "--warn",
            "PUR0102",
            "--format",
            "json",
            "--no-config",
        ],
    );

    assert!(output.status.success());
    assert!(output.stdout.is_empty());
    let document: Value = serde_json::from_slice(&output.stderr)
        .expect("formatter recovery diagnostics are valid JSON on stderr");
    assert_eq!(document["diagnostics"][0]["code"], "PUR0102");
    assert_eq!(document["diagnostics"][0]["severity"], "warning");
    assert_eq!(document["summary"]["errors"], 0);
    assert_eq!(document["summary"]["warnings"], 1);
    assert_eq!(document["summary"]["total"], 1);
}

#[test]
fn formatter_never_partially_writes_when_an_input_has_recovery_diagnostics() {
    let fixture = Fixture::new("format-atomic-recovery");
    let valid_before = "model::Person . all ( )";
    let broken_before = "\0";
    fixture.write("valid.pure", valid_before);
    fixture.write("broken.pure", broken_before);

    let output = run(
        &fixture.root,
        &["fmt", "valid.pure", "broken.pure", "--no-config"],
    );

    assert_eq!(output.status.code(), Some(EXIT_ACTIONABLE));
    assert!(output.stdout.is_empty());
    assert!(utf8(&output.stderr).contains("PUR0102"));
    assert_eq!(fixture.read("valid.pure"), valid_before);
    assert_eq!(fixture.read("broken.pure"), broken_before);
}

#[test]
fn formatter_recovery_blocks_every_atomic_write_after_a_policy_downgrade() {
    let fixture = Fixture::new("format-policy-atomic");
    let valid_before = "model::Person . all ( )";
    let broken_before = "\0";
    fixture.write("valid.pure", valid_before);
    fixture.write("broken.pure", broken_before);

    let output = run(
        &fixture.root,
        &[
            "fmt",
            "valid.pure",
            "broken.pure",
            "--warn",
            "PUR0102",
            "--no-config",
        ],
    );

    assert!(output.status.success());
    assert!(output.stdout.is_empty());
    assert_eq!(fixture.read("valid.pure"), valid_before);
    assert_eq!(fixture.read("broken.pure"), broken_before);
}

#[test]
fn usage_and_model_failures_use_exit_three_without_stdout() {
    let fixture = Fixture::new("usage-model");
    let missing = run(&fixture.root, &["validate", "missing.pure", "--no-config"]);
    assert_eq!(missing.status.code(), Some(EXIT_USAGE));
    assert!(missing.stdout.is_empty());
    assert!(utf8(&missing.stderr).contains("missing.pure"));

    fixture.write("query.pure", "model::Person.all()");
    fixture.write("model.json", "not JSON");
    let model = run(
        &fixture.root,
        &["lint", "query.pure", "--model", "model.json", "--no-config"],
    );
    assert_eq!(model.status.code(), Some(EXIT_USAGE));
    assert!(model.stdout.is_empty());
    assert!(utf8(&model.stderr).contains("could not load model"));

    let parse = run(&fixture.root, &["validate", "--unknown-option"]);
    assert_eq!(parse.status.code(), Some(EXIT_USAGE));
    assert!(parse.stdout.is_empty());
}

fn run(root: &Path, arguments: &[&str]) -> Output {
    analyzer(root)
        .args(arguments)
        .output()
        .expect("run pure-analyzer")
}

fn run_with_stdin(root: &Path, arguments: &[&str], stdin: &str) -> Output {
    let mut child = analyzer(root)
        .args(arguments)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn pure-analyzer");
    child
        .stdin
        .take()
        .expect("piped stdin")
        .write_all(stdin.as_bytes())
        .expect("write process stdin");
    child.wait_with_output().expect("wait for pure-analyzer")
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

fn person_model() -> String {
    r#"{"_type":"data","elements":[{"_type":"class","package":"model","name":"Person","stereotypes":[],"superTypes":[],"properties":[{"name":"name","genericType":{"rawType":"String","typeArguments":[]},"multiplicity":{"lowerBound":0,"upperBound":1}}],"qualifiedProperties":[]}]}"#.to_owned()
}

fn utf8(bytes: &[u8]) -> &str {
    std::str::from_utf8(bytes).expect("UTF-8 process output")
}

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new(label: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "pure-analyzer-cli-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).expect("create fixture directory");
        Self { root }
    }

    fn write(&self, relative: &str, text: &str) {
        fs::write(self.root.join(relative), text).expect("write fixture");
    }

    fn read(&self, relative: &str) -> String {
        fs::read_to_string(self.root.join(relative)).expect("read fixture")
    }

    fn entries(&self) -> Vec<String> {
        fs::read_dir(&self.root)
            .expect("read fixture directory")
            .map(|entry| {
                entry
                    .expect("read fixture entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
