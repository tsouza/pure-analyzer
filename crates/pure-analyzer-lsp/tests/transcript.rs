//! End-to-end framed JSON-RPC transcripts for the LSP bootstrap server.

use std::io::{Cursor, Write};
use std::process::{Command, Output, Stdio};

use pure_analyzer_lsp::{
    CancellationRegistry, DocumentSnapshot, DocumentStore, RequestId, Server, ServerExit,
    WorkspaceConfiguration,
};
use serde_json::{Map, Value};

const PURE_WINNER_MODEL: &str = "Class demo::Winner\n{\n  value: String[0..1];\n}";
const PMCD_WINNER_MODEL: &str = r#"{
    "_type": "data",
    "elements": [{
        "_type": "class",
        "package": "demo",
        "name": "Winner",
        "stereotypes": [],
        "superTypes": [],
        "properties": [{
            "name": "value",
            "genericType": {"rawType": "Integer", "typeArguments": []},
            "multiplicity": {"lowerBound": 1, "upperBound": 1}
        }],
        "qualifiedProperties": []
    }]
}"#;

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
    let frames = responses(&output);
    assert_eq!(frames.len(), 2);
    assert_eq!(frames[0]["jsonrpc"], "2.0");
    assert_eq!(frames[0]["id"], 1);
    assert_eq!(
        frames[0]["result"]["capabilities"]["positionEncoding"],
        "utf-16"
    );
    assert_eq!(
        frames[0]["result"]["capabilities"]["textDocumentSync"],
        value(r#"{"openClose":true,"change":2,"save":{"includeText":false}}"#)
    );
    assert_eq!(
        frames[0]["result"]["serverInfo"]["name"],
        "pure-analyzer-lsp"
    );
    assert_eq!(
        frames[0]["result"]["serverInfo"]["version"],
        env!("CARGO_PKG_VERSION")
    );
    assert_eq!(
        frames[1],
        value(r#"{"jsonrpc":"2.0","id":2,"result":null}"#)
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
fn unicode_incremental_lifecycle_rejects_stale_and_invalid_changes() {
    let mut server = Server::new();
    let mut output = Vec::new();
    let mut input = Cursor::new(transcript(&[
        value(r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#),
        value(
            r#"{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"untitled:unicode","version":1,"text":"/* 😀 */ [a,]"}}}"#,
        ),
        value(
            r#"{"jsonrpc":"2.0","method":"textDocument/didChange","params":{"textDocument":{"uri":"untitled:unicode","version":2},"contentChanges":[{"range":{"start":{"line":0,"character":12},"end":{"line":0,"character":13}},"rangeLength":1,"text":"b]"}]}}"#,
        ),
        value(
            r#"{"jsonrpc":"2.0","method":"textDocument/didSave","params":{"textDocument":{"uri":"untitled:unicode"}}}"#,
        ),
        value(
            r#"{"jsonrpc":"2.0","method":"textDocument/didChange","params":{"textDocument":{"uri":"untitled:unicode","version":1},"contentChanges":[{"text":"[a,]"}]}}"#,
        ),
        value(
            r#"{"jsonrpc":"2.0","method":"textDocument/didChange","params":{"textDocument":{"uri":"untitled:unicode","version":3},"contentChanges":[{"range":{"start":{"line":0,"character":12},"end":{"line":0,"character":13}},"rangeLength":2,"text":"c"}]}}"#,
        ),
        value(
            r#"{"jsonrpc":"2.0","method":"textDocument/didChange","params":{"textDocument":{"uri":"untitled:unicode","version":4},"contentChanges":[{"range":{"start":{"line":0,"character":4},"end":{"line":0,"character":5}},"rangeLength":1,"text":"x"}]}}"#,
        ),
        value(
            r#"{"jsonrpc":"2.0","method":"textDocument/didClose","params":{"textDocument":{"uri":"untitled:unicode"}}}"#,
        ),
        value(r#"{"jsonrpc":"2.0","id":2,"method":"shutdown"}"#),
        value(r#"{"jsonrpc":"2.0","method":"exit"}"#),
    ]));

    assert_eq!(
        server
            .serve(&mut input, &mut output)
            .expect("valid transcript"),
        ServerExit::Clean
    );
    let publications = published_diagnostics(&output);
    assert_eq!(publications.len(), 4);
    assert_eq!(
        publications[0],
        value(
            r#"{"jsonrpc":"2.0","method":"textDocument/publishDiagnostics","params":{"uri":"untitled:unicode","version":1,"diagnostics":[{"range":{"start":{"line":0,"character":12},"end":{"line":0,"character":13}},"severity":1,"code":"PUR1200","source":"pure-analyzer","message":"expected an expression after `,`"}]}}"#,
        )
    );
    for publication in &publications[1..] {
        assert_eq!(publication["params"]["uri"], "untitled:unicode");
        assert_eq!(publication["params"]["version"], 2);
        assert_eq!(publication["params"]["diagnostics"], value("[]"));
    }
    assert!(server.documents().is_empty());
}

#[test]
fn incomplete_unconfigured_document_publishes_a_deterministic_diagnostic() {
    let mut server = Server::new();
    let mut output = Vec::new();
    let mut input = Cursor::new(transcript(&[
        value(r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#),
        value(
            r#"{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///query.pmcd","version":7,"text":"[a,]"}}}"#,
        ),
        value(r#"{"jsonrpc":"2.0","id":2,"method":"shutdown"}"#),
        value(r#"{"jsonrpc":"2.0","method":"exit"}"#),
    ]));

    assert_eq!(
        server
            .serve(&mut input, &mut output)
            .expect("valid transcript"),
        ServerExit::Clean
    );
    assert_eq!(
        published_diagnostics(&output),
        vec![value(
            r#"{"jsonrpc":"2.0","method":"textDocument/publishDiagnostics","params":{"uri":"file:///query.pmcd","version":7,"diagnostics":[{"range":{"start":{"line":0,"character":3},"end":{"line":0,"character":4}},"severity":1,"code":"PUR1200","source":"pure-analyzer","message":"expected an expression after `,`"}]}}"#,
        )]
    );
}

#[test]
fn configured_multi_file_models_publish_findings_to_their_explicit_routes() {
    let mut server = Server::new();
    let mut output = Vec::new();
    let mut input = Cursor::new(transcript(&[
        value(r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#),
        value(
            r#"{"jsonrpc":"2.0","method":"workspace/didChangeConfiguration","params":{"settings":{"modelDocuments":[{"uri":"untitled:domain-one","kind":"pure"},{"uri":"untitled:domain-two","kind":"pmcd"}]}}}"#,
        ),
        did_open("untitled:domain-one", 1, PURE_WINNER_MODEL),
        did_open("untitled:domain-two", 1, PMCD_WINNER_MODEL),
        value(
            r#"{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///query.pmcd","version":1,"text":"demo::Winner.all()"}}}"#,
        ),
        value(r#"{"jsonrpc":"2.0","id":2,"method":"shutdown"}"#),
        value(r#"{"jsonrpc":"2.0","method":"exit"}"#),
    ]));

    assert_eq!(
        server
            .serve(&mut input, &mut output)
            .expect("valid transcript"),
        ServerExit::Clean
    );
    let publications = published_diagnostics(&output);
    assert_eq!(publications.len(), 6);
    let final_publications = &publications[3..];
    let first_model = publication_for(final_publications, "untitled:domain-one");
    let second_model = publication_for(final_publications, "untitled:domain-two");
    let query = publication_for(final_publications, "file:///query.pmcd");
    assert_eq!(first_model["params"]["diagnostics"], value("[]"));
    assert_eq!(query["params"]["diagnostics"], value("[]"));
    assert_eq!(
        second_model["params"]["diagnostics"],
        value(
            r#"[{"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":0}},"severity":2,"code":"PUR9000","source":"pure-analyzer","message":"model element `demo::Winner` from `untitled:domain-two` replaces the definition from `untitled:domain-one`"}]"#,
        )
    );
}

#[test]
fn model_only_routes_publish_merge_diagnostics_without_queries() {
    let mut server = Server::new();
    let mut output = Vec::new();
    let mut input = Cursor::new(transcript(&[
        value(r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#),
        value(
            r#"{"jsonrpc":"2.0","method":"workspace/didChangeConfiguration","params":{"settings":{"modelDocuments":[{"uri":"untitled:domain-one","kind":"pure"},{"uri":"untitled:domain-two","kind":"pmcd"}]}}}"#,
        ),
        did_open("untitled:domain-one", 1, PURE_WINNER_MODEL),
        did_open("untitled:domain-two", 1, PMCD_WINNER_MODEL),
        value(r#"{"jsonrpc":"2.0","id":2,"method":"shutdown"}"#),
        value(r#"{"jsonrpc":"2.0","method":"exit"}"#),
    ]));

    assert_eq!(
        server
            .serve(&mut input, &mut output)
            .expect("valid transcript"),
        ServerExit::Clean
    );
    let publications = published_diagnostics(&output);
    assert_eq!(publications.len(), 3);
    let final_publications = &publications[1..];
    let first_model = publication_for(final_publications, "untitled:domain-one");
    let second_model = publication_for(final_publications, "untitled:domain-two");
    assert_eq!(first_model["params"]["diagnostics"], value("[]"));
    assert_eq!(
        second_model["params"]["diagnostics"],
        value(
            r#"[{"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":0}},"severity":2,"code":"PUR9000","source":"pure-analyzer","message":"model element `demo::Winner` from `untitled:domain-two` replaces the definition from `untitled:domain-one`"}]"#,
        )
    );
}

#[test]
fn malformed_configured_pmcd_is_reported_without_queries_and_after_query_close() {
    let mut server = Server::new();
    let mut output = Vec::new();
    let mut input = Cursor::new(transcript(&[
        value(r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#),
        value(
            r#"{"jsonrpc":"2.0","method":"workspace/didChangeConfiguration","params":{"settings":{"modelDocuments":[{"uri":"untitled:broken","kind":"pmcd"}]}}}"#,
        ),
        value(
            r#"{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"untitled:broken","version":1,"text":"{"}}}"#,
        ),
        value(
            r#"{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///query.pure","version":1,"text":"demo::Winner.all()"}}}"#,
        ),
        value(
            r#"{"jsonrpc":"2.0","method":"textDocument/didClose","params":{"textDocument":{"uri":"file:///query.pure"}}}"#,
        ),
        value(r#"{"jsonrpc":"2.0","id":2,"method":"shutdown"}"#),
        value(r#"{"jsonrpc":"2.0","method":"exit"}"#),
    ]));

    assert_eq!(
        server
            .serve(&mut input, &mut output)
            .expect("valid transcript"),
        ServerExit::Clean
    );
    let publications = published_diagnostics(&output);
    assert_eq!(publications.len(), 5);
    let model_publications = publications_for(&publications, "untitled:broken");
    assert_eq!(model_publications.len(), 3);
    for publication in model_publications {
        assert_model_load_diagnostic(publication);
    }
    let query_publications = publications_for(&publications, "file:///query.pure");
    assert_eq!(query_publications.len(), 2);
    for publication in query_publications {
        assert_eq!(publication["params"]["diagnostics"], value("[]"));
    }
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

fn published_diagnostics(output: &[u8]) -> Vec<Value> {
    responses(output)
        .into_iter()
        .filter(|message| message["method"] == "textDocument/publishDiagnostics")
        .collect()
}

fn publication_for<'a>(publications: &'a [Value], uri: &str) -> &'a Value {
    let matches = publications_for(publications, uri);
    assert_eq!(matches.len(), 1, "expected one publication for {uri}");
    matches[0]
}

fn publications_for<'a>(publications: &'a [Value], uri: &str) -> Vec<&'a Value> {
    publications
        .iter()
        .filter(|publication| publication["params"]["uri"] == uri)
        .collect()
}

fn assert_model_load_diagnostic(publication: &Value) {
    let diagnostics = publication["params"]["diagnostics"]
        .as_array()
        .expect("diagnostics array");
    assert_eq!(diagnostics.len(), 1);
    let diagnostic = &diagnostics[0];
    assert_eq!(
        diagnostic["range"],
        value(r#"{"start":{"line":0,"character":0},"end":{"line":0,"character":0}}"#)
    );
    assert_eq!(diagnostic["severity"], 1);
    assert_eq!(diagnostic["code"], Value::Null);
    assert_eq!(diagnostic["source"], "pure-analyzer");
    assert!(diagnostic["message"].as_str().is_some_and(|message| {
        message.starts_with("PMCD source `untitled:broken` is not valid JSON")
    }));
}

fn did_open(uri: &str, version: i64, text: &str) -> Value {
    object([
        ("jsonrpc", Value::String("2.0".to_owned())),
        ("method", Value::String("textDocument/didOpen".to_owned())),
        (
            "params",
            object([(
                "textDocument",
                object([
                    ("uri", Value::String(uri.to_owned())),
                    ("version", Value::Number(version.into())),
                    ("text", Value::String(text.to_owned())),
                ]),
            )]),
        ),
    ])
}

fn value(source: &str) -> Value {
    serde_json::from_str(source).expect("test JSON must parse")
}

fn object(fields: impl IntoIterator<Item = (&'static str, Value)>) -> Value {
    Value::Object(
        fields
            .into_iter()
            .map(|(name, value)| (name.to_owned(), value))
            .collect::<Map<_, _>>(),
    )
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
