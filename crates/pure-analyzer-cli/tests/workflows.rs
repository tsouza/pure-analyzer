#![forbid(unsafe_code)]
#![deny(missing_docs)]

//! Process-boundary coverage for v0.1 validation, linting, formatting, explain, and completion workflows.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use libpure::ExplainContent;
use serde_json::Value;

const EXIT_ACTIONABLE: i32 = 1;
const EXIT_INDECISIVE: i32 = 2;
#[cfg(not(any(target_os = "linux", target_vendor = "apple")))]
const EXIT_INTERNAL: i32 = 4;
const EXIT_USAGE: i32 = 3;
const FORMATTER_VALID_SOURCE: &str = "model::Person . all ( )";
const FORMATTER_BROKEN_SOURCE: &str = "\0";

#[test]
fn command_surface_completion_and_config_independence_are_stable() {
    let fixture = Fixture::new("help");
    let help = run(&fixture.root, &["--help"]);
    assert!(help.status.success());
    assert!(help.stderr.is_empty());
    let help = utf8(&help.stdout);
    for command in [
        "validate",
        "lint",
        "fmt",
        "eq",
        "diff",
        "explain",
        "completions",
    ] {
        assert!(help.contains(command), "help omitted {command}: {help}");
    }
    assert!(
        help.contains("transactional in-place file updates"),
        "fmt help omitted its write contract: {help}"
    );
    let comparison_help = run(&fixture.root, &["eq", "--help"]);
    assert!(comparison_help.status.success());
    assert!(comparison_help.stderr.is_empty());
    assert!(
        utf8(&comparison_help.stdout)
            .contains("Exit status: 0 equivalent; 1 structurally not equivalent; 2 indecisive."),
        "comparison help must document the unified result codes"
    );

    let mut command = analyzer(&fixture.root);
    command
        .args(["completions", "bash"])
        .env("PURE_ANALYZER_JOBS", "invalid-on-purpose");
    let completion = command.output().expect("generate Bash completion");
    assert!(completion.status.success());
    assert!(completion.stderr.is_empty());
    assert_eq!(
        completion.stdout,
        include_bytes!("golden/completions.bash.golden")
    );
}

#[test]
fn explain_returns_shared_content_in_human_and_json_without_mixing_streams() {
    let fixture = Fixture::new("explain");

    for identifier in ["PUR2001", "IND_WINDOW"] {
        let content = libpure::explain(identifier).expect("registered explain content");
        let human = run(
            &fixture.root,
            &["explain", content.identifier, "--no-config"],
        );
        assert!(human.status.success());
        assert!(human.stderr.is_empty());
        assert_eq!(utf8(&human.stdout), human_explanation(content));

        let json = run(
            &fixture.root,
            &[
                "explain",
                content.identifier,
                "--format",
                "json",
                "--no-config",
            ],
        );
        assert!(json.status.success());
        assert!(json.stderr.is_empty());
        let expected = format!(
            "{}\n",
            serde_json::to_string_pretty(content).expect("serialize shared explain content")
        );
        assert_eq!(json.stdout, expected.as_bytes());
        let document: Value =
            serde_json::from_slice(&json.stdout).expect("valid JSON explain output");
        assert_eq!(document["identifier"], content.identifier);
        assert_eq!(document["kind"], content.kind.as_str());
    }
}

#[test]
fn explain_rejects_unknown_identifiers_and_sarif_as_usage_errors() {
    let fixture = Fixture::new("explain-errors");
    let unknown = run(&fixture.root, &["explain", "pur2001", "--no-config"]);
    assert_eq!(unknown.status.code(), Some(EXIT_USAGE));
    assert!(unknown.stdout.is_empty());
    let error = libpure::explain("pur2001").expect_err("unknown explain identifier");
    assert_eq!(utf8(&unknown.stderr), format!("error: {error}\n"));

    let sarif = run(
        &fixture.root,
        &["explain", "PUR2001", "--format", "sarif", "--no-config"],
    );
    assert_eq!(sarif.status.code(), Some(EXIT_USAGE));
    assert!(sarif.stdout.is_empty());
    assert_eq!(
        utf8(&sarif.stderr),
        "error: explain supports only --format human or --format json\n"
    );
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

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
#[test]
fn lint_fix_applies_a_real_single_file_change() {
    let (fixture, _, fixed) = lint_fix_fixture("lint-fix-single");

    let applied = run(
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

    assert!(applied.status.success());
    assert!(applied.stdout.is_empty());
    assert!(applied.stderr.is_empty());
    assert_eq!(fixture.read("query.pure"), fixed);
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
#[test]
fn lint_fix_applies_real_multi_file_changes_and_is_idempotent() {
    let fixture = Fixture::new("lint-fix-write");
    let first = "model::Source.all()->filter(x| $x.point())";
    let second = "model::Source.all()->filter(x| $x.point(/* second */))";
    fixture.write("first.pure", first);
    fixture.write("second.pure", second);
    fixture.write("model.json", &milestoning_model());

    let first_apply = run(
        &fixture.root,
        &[
            "lint",
            "first.pure",
            "second.pure",
            "--model",
            "model.json",
            "--fix",
            "--quiet",
            "--no-config",
        ],
    );
    assert!(first_apply.status.success());
    assert!(first_apply.stdout.is_empty());
    assert!(first_apply.stderr.is_empty());
    assert_eq!(
        fixture.read("first.pure"),
        "model::Source.all()->filter(x| $x.point(%latest))"
    );
    assert_eq!(
        fixture.read("second.pure"),
        "model::Source.all()->filter(x| $x.point(/* second */%latest))"
    );

    let repeated = run(
        &fixture.root,
        &[
            "lint",
            "first.pure",
            "second.pure",
            "--model",
            "model.json",
            "--fix",
            "--quiet",
            "--no-config",
        ],
    );
    assert!(repeated.status.success());
    assert!(repeated.stdout.is_empty());
    assert!(repeated.stderr.is_empty());
}

#[cfg(not(any(target_os = "linux", target_vendor = "apple")))]
#[test]
fn lint_fix_fails_closed_without_changing_source() {
    let (fixture, query, _) = lint_fix_fixture("lint-fix-unsupported-exchange");

    let applied = run(
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

    assert_eq!(applied.status.code(), Some(EXIT_INTERNAL));
    assert!(applied.stdout.is_empty());
    assert!(utf8(&applied.stderr).contains("atomic file exchange is unavailable on this platform"));
    assert_eq!(fixture.read("query.pure"), query);
    fixture.assert_no_writer_artifacts();
}

#[test]
fn lint_fix_check_reports_pending_changes_without_writing() {
    let (fixture, query, _) = lint_fix_fixture("lint-fix-check");

    let check = run(
        &fixture.root,
        &[
            "lint",
            "query.pure",
            "--model",
            "model.json",
            "--fix",
            "--check",
            "--quiet",
            "--no-config",
        ],
    );
    assert_eq!(check.status.code(), Some(EXIT_ACTIONABLE));
    assert!(check.stdout.is_empty());
    assert!(check.stderr.is_empty());
    assert_eq!(fixture.read("query.pure"), query);

    let warned_check = run(
        &fixture.root,
        &[
            "lint",
            "query.pure",
            "--model",
            "model.json",
            "--fix",
            "--check",
            "--warn",
            "PUR2001",
            "--quiet",
            "--no-config",
        ],
    );
    assert_eq!(warned_check.status.code(), Some(EXIT_ACTIONABLE));
    assert!(warned_check.stdout.is_empty());
    assert!(warned_check.stderr.is_empty());
    assert_eq!(fixture.read("query.pure"), query);
}

#[test]
fn lint_fix_diff_preserves_files_and_routes_diagnostics_to_stderr() {
    let (fixture, query, _) = lint_fix_fixture("lint-fix-diff");

    let diff = run(
        &fixture.root,
        &[
            "lint",
            "query.pure",
            "--model",
            "model.json",
            "--fix",
            "--diff",
            "--format",
            "json",
            "--no-config",
        ],
    );
    assert_eq!(diff.status.code(), Some(EXIT_ACTIONABLE));
    assert!(utf8(&diff.stdout).contains("--- query.pure"));
    assert!(utf8(&diff.stdout).contains("+++ query.pure (fixed)"));
    let diagnostics: Value = serde_json::from_slice(&diff.stderr).expect("JSON diagnostics");
    assert_eq!(diagnostics["diagnostics"][0]["code"], "PUR2001");
    assert_eq!(fixture.read("query.pure"), query);
}

#[test]
fn lint_fix_stdout_is_machine_clean_and_reflects_the_fixed_snapshot() {
    let (fixture, query, fixed) = lint_fix_fixture("lint-fix-stdout");

    let stdout = run(
        &fixture.root,
        &[
            "lint",
            "query.pure",
            "--model",
            "model.json",
            "--fix",
            "--stdout",
            "--format",
            "json",
            "--no-config",
        ],
    );
    assert!(stdout.status.success());
    assert_eq!(stdout.stdout, fixed.as_bytes());
    let diagnostics: Value = serde_json::from_slice(&stdout.stderr).expect("JSON diagnostics");
    assert_eq!(diagnostics["summary"]["errors"], 0);
    assert_eq!(diagnostics["diagnostics"], Value::Array(Vec::new()));
    let files = diagnostics["files"].as_array().expect("JSON source files");
    assert!(
        files
            .iter()
            .any(|file| { file["name"] == "query.pure" && file["origin"] == "file" })
    );
    assert!(files.iter().any(|file| {
        file["name"]
            .as_str()
            .is_some_and(|name| name.ends_with("model.json"))
            && file["origin"] == "file"
    }));
    assert_eq!(fixture.read("query.pure"), query);
}

#[test]
fn lint_fix_has_a_deterministic_standard_input_policy() {
    let fixture = Fixture::new("lint-fix-stdin");
    let query = "model::Source.all()->filter(x| $x.point())";
    let fixed = "model::Source.all()->filter(x| $x.point(%latest))";
    fixture.write("model.json", &milestoning_model());

    let write = run_with_stdin(
        &fixture.root,
        &["lint", "-", "--model", "model.json", "--fix", "--no-config"],
        query,
    );
    assert_eq!(write.status.code(), Some(EXIT_USAGE));
    assert!(write.stdout.is_empty());
    assert!(utf8(&write.stderr).contains("cannot update standard input"));

    let preview = run_with_stdin(
        &fixture.root,
        &[
            "lint",
            "-",
            "--model",
            "model.json",
            "--fix",
            "--stdout",
            "--quiet",
            "--no-config",
        ],
        query,
    );
    assert!(preview.status.success());
    assert_eq!(preview.stdout, fixed.as_bytes());
    assert!(preview.stderr.is_empty());
}

#[test]
fn formatter_read_only_modes_and_transactional_write_have_distinct_behavior() {
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

    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    {
        let write = run(&fixture.root, &["fmt", "query.pure", "--no-config"]);
        assert!(write.status.success());
        assert!(write.stdout.is_empty());
        assert!(write.stderr.is_empty());
        assert_eq!(fixture.read("query.pure"), formatted);
        fixture.assert_no_writer_artifacts();
    }

    #[cfg(not(any(target_os = "linux", target_vendor = "apple")))]
    {
        let write = run(&fixture.root, &["fmt", "query.pure", "--no-config"]);
        assert_eq!(write.status.code(), Some(EXIT_INTERNAL));
        assert!(write.stdout.is_empty());
        assert!(
            utf8(&write.stderr).contains("atomic file exchange is unavailable on this platform")
        );
        assert_eq!(fixture.read("query.pure"), original);
        fixture.assert_no_writer_artifacts();
    }
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
#[test]
fn formatter_default_write_updates_multiple_files_and_is_idempotent() {
    let fixture = Fixture::new("format-write-multiple");
    fixture.write("first.pure", "model::Person . all ( )");
    fixture.write("second.pure", "model::Order . all ( )");

    let apply = run(
        &fixture.root,
        &["fmt", "first.pure", "second.pure", "--no-config"],
    );
    assert!(apply.status.success());
    assert!(apply.stdout.is_empty());
    assert!(apply.stderr.is_empty());
    assert_eq!(fixture.read("first.pure"), "model::Person.all()\n");
    assert_eq!(fixture.read("second.pure"), "model::Order.all()\n");
    fixture.assert_no_writer_artifacts();

    let repeated = run(
        &fixture.root,
        &["fmt", "first.pure", "second.pure", "--no-config"],
    );
    assert!(repeated.status.success());
    assert!(repeated.stdout.is_empty());
    assert!(repeated.stderr.is_empty());
    assert_eq!(fixture.read("first.pure"), "model::Person.all()\n");
    assert_eq!(fixture.read("second.pure"), "model::Order.all()\n");
    fixture.assert_no_writer_artifacts();
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
fn formatter_default_stdin_is_machine_clean_and_idempotent() {
    let fixture = Fixture::new("format-stdin-default");
    let source = "model::Person . all ( )";
    let formatted = run_with_stdin(&fixture.root, &["fmt", "-", "--no-config"], source);
    assert!(formatted.status.success());
    assert_eq!(formatted.stdout, b"model::Person.all()\n");
    assert!(formatted.stderr.is_empty());

    let stable = run_with_stdin(
        &fixture.root,
        &["fmt", "-", "--no-config"],
        utf8(&formatted.stdout),
    );
    assert!(stable.status.success());
    assert_eq!(stable.stdout, formatted.stdout);
    assert!(stable.stderr.is_empty());
}

#[test]
fn formatter_stdin_stdout_is_machine_clean_and_idempotent() {
    let fixture = Fixture::new("format-stdin-stdout");
    let source = "model::Person . all ( )";
    let stdout = run_with_stdin(
        &fixture.root,
        &["fmt", "-", "--stdout", "--no-config"],
        source,
    );
    assert!(stdout.status.success());
    assert!(!stdout.stdout.is_empty());
    assert!(stdout.stderr.is_empty());
    let stable = run_with_stdin(
        &fixture.root,
        &["fmt", "-", "--stdout", "--no-config"],
        utf8(&stdout.stdout),
    );
    assert!(stable.status.success());
    assert_eq!(stable.stdout, stdout.stdout);
}

#[test]
fn formatter_stdin_check_and_diff_are_read_only() {
    let fixture = Fixture::new("format-stdin-read-only");
    let check = run_with_stdin(
        &fixture.root,
        &["fmt", "-", "--check", "--no-config"],
        "model::Person.all()\n",
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
}

#[test]
fn formatter_check_recovery_preserves_file_input() {
    let fixture = Fixture::new("format-check-recovery");
    fixture.write("broken.pure", "a  +  \0");
    let before = fixture.read("broken.pure");
    let broken = run(
        &fixture.root,
        &["fmt", "broken.pure", "--check", "--no-config"],
    );
    assert_eq!(broken.status.code(), Some(EXIT_ACTIONABLE));
    assert!(broken.stdout.is_empty());
    assert!(utf8(&broken.stderr).contains("PUR0102"));
    assert_eq!(fixture.read("broken.pure"), before);
}

#[test]
fn formatter_default_mode_rejects_mixed_file_and_stdin() {
    let fixture = Fixture::new("format-mixed-inputs");
    let source = "model::Person . all ( )";
    fixture.write("query.pure", source);
    let mixed = run_with_stdin(
        &fixture.root,
        &["fmt", "query.pure", "-", "--no-config"],
        source,
    );
    assert_eq!(mixed.status.code(), Some(EXIT_USAGE));
    assert!(mixed.stdout.is_empty());
    assert!(
        utf8(&mixed.stderr).contains("cannot combine standard input with in-place file writes")
    );
    assert_eq!(fixture.read("query.pure"), source);
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
            "--stdout",
            "--warn",
            "PUR0102",
            "--format",
            "json",
            "--no-config",
        ],
    );

    assert!(output.status.success());
    assert!(!output.stdout.is_empty());
    let document: Value = serde_json::from_slice(&output.stderr)
        .expect("formatter recovery diagnostics are valid JSON on stderr");
    assert_eq!(document["diagnostics"][0]["code"], "PUR0102");
    assert_eq!(document["diagnostics"][0]["severity"], "warning");
    assert_eq!(document["summary"]["errors"], 0);
    assert_eq!(document["summary"]["warnings"], 1);
    assert_eq!(document["summary"]["total"], 1);
}

#[test]
fn formatter_recovery_blocks_every_default_write_even_when_the_diagnostic_is_hidden() {
    let fixture = Fixture::new("format-policy-atomic");
    fixture.write("valid.pure", FORMATTER_VALID_SOURCE);
    fixture.write("broken.pure", FORMATTER_BROKEN_SOURCE);

    assert_default_format_write_is_blocked(
        &fixture,
        &["fmt", "valid.pure", "broken.pure", "--no-config"],
        Some("PUR0102"),
    );
    assert_default_format_write_is_blocked(
        &fixture,
        &[
            "fmt",
            "valid.pure",
            "broken.pure",
            "--warn",
            "PUR0102",
            "--no-config",
        ],
        Some("PUR0102"),
    );
    assert_default_format_write_is_blocked(
        &fixture,
        &[
            "fmt",
            "valid.pure",
            "broken.pure",
            "--ignore",
            "PUR0102",
            "--no-config",
        ],
        None,
    );
    fixture.assert_no_writer_artifacts();
}

fn assert_default_format_write_is_blocked(
    fixture: &Fixture,
    arguments: &[&str],
    expected_diagnostic: Option<&str>,
) {
    let output = run(&fixture.root, arguments);

    assert_eq!(output.status.code(), Some(EXIT_ACTIONABLE));
    assert!(output.stdout.is_empty());
    match expected_diagnostic {
        Some(diagnostic) => assert!(utf8(&output.stderr).contains(diagnostic)),
        None => assert!(output.stderr.is_empty()),
    }
    assert_eq!(fixture.read("valid.pure"), FORMATTER_VALID_SOURCE);
    assert_eq!(fixture.read("broken.pure"), FORMATTER_BROKEN_SOURCE);
}

const EQUIVALENT_COMPARISON_QUERY: &str =
    "model::Person.all()->project(~[label: person | $person.name])";

fn equivalent_comparison_fixture(name: &str) -> Fixture {
    let fixture = Fixture::new(name);
    fixture.write("query.pure", EQUIVALENT_COMPARISON_QUERY);
    fixture.write("model.json", &person_model());
    fixture
}

#[test]
fn comparison_commands_render_equivalence() {
    let fixture = equivalent_comparison_fixture("comparison-equivalent-human");

    for command in ["eq", "diff"] {
        let output = run(
            &fixture.root,
            &[
                command,
                "query.pure",
                "query.pure",
                "--model",
                "model.json",
                "--no-config",
            ],
        );
        assert!(
            output.status.success(),
            "{command}: {}",
            utf8(&output.stderr)
        );
        assert!(output.stderr.is_empty());
        assert!(
            utf8(&output.stdout).contains("equivalent"),
            "{command} did not render equivalence: {}",
            utf8(&output.stdout)
        );
    }
}

#[test]
fn comparison_equivalence_json_is_witness_free() {
    let fixture = equivalent_comparison_fixture("comparison-equivalent-json");
    let output = run(
        &fixture.root,
        &[
            "eq",
            "query.pure",
            "query.pure",
            "--model",
            "model.json",
            "--format",
            "json",
            "--no-config",
        ],
    );
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let document: Value = serde_json::from_slice(&output.stdout).expect("valid comparison JSON");
    assert_eq!(document["version"], "1.0");
    assert_eq!(document["outcome"], "equivalent");
    assert!(document.get("witness").is_none());
}

#[test]
fn comparison_quiet_mode_suppresses_equivalence_output() {
    let fixture = equivalent_comparison_fixture("comparison-equivalent-quiet");
    let output = run(
        &fixture.root,
        &[
            "eq",
            "query.pure",
            "query.pure",
            "--model",
            "model.json",
            "--quiet",
            "--no-config",
        ],
    );
    assert!(output.status.success());
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
}

#[test]
fn comparison_accepts_one_standard_input_operand() {
    let fixture = equivalent_comparison_fixture("comparison-equivalent-stdin");
    let output = run_with_stdin(
        &fixture.root,
        &[
            "diff",
            "-",
            "query.pure",
            "--model",
            "model.json",
            "--format",
            "json",
            "--no-config",
        ],
        EQUIVALENT_COMPARISON_QUERY,
    );
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let document: Value =
        serde_json::from_slice(&output.stdout).expect("valid stdin comparison JSON");
    assert_eq!(document["outcome"], "equivalent");
    assert_eq!(document["version"], "1.0");
}

#[test]
fn comparison_glob_operands_resolve_to_exactly_one_file() {
    let fixture = equivalent_comparison_fixture("comparison-equivalent-glob");
    let glob = run(
        &fixture.root,
        &[
            "eq",
            "*.pure",
            "query.pure",
            "--model",
            "model.json",
            "--no-config",
        ],
    );
    assert!(glob.status.success());
    fixture.write("other.pure", EQUIVALENT_COMPARISON_QUERY);
    let multiple_glob = run(
        &fixture.root,
        &[
            "eq",
            "*.pure",
            "query.pure",
            "--model",
            "model.json",
            "--no-config",
        ],
    );
    assert_eq!(multiple_glob.status.code(), Some(EXIT_USAGE));
    assert!(multiple_glob.stdout.is_empty());
    assert!(utf8(&multiple_glob.stderr).contains("must resolve to exactly one file"));
}

#[test]
fn comparison_structural_refutation_uses_the_typed_difference_without_a_witness() {
    let fixture = Fixture::new("comparison-not-equivalent");
    fixture.write(
        "left.pure",
        "model::Person.all()->project(~[label: person | $person.name])",
    );
    fixture.write(
        "right.pure",
        "model::Person.all()->project(~[other: person | $person.name])",
    );
    fixture.write("model.json", &person_model());

    let output = run(
        &fixture.root,
        &[
            "eq",
            "left.pure",
            "right.pure",
            "--model",
            "model.json",
            "--format",
            "json",
            "--no-config",
        ],
    );
    assert_eq!(output.status.code(), Some(EXIT_ACTIONABLE));
    assert!(output.stderr.is_empty());
    let document: Value = serde_json::from_slice(&output.stdout).expect("valid comparison JSON");
    assert_eq!(document["outcome"], "not_equivalent");
    assert_eq!(document["difference"]["kind"], "output_column");
    assert_eq!(document["difference"]["index"], 0);
    assert_eq!(document["difference"]["field"], "name");
    assert!(document["difference"].get("primary_origin").is_some());
    assert!(document["difference"].get("secondary_origin").is_some());
    assert!(document.get("witness").is_none());
    assert!(document["difference"].get("witness").is_none());
}

fn comparison_indecision_fixture(name: &str) -> Fixture {
    let fixture = Fixture::new(name);
    fixture.write("left.pure", "model::Person.all()");
    fixture.write("right.pure", "model::Person.all()");
    fixture
}

fn assert_indecisive_json(output: Output, reason: &str) {
    assert_eq!(output.status.code(), Some(EXIT_INDECISIVE));
    assert!(output.stderr.is_empty());
    let document: Value = serde_json::from_slice(&output.stdout).expect("valid comparison JSON");
    assert_eq!(document["outcome"], "indecisive");
    assert_eq!(document["reason"]["id"], reason);
}

#[test]
fn comparison_without_a_model_is_indecisive() {
    let fixture = comparison_indecision_fixture("comparison-without-model");
    let output = run(
        &fixture.root,
        &[
            "diff",
            "left.pure",
            "right.pure",
            "--format",
            "json",
            "--no-config",
        ],
    );
    assert_indecisive_json(output, "MODEL_INCOMPLETE");
}

#[test]
fn comparison_with_an_unresolved_schema_is_indecisive() {
    let fixture = comparison_indecision_fixture("comparison-unresolved-schema");
    fixture.write("model.json", &person_model());
    fixture.write(
        "unresolved.pure",
        "model::Person.all()->project(~[label: person | $person.missing])",
    );
    let output = run(
        &fixture.root,
        &[
            "diff",
            "unresolved.pure",
            "right.pure",
            "--model",
            "model.json",
            "--format",
            "json",
            "--no-config",
        ],
    );
    assert_indecisive_json(output, "IND_UNRESOLVED_SCHEMA");
}

#[test]
fn comparison_with_malformed_syntax_is_indecisive() {
    let fixture = comparison_indecision_fixture("comparison-malformed-syntax");
    fixture.write("model.json", &person_model());
    fixture.write(
        "malformed.pure",
        "model::Person.all()->filter(person| $person.name ==)",
    );
    let output = run(
        &fixture.root,
        &[
            "eq",
            "malformed.pure",
            "right.pure",
            "--model",
            "model.json",
            "--format",
            "json",
            "--no-config",
        ],
    );
    assert_indecisive_json(output, "IND_UNPARSEABLE");
}

#[test]
fn comparison_rejects_a_malformed_model_at_the_boundary() {
    let fixture = comparison_indecision_fixture("comparison-bad-model");
    fixture.write("broken.json", "not JSON");
    let model_failure = run(
        &fixture.root,
        &[
            "eq",
            "left.pure",
            "right.pure",
            "--model",
            "broken.json",
            "--no-config",
        ],
    );
    assert_eq!(model_failure.status.code(), Some(EXIT_USAGE));
    assert!(model_failure.stdout.is_empty());
    assert!(utf8(&model_failure.stderr).contains("could not load model"));
}

#[test]
fn comparison_rejects_sarif_and_two_standard_input_operands_as_usage_errors() {
    let fixture = Fixture::new("comparison-usage");
    fixture.write("left.pure", "model::Person.all()");
    fixture.write("right.pure", "model::Person.all()");

    let sarif = run(
        &fixture.root,
        &[
            "eq",
            "left.pure",
            "right.pure",
            "--format",
            "sarif",
            "--no-config",
        ],
    );
    assert_eq!(sarif.status.code(), Some(EXIT_USAGE));
    assert!(sarif.stdout.is_empty());
    assert_eq!(
        utf8(&sarif.stderr),
        "error: eq and diff support only --format human or --format json\n"
    );

    let duplicate_stdin = run(&fixture.root, &["diff", "-", "-", "--no-config"]);
    assert_eq!(duplicate_stdin.status.code(), Some(EXIT_USAGE));
    assert!(duplicate_stdin.stdout.is_empty());
    assert!(
        utf8(&duplicate_stdin.stderr)
            .contains("comparison accepts standard input for at most one operand")
    );
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

fn lint_fix_fixture(label: &str) -> (Fixture, &'static str, &'static str) {
    let fixture = Fixture::new(label);
    let query = "model::Source.all()->filter(x| $x.point())";
    let fixed = "model::Source.all()->filter(x| $x.point(%latest))";
    fixture.write("query.pure", query);
    fixture.write("model.json", &milestoning_model());
    (fixture, query, fixed)
}

fn milestoning_model() -> String {
    r#"{
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
                    "returnGenericType": {
                        "rawType": "model::TemporalTarget",
                        "typeArguments": []
                    },
                    "returnMultiplicity": {
                        "lowerBound": 0,
                        "upperBound": 1
                    },
                    "stereotypes": [{
                        "profile": "meta::pure::profiles::milestoning",
                        "value": "generatedmilestoningproperty"
                    }],
                    "parameters": []
                }]
            }
        ]
    }"#
    .to_owned()
}

fn utf8(bytes: &[u8]) -> &str {
    std::str::from_utf8(bytes).expect("UTF-8 process output")
}

fn human_explanation(content: &ExplainContent) -> String {
    format!(
        "{} ({}, {})\n\nMeaning\n{}\n\nLimit\n{}\n\nRemedy\n{}\n\nDocumentation\n{}\n",
        content.identifier,
        content.kind.as_str(),
        content.classification.as_str(),
        content.meaning,
        content.limit,
        content.remedy,
        content.documentation_url,
    )
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
