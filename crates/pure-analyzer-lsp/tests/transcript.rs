//! Black-box transcripts for the `pure-analyzer-lsp` binary.
//!
//! These are the only tests that genuinely need no more than the compiled
//! binary existing: they spawn it as a subprocess and exercise it purely
//! through its stdio JSON-RPC transport. Every test that instead drives
//! `Server` in-process lives inside `crates/pure-analyzer-lsp/src/server.rs`,
//! where it can use the crate's internal (`pub(crate)`) surface instead of
//! requiring a public one just for tests.

use std::io::Write;
use std::process::{Command, Output, Stdio};

use serde_json::Value;

#[test]
fn malformed_input_is_logged_to_stderr_by_the_process_own_subscriber() {
    // `init_tracing` has no return value; a mutant that replaces its body
    // with `()` never installs a subscriber, so `tracing::error!` in
    // `main`'s error branch reaches the global no-op dispatcher instead of
    // standard error. A malformed header (no `:` separator) makes
    // `read_frame` return an `io::Error`, which `main` logs and turns into
    // an unsuccessful exit.
    let output = run_lsp(b"not-a-header-line\r\n");
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert_unsuccessful_exit(output);
    assert!(
        stderr.contains("pure-analyzer-lsp exited on an I/O failure"),
        "expected the I/O failure to be logged to stderr, got: {stderr}"
    );
}

#[test]
fn process_exits_unsuccessfully_without_shutdown() {
    let exit_before_shutdown = run_lsp(&transcript(&[value(
        r#"{"jsonrpc":"2.0","method":"exit"}"#,
    )]));
    let end_of_file = lsp_command()
        .stdin(Stdio::null())
        .output()
        .expect("run pure-analyzer-lsp through EOF");

    assert_unsuccessful_exit(exit_before_shutdown);
    assert_unsuccessful_exit(end_of_file);
}

fn value(source: &str) -> Value {
    serde_json::from_str(source).expect("test JSON must parse")
}

fn transcript(messages: &[Value]) -> Vec<u8> {
    let mut output = Vec::new();
    for message in messages {
        let body = serde_json::to_vec(message).expect("test JSON must serialize");
        output.extend_from_slice(format!("Content-Length: {}\r\n\r\n", body.len()).as_bytes());
        output.extend_from_slice(&body);
    }
    output
}

fn run_lsp(input: &[u8]) -> Output {
    let mut child = lsp_command()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn pure-analyzer-lsp");
    let mut stdin = child.stdin.take().expect("piped stdin");
    stdin
        .write_all(input)
        .expect("write pure-analyzer-lsp standard input");
    drop(stdin);
    child
        .wait_with_output()
        .expect("wait for pure-analyzer-lsp")
}

fn lsp_command() -> Command {
    Command::new(env!("CARGO_BIN_EXE_pure-analyzer-lsp"))
}

fn assert_unsuccessful_exit(output: Output) {
    assert!(
        !output.status.success(),
        "pure-analyzer-lsp must fail without shutdown: {output:?}"
    );
}
