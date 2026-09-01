//! End-to-end framed JSON-RPC transcripts for the LSP bootstrap server.

use std::io::{Cursor, Write};
use std::process::{Command, Output, Stdio};

use pure_analyzer_lsp::{
    CancellationRegistry, DocumentSnapshot, DocumentStore, RequestId, Server, ServerExit,
    WorkspaceConfiguration,
};
use serde_json::Value;

#[test]
fn startup_shutdown_and_exit_follow_one_deterministic_transcript() {
    let mut server = Server::new();
    let mut output = Vec::new();
    let mut input = Cursor::new(transcript(&[
        value(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#),
        value(r#"{"jsonrpc":"2.0","id":2,"method":"shutdown"}"#),
        value(r#"{"jsonrpc":"2.0","method":"exit"}"#),
    ]));

    assert_eq!(
        server
            .serve(&mut input, &mut output)
            .expect("valid transcript"),
        ServerExit::Clean
    );
    let initialize = format!(
        r#"{{"jsonrpc":"2.0","id":1,"result":{{"capabilities":{{}},"serverInfo":{{"name":"pure-analyzer-lsp","version":"{}"}}}}}}"#,
        env!("CARGO_PKG_VERSION")
    );
    assert_eq!(
        responses(&output),
        vec![
            value(&initialize),
            value(r#"{"jsonrpc":"2.0","id":2,"result":null}"#),
        ]
    );
}

#[test]
fn cancellation_document_store_and_configuration_stay_at_the_front_end() {
    let mut server = Server::new();
    let mut output = Vec::new();
    let mut input = Cursor::new(transcript(&[
        value(r#"{"jsonrpc":"2.0","id":"start","method":"initialize"}"#),
        value(r#"{"jsonrpc":"2.0","method":"$/cancelRequest","params":{"id":"work-7"}}"#),
        value(
            r#"{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///model.pure","version":3,"text":"Class A{}"}}}"#,
        ),
        value(
            r#"{"jsonrpc":"2.0","method":"workspace/didChangeConfiguration","params":{"settings":{"maxProblems":20}}}"#,
        ),
        value(r#"{"jsonrpc":"2.0","method":"exit"}"#),
    ]));

    assert_eq!(
        server
            .serve(&mut input, &mut output)
            .expect("valid transcript"),
        ServerExit::Unclean
    );
    assert!(
        server
            .cancellation()
            .is_cancelled(&RequestId::String("work-7".into()))
    );
    let document = server
        .documents()
        .get("file:///model.pure")
        .expect("open document");
    assert_eq!(document.text(), "Class A{}");
    assert_eq!(document.version(), Some(3));
    assert_eq!(
        server.configuration().settings(),
        Some(&value(r#"{"maxProblems":20}"#))
    );
}

#[test]
fn numeric_cancellation_and_document_close_stay_at_the_front_end() {
    let mut server = Server::new();
    let mut output = Vec::new();
    let mut input = Cursor::new(transcript(&[
        value(r#"{"jsonrpc":"2.0","id":"start","method":"initialize"}"#),
        value(r#"{"jsonrpc":"2.0","method":"$/cancelRequest","params":{"id":7}}"#),
        value(
            r#"{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///model.pure","version":3,"text":"Class A{}"}}}"#,
        ),
        value(
            r#"{"jsonrpc":"2.0","method":"textDocument/didClose","params":{"textDocument":{"uri":"file:///model.pure"}}}"#,
        ),
        value(r#"{"jsonrpc":"2.0","method":"exit"}"#),
    ]));

    assert_eq!(
        server
            .serve(&mut input, &mut output)
            .expect("valid transcript"),
        ServerExit::Unclean
    );
    assert!(server.cancellation().is_cancelled(&RequestId::Number(7)));
    assert!(server.documents().is_empty());
}

#[test]
fn unknown_requests_receive_a_json_rpc_method_error() {
    let mut server = Server::new();
    let mut output = Vec::new();
    let mut input = Cursor::new(transcript(&[
        value(r#"{"jsonrpc":"2.0","id":9,"method":"pureAnalyzer/notReady"}"#),
        value(r#"{"jsonrpc":"2.0","method":"exit"}"#),
    ]));

    assert_eq!(
        server
            .serve(&mut input, &mut output)
            .expect("valid transcript"),
        ServerExit::Unclean
    );
    assert_eq!(
        responses(&output),
        vec![value(
            r#"{"jsonrpc":"2.0","id":9,"error":{"code":-32601,"message":"method not found"}}"#
        )]
    );
}

#[test]
fn non_object_requests_receive_a_json_rpc_invalid_request_error() {
    let mut server = Server::new();
    let mut output = Vec::new();
    let mut input = Cursor::new(transcript(&[
        value("null"),
        value(r#"{"jsonrpc":"2.0","method":"exit"}"#),
    ]));

    assert_eq!(
        server
            .serve(&mut input, &mut output)
            .expect("valid transcript"),
        ServerExit::Unclean
    );
    assert_eq!(
        responses(&output),
        vec![value(
            r#"{"jsonrpc":"2.0","id":null,"error":{"code":-32600,"message":"invalid request"}}"#
        )]
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

#[test]
fn cancellation_registry_distinguishes_present_and_absent_requests() {
    let first = RequestId::Number(1);
    let other = RequestId::String("other".to_owned());
    let mut cancellations = CancellationRegistry::default();
    assert!(cancellations.is_empty());
    assert_eq!(cancellations.len(), 0);
    assert!(!cancellations.is_cancelled(&first));
    cancellations.cancel(first.clone());
    assert!(!cancellations.is_empty());
    assert_eq!(cancellations.len(), 1);
    assert!(cancellations.is_cancelled(&first));
    assert!(!cancellations.is_cancelled(&other));
}

#[test]
fn document_store_distinguishes_present_and_absent_documents() {
    let document = DocumentSnapshot::new(
        "file:///model.pure".to_owned(),
        "Class A{}".to_owned(),
        Some(3),
    );
    assert_eq!(document.uri(), "file:///model.pure");
    let mut documents = DocumentStore::default();
    assert!(documents.is_empty());
    assert_eq!(documents.len(), 0);
    assert_eq!(documents.get(document.uri()), None);
    documents.insert(document.clone());
    assert!(!documents.is_empty());
    assert_eq!(documents.len(), 1);
    assert_eq!(documents.get(document.uri()), Some(&document));
    assert_eq!(documents.remove(document.uri()), Some(document.clone()));
    assert!(documents.is_empty());
    assert_eq!(documents.len(), 0);
    assert_eq!(documents.get(document.uri()), None);
    assert_eq!(documents.remove(document.uri()), None);
}

#[test]
fn workspace_configuration_distinguishes_initial_and_replaced_values() {
    let mut configuration = WorkspaceConfiguration::default();
    assert_eq!(configuration.settings(), None);
    configuration.replace(value(r#"{"maxProblems":20}"#));
    assert_eq!(
        configuration.settings(),
        Some(&value(r#"{"maxProblems":20}"#))
    );
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

fn responses(mut input: &[u8]) -> Vec<Value> {
    let mut values = Vec::new();
    while !input.is_empty() {
        let boundary = input
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .expect("frame boundary");
        let header = std::str::from_utf8(&input[..boundary]).expect("ASCII header");
        let length = header
            .strip_prefix("Content-Length: ")
            .expect("length header")
            .parse::<usize>()
            .expect("numeric length");
        let body_start = boundary + 4;
        let body_end = body_start + length;
        values.push(serde_json::from_slice(&input[body_start..body_end]).expect("response JSON"));
        input = &input[body_end..];
    }
    values
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
