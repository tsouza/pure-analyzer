#![forbid(unsafe_code)]
#![deny(missing_docs)]

//! Process-boundary tests for deterministic resolved configuration output.

use std::process::{Command, Output};

fn print_config() -> Output {
    Command::new(env!("CARGO_BIN_EXE_pure-analyzer"))
        .args(["--print-config", "--no-config"])
        .env_remove("PURE_ANALYZER_JOBS")
        .env_remove("PURE_ANALYZER_FORMAT")
        .env_remove("PURE_ANALYZER_COLOR")
        .env_remove("PURE_ANALYZER_QUIET")
        .env_remove("PURE_ANALYZER_SELECT")
        .env_remove("PURE_ANALYZER_IGNORE")
        .env_remove("PURE_ANALYZER_DENY")
        .env_remove("PURE_ANALYZER_WARN")
        .env_remove("PURE_ANALYZER_STRICT")
        .env_remove("PURE_ANALYZER_FMT_LINE_WIDTH")
        .env_remove("PURE_ANALYZER_MODEL_PATHS")
        .output()
        .expect("run pure-analyzer")
}

#[test]
fn print_config_is_machine_clean_deterministic_toml() {
    let first = print_config();
    let second = print_config();

    assert!(first.status.success());
    assert!(first.stderr.is_empty());
    assert_eq!(first.stdout, second.stdout);
    let text = String::from_utf8(first.stdout).expect("UTF-8 config output");
    let value = text.parse::<toml::Table>().expect("resolved TOML");
    assert_eq!(value["version"].as_integer(), Some(1));
    assert_eq!(value["jobs"].as_integer(), Some(1));
}

#[test]
fn malformed_environment_keeps_stdout_empty() {
    let output = Command::new(env!("CARGO_BIN_EXE_pure-analyzer"))
        .args(["--print-config", "--no-config"])
        .env("PURE_ANALYZER_JOBS", "many")
        .output()
        .expect("run pure-analyzer");

    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("PURE_ANALYZER_JOBS"));
}
