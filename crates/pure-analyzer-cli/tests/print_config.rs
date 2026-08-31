#![forbid(unsafe_code)]
#![deny(missing_docs)]

//! Process-boundary tests for deterministic resolved configuration output.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

static FIXTURE_COUNTER: AtomicUsize = AtomicUsize::new(0);

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let counter = FIXTURE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "pure-analyzer-print-config-{}-{counter}-{name}",
            std::process::id()
        ));
        fs::create_dir_all(&root).expect("create fixture root");
        Self { root }
    }

    fn write(&self, relative: &str, text: &str) -> PathBuf {
        let path = self.root.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create fixture parent");
        }
        fs::write(&path, text).expect("write fixture");
        path
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn command() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_pure-analyzer"));
    command.env_remove("RUST_LOG");
    for (name, _) in std::env::vars_os() {
        if name.to_string_lossy().starts_with("PURE_ANALYZER_") {
            command.env_remove(name);
        }
    }
    command
}

fn isolate_user_config(command: &mut Command, root: &Path) {
    #[cfg(windows)]
    command.env("APPDATA", root.join("appdata"));
    #[cfg(not(windows))]
    command.env("XDG_CONFIG_HOME", root.join("xdg-config"));
}

fn print_config() -> Output {
    command()
        .args(["--print-config", "--no-config"])
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
    let expected = concat!(
        "version = 1\n",
        "jobs = 1\n\n",
        "[output]\n",
        "format = \"human\"\n",
        "color = \"auto\"\n",
        "quiet = false\n\n",
        "[lint]\n",
        "select = []\n",
        "ignore = []\n",
        "deny = []\n",
        "warn = []\n\n",
        "[validate]\n",
        "strict = false\n\n",
        "[fmt]\n",
        "line-width = 100\n\n",
        "[model]\n",
        "paths = []\n",
    );
    assert_eq!(text, expected);
    text.parse::<toml::Table>().expect("resolved TOML");
}

#[test]
fn malformed_environment_keeps_stdout_empty() {
    let output = command()
        .args(["--print-config", "--no-config"])
        .env("PURE_ANALYZER_JOBS", "many")
        .output()
        .expect("run pure-analyzer");

    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("PURE_ANALYZER_JOBS"));
}

#[test]
fn environment_overrides_file_config_with_native_model_path_lists() {
    let fixture = Fixture::new("environment-precedence");
    let config = fixture.write(
        "config/project.toml",
        "version = 1\njobs = 2\n\n[output]\nformat = \"human\"\ncolor = \"always\"\n\n[model]\npaths = [\"from-file.pure\"]\n",
    );
    let model_paths = [
        fixture.root.join("models/first.pure"),
        fixture.root.join("models/second.pure"),
    ];
    let model_paths_value = std::env::join_paths(&model_paths).expect("join native model paths");
    let mut process = command();
    isolate_user_config(&mut process, &fixture.root);
    let output = process
        .current_dir(&fixture.root)
        .args(["--print-config", "--config"])
        .arg(&config)
        .env("PURE_ANALYZER_JOBS", "6")
        .env("PURE_ANALYZER_FORMAT", "json")
        .env("PURE_ANALYZER_MODEL_PATHS", model_paths_value)
        .output()
        .expect("run pure-analyzer with file and environment config");

    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let document = String::from_utf8(output.stdout)
        .expect("UTF-8 config output")
        .parse::<toml::Table>()
        .expect("resolved TOML");
    assert_eq!(document["jobs"].as_integer(), Some(6));
    assert_eq!(document["output"]["format"].as_str(), Some("json"));
    assert_eq!(document["output"]["color"].as_str(), Some("always"));
    let printed_paths = document["model"]["paths"]
        .as_array()
        .expect("model paths array")
        .iter()
        .map(|value| value.as_str().expect("model path string"))
        .collect::<Vec<_>>();
    let expected_paths = model_paths
        .iter()
        .map(|path| path.to_str().expect("UTF-8 fixture path"))
        .collect::<Vec<_>>();
    assert_eq!(printed_paths, expected_paths);
}

#[test]
fn unknown_reserved_environment_variable_fails_closed() {
    let output = command()
        .args(["--print-config", "--no-config"])
        .env("PURE_ANALYZER_UNRECOGNIZED", "true")
        .output()
        .expect("run pure-analyzer with unknown environment variable");

    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("PURE_ANALYZER_UNRECOGNIZED"));
}
