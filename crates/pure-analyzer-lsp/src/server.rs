use std::{
    io::{self, BufRead, Write},
    sync::mpsc::{self, Receiver, Sender, SyncSender},
    thread,
};

#[cfg(test)]
use std::sync::Arc;

use serde_json::Value;

use crate::{
    CancellationRegistry, DocumentStore, WorkspaceConfiguration, dispatch,
    frame::read_frame,
    scheduler::{CompletedRequest, RequestScheduler},
};

#[cfg(test)]
use crate::scheduler::RequestTestBarrier;

/// The terminal result of an LSP server session.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ServerExit {
    /// The client sent `shutdown` before `exit`.
    Clean,
    /// The client closed the stream or exited before shutdown.
    Unclean,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Lifecycle {
    New,
    Running,
    ShuttingDown,
}

/// A concurrent stdio JSON-RPC server with explicit front-end boundaries.
///
/// Internal only: nothing outside this crate constructs or drives a `Server`
/// directly. `main.rs` and any other external consumer reach the LSP only
/// through [`serve_stdio`].
#[derive(Debug)]
pub(crate) struct Server {
    pub(crate) cancellation: CancellationRegistry,
    pub(crate) configuration: WorkspaceConfiguration,
    pub(crate) documents: DocumentStore,
    pub(crate) lifecycle: Lifecycle,
    #[cfg(test)]
    pub(crate) request_barrier: Option<Arc<RequestTestBarrier>>,
}

impl Default for Server {
    fn default() -> Self {
        Self::new()
    }
}

impl Server {
    /// Construct a server before its `initialize` request.
    #[must_use]
    pub(crate) fn new() -> Self {
        Self {
            cancellation: CancellationRegistry::default(),
            configuration: WorkspaceConfiguration::default(),
            documents: DocumentStore::default(),
            lifecycle: Lifecycle::New,
            #[cfg(test)]
            request_barrier: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn with_request_barrier(request_barrier: Arc<RequestTestBarrier>) -> Self {
        let mut server = Self::new();
        server.request_barrier = Some(request_barrier);
        server
    }

    /// Serve framed JSON-RPC messages until the client exits or closes input.
    ///
    /// The caller owns transport lifetime and receives an I/O error for invalid
    /// framing or JSON. Valid but unsupported protocol requests get standard
    /// JSON-RPC error responses instead.
    pub(crate) fn serve<R: BufRead + Send, W: Write>(
        &mut self,
        reader: &mut R,
        writer: &mut W,
    ) -> io::Result<ServerExit> {
        let (events, receiver) = mpsc::channel();
        thread::scope(|scope| {
            let reader_events = events.clone();
            scope.spawn(move || read_events(reader, reader_events));
            let mut scheduler = RequestScheduler::new(
                events,
                #[cfg(test)]
                self.request_barrier.clone(),
            );
            self.serve_events(&receiver, writer, &mut scheduler)
        })
    }

    fn serve_events<W: Write>(
        &mut self,
        events: &Receiver<ServerEvent>,
        writer: &mut W,
        scheduler: &mut RequestScheduler,
    ) -> io::Result<ServerExit> {
        let mut terminal = None;
        loop {
            if let Some(exit) = terminal
                && scheduler.is_idle()
            {
                return Ok(exit);
            }
            match events.recv().map_err(|_| {
                io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "LSP input and request workers disconnected",
                )
            })? {
                ServerEvent::Input(message, acknowledgement) => {
                    if terminal.is_some() {
                        let _ = acknowledgement.send(ReaderAcknowledgement::Stop);
                        continue;
                    }
                    match dispatch::handle(self, message, writer, scheduler) {
                        Ok(Some(exit)) => {
                            let _ = acknowledgement.send(ReaderAcknowledgement::Stop);
                            terminal = Some(exit);
                        }
                        Ok(None) => {
                            let _ = acknowledgement.send(ReaderAcknowledgement::Continue);
                        }
                        Err(error) => {
                            let _ = acknowledgement.send(ReaderAcknowledgement::Stop);
                            return Err(error);
                        }
                    }
                }
                ServerEvent::EndOfInput => {
                    terminal.get_or_insert(ServerExit::Unclean);
                }
                ServerEvent::InputError(error) => return Err(error),
                ServerEvent::Completed(completed) => {
                    scheduler.complete(self, writer, completed)?;
                }
            }
        }
    }
}

/// Serve the Language Server Protocol over this process's standard streams.
pub fn serve_stdio() -> io::Result<ServerExit> {
    let stdin = io::stdin();
    let mut reader = io::BufReader::new(stdin);
    let mut writer = io::stdout();
    Server::new().serve(&mut reader, &mut writer)
}

pub(crate) enum ServerEvent {
    Input(Value, SyncSender<ReaderAcknowledgement>),
    EndOfInput,
    InputError(io::Error),
    Completed(CompletedRequest),
}

pub(crate) enum ReaderAcknowledgement {
    Continue,
    Stop,
}

fn read_events<R: BufRead>(reader: &mut R, events: Sender<ServerEvent>) {
    loop {
        match read_frame(reader) {
            Ok(Some(message)) => {
                let (acknowledgement, receiver) = mpsc::sync_channel(0);
                if events
                    .send(ServerEvent::Input(message, acknowledgement))
                    .is_err()
                {
                    return;
                }
                if !matches!(receiver.recv(), Ok(ReaderAcknowledgement::Continue)) {
                    return;
                }
            }
            Ok(None) => {
                let _ = events.send(ServerEvent::EndOfInput);
                return;
            }
            Err(error) => {
                let _ = events.send(ServerEvent::InputError(error));
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        cmp,
        io::{self, BufRead, Cursor, Read, Write},
        mem,
        sync::mpsc::{self, Receiver, Sender, SyncSender},
        thread::{self, JoinHandle},
        time::Duration,
    };

    use serde_json::{Map, Value};

    use super::{Server, ServerExit};
    use crate::{
        RequestId,
        frame::read_frame,
        scheduler::{RequestTestBarrier, RequestTestEvent},
    };

    const QUERY_URI: &str = "untitled:gated-query";
    const SECOND_QUERY_URI: &str = "untitled:independent-query";
    const INCOMPLETE_QUERY: &str = "/* 😀 */ [a,]";
    const DEFINITION_QUERY: &str =
        "{row: Relation<(zeta:String[1], alpha:Integer[0..1])>| $row.alpha}";
    const CHANGED_DEFINITION_QUERY: &str =
        "{row: Relation<(zeta:String[1], alpha:Integer[0..1])>| $row.zeta}";
    const MILESTONING_MODEL: &str = r#"{
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
}"#;
    const ACTION_MODEL_URI: &str = "untitled:gated-action-model";
    const ACTION_QUERY_URI: &str = "untitled:gated-action-query";
    const ACTION_QUERY: &str = "model::Source.all()->filter(x| $x.point())";

    #[test]
    fn cancellation_and_revisions_suppress_each_active_request_kind() {
        for request in [
            hover_request(2, QUERY_URI),
            definition_request(2, QUERY_URI),
            code_action_request(2, QUERY_URI),
        ] {
            let (mut client, events, release, server) = start_session(RequestId::Number(2));
            initialize_session(&mut client);
            client.send(did_open(QUERY_URI, 1, INCOMPLETE_QUERY));
            let _ = client.diagnostics(QUERY_URI, 1);
            client.send(request);
            wait_snapshot_captured(&events);
            client.send(did_change(QUERY_URI, 2, "/* 😀 */ [b,]"));
            let _ = client.diagnostics(QUERY_URI, 2);
            client.send(change_configuration());
            let _ = client.diagnostics(QUERY_URI, 2);
            release.send(()).expect("allow the worker to execute");
            wait_work_completed(&events);
            client.send(cancel_request(2));
            client.send(invalid_hover_request(90));
            assert_error(&client.response(90), -32_602, "invalid params");
            release
                .send(())
                .expect("allow the completed worker to reach the coordinator");

            let response = client.response(2);
            assert_error(&response, -32_800, "request cancelled");
            assert!(response.get("result").is_none());
            finish_clean(client, server);
        }
    }

    #[test]
    fn malformed_params_yield_invalid_params_for_every_request_kind() {
        let (mut client, _events, _release, server) = start_session(RequestId::Number(2));
        initialize_session(&mut client);

        client.send(invalid_hover_request(10));
        assert_error(&client.response(10), -32_602, "invalid params");

        client.send(invalid_definition_request(11));
        assert_error(&client.response(11), -32_602, "invalid params");

        client.send(invalid_code_action_request(12));
        assert_error(&client.response(12), -32_602, "invalid params");

        finish_clean(client, server);
    }

    #[test]
    fn active_request_identifiers_are_unique_and_reusable_after_completion() {
        let (mut client, events, release, server) = start_session(RequestId::Number(2));
        initialize_session(&mut client);
        client.send(did_open(QUERY_URI, 1, INCOMPLETE_QUERY));
        let _ = client.diagnostics(QUERY_URI, 1);
        client.send(hover_request(2, QUERY_URI));
        wait_snapshot_captured(&events);

        client.send(hover_request(2, QUERY_URI));
        let duplicate = client.take_matching(|frame| {
            frame.get("id").and_then(Value::as_i64) == Some(2) && frame.get("error").is_some()
        });
        assert_error(&duplicate, -32_600, "duplicate active request id");

        release
            .send(())
            .expect("allow the original request to execute");
        wait_work_completed(&events);
        release
            .send(())
            .expect("allow the original request to reach the coordinator");
        let original = client.take_matching(|frame| {
            frame.get("id").and_then(Value::as_i64) == Some(2) && frame.get("result").is_some()
        });
        assert_eq!(original["result"]["contents"]["kind"], "markdown");

        client.send(hover_request(2, QUERY_URI));
        wait_snapshot_captured(&events);
        release
            .send(())
            .expect("allow the reused request identifier to execute");
        wait_work_completed(&events);
        release
            .send(())
            .expect("allow the reused request identifier to reach the coordinator");
        let reused = client.take_matching(|frame| {
            frame.get("id").and_then(Value::as_i64) == Some(2) && frame.get("result").is_some()
        });
        assert_eq!(reused["result"]["contents"]["kind"], "markdown");
        finish_clean(client, server);
    }

    #[test]
    fn revision_and_configuration_changes_suppress_stale_results_for_every_request_kind() {
        stale_hover_is_suppressed();
        stale_definition_is_suppressed();
        stale_code_actions_are_suppressed();
    }

    #[test]
    fn overlapping_snapshots_do_not_cross_document_revisions() {
        let (mut client, events, release, server) = start_session(RequestId::Number(2));
        initialize_session(&mut client);
        client.send(did_open(QUERY_URI, 1, INCOMPLETE_QUERY));
        let _ = client.diagnostics(QUERY_URI, 1);
        client.send(did_open(SECOND_QUERY_URI, 1, INCOMPLETE_QUERY));
        let _ = client.diagnostics(SECOND_QUERY_URI, 1);
        client.send(hover_request(2, QUERY_URI));
        wait_snapshot_captured(&events);
        client.send(did_change(QUERY_URI, 2, "/* 😀 */ [b,]"));
        let _ = client.diagnostics(QUERY_URI, 2);
        client.send(hover_request(3, SECOND_QUERY_URI));
        let independent = client.response(3);
        assert_eq!(independent["result"]["contents"]["kind"], "markdown");
        release
            .send(())
            .expect("allow the stale request to execute");
        wait_work_completed(&events);
        release
            .send(())
            .expect("allow the stale request to reach the coordinator");

        assert_eq!(client.response(2)["result"], Value::Null);
        finish_clean(client, server);
    }

    #[test]
    fn eof_drains_active_work_before_the_unclean_server_exit() {
        let (mut client, events, release, server) = start_session(RequestId::Number(2));
        initialize_session(&mut client);
        client.send(did_open(QUERY_URI, 1, INCOMPLETE_QUERY));
        let _ = client.diagnostics(QUERY_URI, 1);
        client.send(hover_request(2, QUERY_URI));
        wait_snapshot_captured(&events);
        client.close_input();
        release
            .send(())
            .expect("allow the active request to execute at EOF");
        wait_work_completed(&events);
        release
            .send(())
            .expect("allow the active request to reach the coordinator at EOF");

        let response = client.response(2);
        assert_eq!(response["result"]["contents"]["kind"], "markdown");
        assert_eq!(
            server
                .join()
                .expect("server thread must not panic")
                .expect("valid EOF transcript"),
            ServerExit::Unclean
        );
    }

    fn stale_hover_is_suppressed() {
        let (mut client, events, release, server) = start_session(RequestId::Number(2));
        initialize_session(&mut client);
        client.send(did_open(QUERY_URI, 1, INCOMPLETE_QUERY));
        let _ = client.diagnostics(QUERY_URI, 1);
        client.send(hover_request(2, QUERY_URI));
        wait_snapshot_captured(&events);
        advance_document_and_configuration(&mut client, QUERY_URI, "/* 😀 */ [b,]");
        release.send(()).expect("allow the stale hover to execute");
        wait_work_completed(&events);
        release
            .send(())
            .expect("allow the stale hover to reach the coordinator");
        assert_eq!(client.response(2)["result"], Value::Null);
        finish_clean(client, server);
    }

    fn stale_definition_is_suppressed() {
        let (mut client, events, release, server) = start_session(RequestId::Number(2));
        initialize_session(&mut client);
        client.send(did_open(QUERY_URI, 1, DEFINITION_QUERY));
        let _ = client.diagnostics(QUERY_URI, 1);
        client.send(definition_request(2, QUERY_URI));
        wait_snapshot_captured(&events);
        advance_document_and_configuration(&mut client, QUERY_URI, CHANGED_DEFINITION_QUERY);
        release
            .send(())
            .expect("allow the stale definition to execute");
        wait_work_completed(&events);
        release
            .send(())
            .expect("allow the stale definition to reach the coordinator");
        assert_eq!(client.response(2)["result"], Value::Null);
        finish_clean(client, server);
    }

    fn stale_code_actions_are_suppressed() {
        let (mut client, events, release, server) = start_session(RequestId::Number(2));
        initialize_session(&mut client);
        client.send(configure_pmcd_model(ACTION_MODEL_URI));
        client.send(did_open(ACTION_MODEL_URI, 1, MILESTONING_MODEL));
        let _ = client.diagnostics(ACTION_MODEL_URI, 1);
        client.send(did_open(ACTION_QUERY_URI, 1, ACTION_QUERY));
        let _ = client.diagnostics(ACTION_QUERY_URI, 1);
        client.send(code_action_request(2, ACTION_QUERY_URI));
        wait_snapshot_captured(&events);
        advance_document_and_configuration(
            &mut client,
            ACTION_QUERY_URI,
            &fixed_machine_query(ACTION_QUERY),
        );
        release
            .send(())
            .expect("allow the stale code actions to execute");
        wait_work_completed(&events);
        release
            .send(())
            .expect("allow the stale code actions to reach the coordinator");
        assert_eq!(client.response(2)["result"], Value::Array(Vec::new()));
        finish_clean(client, server);
    }

    fn advance_document_and_configuration(client: &mut StreamingClient, uri: &str, text: &str) {
        client.send(did_change(uri, 2, text));
        let _ = client.diagnostics(uri, 2);
        client.send(change_configuration());
        let _ = client.diagnostics(uri, 2);
    }

    fn start_session(
        blocked: RequestId,
    ) -> (
        StreamingClient,
        Receiver<RequestTestEvent>,
        SyncSender<()>,
        JoinHandle<io::Result<ServerExit>>,
    ) {
        let (barrier, events, release) = RequestTestBarrier::new(blocked);
        let (input, reader) = mpsc::channel();
        let (frames, output) = mpsc::channel();
        let server = thread::spawn(move || {
            let mut server = Server::with_request_barrier(barrier);
            let mut reader = StreamingReader::new(reader);
            let mut writer = FrameWriter::new(frames);
            server.serve(&mut reader, &mut writer)
        });
        (StreamingClient::new(input, output), events, release, server)
    }

    fn initialize_session(client: &mut StreamingClient) {
        client.send(initialize());
        assert!(client.response(1).get("result").is_some());
    }

    fn finish_clean(mut client: StreamingClient, server: JoinHandle<io::Result<ServerExit>>) {
        client.send(shutdown());
        assert_eq!(client.response(99)["result"], Value::Null);
        client.send(exit());
        client.close_input();
        assert_eq!(
            server
                .join()
                .expect("server thread must not panic")
                .expect("valid clean transcript"),
            ServerExit::Clean
        );
    }

    fn wait_snapshot_captured(events: &Receiver<RequestTestEvent>) {
        wait_for_worker_event(events, RequestTestEvent::SnapshotCaptured);
    }

    fn wait_work_completed(events: &Receiver<RequestTestEvent>) {
        wait_for_worker_event(events, RequestTestEvent::WorkCompleted);
    }

    fn wait_for_worker_event(events: &Receiver<RequestTestEvent>, expected: RequestTestEvent) {
        let event = events
            .recv_timeout(Duration::from_secs(5))
            .expect("worker must reach the deterministic synchronization point");
        assert_eq!(event, expected);
    }

    fn assert_error(response: &Value, code: i64, message: &str) {
        assert_eq!(response["error"]["code"], code);
        assert_eq!(response["error"]["message"], message);
    }

    fn initialize() -> Value {
        object([
            ("jsonrpc", string("2.0")),
            ("id", number(1)),
            ("method", string("initialize")),
        ])
    }

    fn shutdown() -> Value {
        object([
            ("jsonrpc", string("2.0")),
            ("id", number(99)),
            ("method", string("shutdown")),
        ])
    }

    fn exit() -> Value {
        object([("jsonrpc", string("2.0")), ("method", string("exit"))])
    }

    fn did_open(uri: &str, version: i64, text: &str) -> Value {
        object([
            ("jsonrpc", string("2.0")),
            ("method", string("textDocument/didOpen")),
            (
                "params",
                object([(
                    "textDocument",
                    object([
                        ("uri", string(uri)),
                        ("version", number(version)),
                        ("text", string(text)),
                    ]),
                )]),
            ),
        ])
    }

    fn did_change(uri: &str, version: i64, text: &str) -> Value {
        object([
            ("jsonrpc", string("2.0")),
            ("method", string("textDocument/didChange")),
            (
                "params",
                object([
                    (
                        "textDocument",
                        object([("uri", string(uri)), ("version", number(version))]),
                    ),
                    (
                        "contentChanges",
                        Value::Array(vec![object([("text", string(text))])]),
                    ),
                ]),
            ),
        ])
    }

    fn change_configuration() -> Value {
        object([
            ("jsonrpc", string("2.0")),
            ("method", string("workspace/didChangeConfiguration")),
            ("params", object([("settings", object([]))])),
        ])
    }

    fn configure_pmcd_model(uri: &str) -> Value {
        object([
            ("jsonrpc", string("2.0")),
            ("method", string("workspace/didChangeConfiguration")),
            (
                "params",
                object([(
                    "settings",
                    object([(
                        "modelDocuments",
                        Value::Array(vec![object([
                            ("uri", string(uri)),
                            ("kind", string("pmcd")),
                        ])]),
                    )]),
                )]),
            ),
        ])
    }

    fn cancel_request(id: i64) -> Value {
        object([
            ("jsonrpc", string("2.0")),
            ("method", string("$/cancelRequest")),
            ("params", object([("id", number(id))])),
        ])
    }

    fn invalid_hover_request(id: i64) -> Value {
        invalid_request(id, "textDocument/hover")
    }

    fn invalid_definition_request(id: i64) -> Value {
        invalid_request(id, "textDocument/definition")
    }

    fn invalid_code_action_request(id: i64) -> Value {
        invalid_request(id, "textDocument/codeAction")
    }

    fn invalid_request(id: i64, method: &str) -> Value {
        object([
            ("jsonrpc", string("2.0")),
            ("id", number(id)),
            ("method", string(method)),
        ])
    }

    fn hover_request(id: i64, uri: &str) -> Value {
        request_with_position(id, "textDocument/hover", uri, 12)
    }

    fn definition_request(id: i64, uri: &str) -> Value {
        request_with_position(id, "textDocument/definition", uri, 60)
    }

    fn request_with_position(id: i64, method: &str, uri: &str, character: i64) -> Value {
        object([
            ("jsonrpc", string("2.0")),
            ("id", number(id)),
            ("method", string(method)),
            (
                "params",
                object([
                    ("textDocument", object([("uri", string(uri))])),
                    (
                        "position",
                        object([("line", number(0)), ("character", number(character))]),
                    ),
                ]),
            ),
        ])
    }

    fn code_action_request(id: i64, uri: &str) -> Value {
        object([
            ("jsonrpc", string("2.0")),
            ("id", number(id)),
            ("method", string("textDocument/codeAction")),
            (
                "params",
                object([
                    ("textDocument", object([("uri", string(uri))])),
                    (
                        "range",
                        object([
                            (
                                "start",
                                object([("line", number(0)), ("character", number(0))]),
                            ),
                            (
                                "end",
                                object([("line", number(0)), ("character", number(0))]),
                            ),
                        ]),
                    ),
                    (
                        "context",
                        object([("diagnostics", Value::Array(Vec::new()))]),
                    ),
                ]),
            ),
        ])
    }

    fn fixed_machine_query(source: &str) -> String {
        let insertion = source
            .rfind("))")
            .expect("machine-fix fixture has the generated call closure");
        let mut fixed = source.to_owned();
        fixed.insert_str(insertion, "%latest");
        fixed
    }

    fn object(fields: impl IntoIterator<Item = (&'static str, Value)>) -> Value {
        Value::Object(
            fields
                .into_iter()
                .map(|(name, value)| (name.to_owned(), value))
                .collect::<Map<_, _>>(),
        )
    }

    fn string(value: &str) -> Value {
        Value::String(value.to_owned())
    }

    fn number(value: i64) -> Value {
        Value::Number(value.into())
    }

    struct StreamingClient {
        input: Sender<Option<Vec<u8>>>,
        output: Receiver<Value>,
        deferred: Vec<Value>,
    }

    impl StreamingClient {
        fn new(input: Sender<Option<Vec<u8>>>, output: Receiver<Value>) -> Self {
            Self {
                input,
                output,
                deferred: Vec::new(),
            }
        }

        fn send(&self, message: Value) {
            self.input
                .send(Some(frame(&message)))
                .expect("server input must remain connected");
        }

        fn close_input(&self) {
            let _ = self.input.send(None);
        }

        fn response(&mut self, id: i64) -> Value {
            self.take_matching(|frame| frame.get("id").and_then(Value::as_i64) == Some(id))
        }

        fn diagnostics(&mut self, uri: &str, version: i64) -> Value {
            self.take_matching(|frame| {
                frame.get("method").and_then(Value::as_str)
                    == Some("textDocument/publishDiagnostics")
                    && frame
                        .get("params")
                        .and_then(|params| params.get("uri"))
                        .and_then(Value::as_str)
                        == Some(uri)
                    && frame
                        .get("params")
                        .and_then(|params| params.get("version"))
                        .and_then(Value::as_i64)
                        == Some(version)
            })
        }

        fn take_matching(&mut self, matches: impl Fn(&Value) -> bool) -> Value {
            if let Some(index) = self.deferred.iter().position(&matches) {
                return self.deferred.remove(index);
            }
            loop {
                let frame = self
                    .output
                    .recv_timeout(Duration::from_secs(5))
                    .expect("expected deterministic protocol frame");
                if matches(&frame) {
                    return frame;
                }
                self.deferred.push(frame);
            }
        }
    }

    struct StreamingReader {
        chunks: Receiver<Option<Vec<u8>>>,
        buffer: Vec<u8>,
        position: usize,
        closed: bool,
    }

    impl StreamingReader {
        fn new(chunks: Receiver<Option<Vec<u8>>>) -> Self {
            Self {
                chunks,
                buffer: Vec::new(),
                position: 0,
                closed: false,
            }
        }

        fn refill(&mut self) {
            while !self.closed && self.position == self.buffer.len() {
                match self.chunks.recv() {
                    Ok(Some(chunk)) => {
                        self.buffer = chunk;
                        self.position = 0;
                    }
                    Ok(None) | Err(_) => self.closed = true,
                }
            }
        }
    }

    impl Read for StreamingReader {
        fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
            let buffer = self.fill_buf()?;
            let length = cmp::min(buffer.len(), output.len());
            output[..length].copy_from_slice(&buffer[..length]);
            self.consume(length);
            Ok(length)
        }
    }

    impl BufRead for StreamingReader {
        fn fill_buf(&mut self) -> io::Result<&[u8]> {
            self.refill();
            Ok(&self.buffer[self.position..])
        }

        fn consume(&mut self, amount: usize) {
            self.position = self.position.saturating_add(amount).min(self.buffer.len());
        }
    }

    struct FrameWriter {
        output: Sender<Value>,
        buffer: Vec<u8>,
    }

    impl FrameWriter {
        fn new(output: Sender<Value>) -> Self {
            Self {
                output,
                buffer: Vec::new(),
            }
        }
    }

    impl Write for FrameWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.buffer.extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            if self.buffer.is_empty() {
                return Ok(());
            }
            let mut reader = Cursor::new(mem::take(&mut self.buffer));
            let frame = read_frame(&mut reader)?.ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "missing server response frame")
            })?;
            if read_frame(&mut reader)?.is_some() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "server flushed more than one response frame",
                ));
            }
            self.output.send(frame).map_err(|_| {
                io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "test client stopped receiving output",
                )
            })
        }
    }

    fn frame(message: &Value) -> Vec<u8> {
        let body = serde_json::to_vec(message).expect("test request must serialize");
        let mut framed = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
        framed.extend_from_slice(&body);
        framed
    }

    mod transcript_tests {
        //! In-process framed JSON-RPC transcripts for `Server::serve`.
        //!
        //! These tests drive `Server` directly and read its `pub(crate)` fields;
        //! only the true black-box tests that spawn the compiled binary remain in
        //! `tests/transcript.rs`.

        use std::io::Cursor;

        use libpure::explain;
        use serde_json::{Map, Value};

        use super::{Server, ServerExit};
        use crate::RequestId;

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
        const MILESTONING_MODEL: &str = r#"{
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
            assert_eq!(frames[0]["result"]["capabilities"]["hoverProvider"], true);
            assert_eq!(
                frames[0]["result"]["capabilities"]["textDocumentSync"],
                value(r#"{"openClose":true,"change":2,"save":{"includeText":false}}"#)
            );
            assert_eq!(
                frames[0]["result"]["capabilities"]["definitionProvider"],
                true
            );
            assert_eq!(
                frames[0]["result"]["capabilities"]["codeActionProvider"],
                value(r#"{"codeActionKinds":["quickfix"]}"#)
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
        fn local_definition_transcript_returns_the_same_document_declaration() {
            let uri = "untitled:local-definition";
            let source = "{row: Relation<(zeta:String[1], alpha:Integer[0..1])>| $row.alpha}";
            let mut server = Server::new();
            let mut output = Vec::new();
            let mut input = Cursor::new(transcript(&[
                value(r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#),
                did_open(uri, 1, source),
                definition_request(2, uri, 0, 60),
                value(r#"{"jsonrpc":"2.0","id":3,"method":"shutdown"}"#),
                value(r#"{"jsonrpc":"2.0","method":"exit"}"#),
            ]));

            assert_eq!(
                server
                    .serve(&mut input, &mut output)
                    .expect("valid local definition transcript"),
                ServerExit::Clean
            );
            let expected = definition_response(2, definition_location(uri, 0, 32, 0, 51));
            assert_eq!(response_for(&responses(&output), 2), &expected);
        }

        #[test]
        fn model_definition_transcript_returns_a_deterministic_cross_workspace_location() {
            let person_uri = "file:///workspace/person.pure";
            let manager_uri = "file:///workspace/manager.pure";
            let query_uri = "file:///workspace/query.pure";
            let person = "Class model::Person\n{\n  manager: model::Manager[0..1];\n}";
            let manager = "Class model::Manager\n{\n  name: String[1];\n}";
            let query = "model::Person.all()->filter(x| $x.manager.name)";
            let reference = query.find(".name").expect("name reference") + 1;
            let character = utf16_character(query, reference);
            let mut server = Server::new();
            let mut output = Vec::new();
            let mut input = Cursor::new(transcript(&[
                value(r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#),
                value(
                    r#"{"jsonrpc":"2.0","method":"workspace/didChangeConfiguration","params":{"settings":{"modelDocuments":[{"uri":"file:///workspace/person.pure","kind":"pure"},{"uri":"file:///workspace/manager.pure","kind":"pure"}]}}}"#,
                ),
                did_open(person_uri, 1, person),
                did_open(manager_uri, 1, manager),
                did_open(query_uri, 1, query),
                definition_request(2, query_uri, 0, character),
                definition_request(3, query_uri, 0, character),
                value(r#"{"jsonrpc":"2.0","id":4,"method":"shutdown"}"#),
                value(r#"{"jsonrpc":"2.0","method":"exit"}"#),
            ]));

            assert_eq!(
                server
                    .serve(&mut input, &mut output)
                    .expect("valid model definition transcript"),
                ServerExit::Clean
            );
            let expected = definition_location(manager_uri, 2, 2, 2, 18);
            let frames = responses(&output);
            assert_eq!(response_for(&frames, 2)["result"], expected);
            assert_eq!(response_for(&frames, 3)["result"], expected);
        }

        #[test]
        fn non_ascii_definition_transcript_uses_utf16_positions_for_request_and_target() {
            let uri = "untitled:unicode-definition";
            let source = "/* 😀 */ {row: Relation<(name:String[1])>| $row.name}";
            let reference = source.find("$row.name").expect("name reference") + "$row.".len();
            let character = utf16_character(source, reference);
            let mut server = Server::new();
            let mut output = Vec::new();
            let mut input = Cursor::new(transcript(&[
                value(r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#),
                did_open(uri, 1, source),
                definition_request(2, uri, 0, character),
                value(r#"{"jsonrpc":"2.0","id":3,"method":"shutdown"}"#),
                value(r#"{"jsonrpc":"2.0","method":"exit"}"#),
            ]));

            assert_eq!(
                server
                    .serve(&mut input, &mut output)
                    .expect("valid non-ASCII definition transcript"),
                ServerExit::Clean
            );
            let expected = definition_response(2, definition_location(uri, 0, 25, 0, 39));
            assert_eq!(response_for(&responses(&output), 2), &expected);
        }

        #[test]
        fn unavailable_definition_transcript_returns_null_consistently() {
            let uri = "untitled:unavailable-definition";
            let source = "model::Unknown.all()";
            let mut server = Server::new();
            let mut output = Vec::new();
            let mut input = Cursor::new(transcript(&[
                value(r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#),
                did_open(uri, 1, source),
                definition_request(2, uri, 0, 0),
                definition_request(3, uri, 0, 0),
                value(r#"{"jsonrpc":"2.0","id":4,"method":"shutdown"}"#),
                value(r#"{"jsonrpc":"2.0","method":"exit"}"#),
            ]));

            assert_eq!(
                server
                    .serve(&mut input, &mut output)
                    .expect("valid unavailable definition transcript"),
                ServerExit::Clean
            );
            let frames = responses(&output);
            assert_eq!(response_for(&frames, 2)["result"], Value::Null);
            assert_eq!(response_for(&frames, 3)["result"], Value::Null);
        }

        #[test]
        fn spanless_pmcd_definition_transcript_returns_null_consistently() {
            let model_uri = "untitled:spanless-model";
            let query_uri = "untitled:spanless-query";
            let query = "demo::Winner.all()->filter(x| $x.value)";
            let reference = query.find(".value").expect("value reference") + 1;
            let character = utf16_character(query, reference);
            let mut server = Server::new();
            let mut output = Vec::new();
            let mut input = Cursor::new(transcript(&[
                value(r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#),
                value(
                    r#"{"jsonrpc":"2.0","method":"workspace/didChangeConfiguration","params":{"settings":{"modelDocuments":[{"uri":"untitled:spanless-model","kind":"pmcd"}]}}}"#,
                ),
                did_open(model_uri, 1, PMCD_WINNER_MODEL),
                did_open(query_uri, 1, query),
                definition_request(2, query_uri, 0, character),
                definition_request(3, query_uri, 0, character),
                value(r#"{"jsonrpc":"2.0","id":4,"method":"shutdown"}"#),
                value(r#"{"jsonrpc":"2.0","method":"exit"}"#),
            ]));

            assert_eq!(
                server
                    .serve(&mut input, &mut output)
                    .expect("valid spanless definition transcript"),
                ServerExit::Clean
            );
            let frames = responses(&output);
            assert_eq!(response_for(&frames, 2)["result"], Value::Null);
            assert_eq!(response_for(&frames, 3)["result"], Value::Null);
        }

        #[test]
        fn unknown_cancellation_does_not_poison_front_end_state() {
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
                !server
                    .cancellation
                    .is_cancelled(&RequestId::String("work-7".into()))
            );
            assert!(server.cancellation.is_empty());
            let document = server
                .documents
                .get("file:///model.pure")
                .expect("open document");
            assert_eq!(document.text(), "Class A{}");
            assert_eq!(document.version(), Some(3));
            assert_eq!(
                server.configuration.settings(),
                Some(&value(r#"{"maxProblems":20}"#))
            );
        }

        #[test]
        fn unknown_numeric_cancellation_does_not_poison_identifier_reuse() {
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
            assert!(!server.cancellation.is_cancelled(&RequestId::Number(7)));
            assert!(server.cancellation.is_empty());
            assert!(server.documents.is_empty());
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
            assert!(server.documents.is_empty());
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
        fn incomplete_unicode_diagnostic_hover_uses_shared_explain_content() {
            let mut server = Server::new();
            let mut output = Vec::new();
            let mut input = Cursor::new(transcript(&[
                value(r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#),
                value(
                    r#"{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"untitled:unicode","version":1,"text":"/* 😀 */ [a,]"}}}"#,
                ),
                value(
                    r#"{"jsonrpc":"2.0","id":2,"method":"textDocument/hover","params":{"textDocument":{"uri":"untitled:unicode"},"position":{"line":0,"character":12}}}"#,
                ),
                value(r#"{"jsonrpc":"2.0","id":3,"method":"shutdown"}"#),
                value(r#"{"jsonrpc":"2.0","method":"exit"}"#),
            ]));

            assert_eq!(
                server
                    .serve(&mut input, &mut output)
                    .expect("valid transcript"),
                ServerExit::Clean
            );
            let frames = responses(&output);
            let hover = response_for(&frames, 2);
            assert_eq!(hover["result"]["contents"]["kind"], "markdown");
            assert_eq!(
                hover["result"]["contents"]["value"],
                expected_hover_markup("PUR1200")
            );
            assert_eq!(
                hover["result"]["range"],
                value(r#"{"start":{"line":0,"character":12},"end":{"line":0,"character":13}}"#)
            );
        }

        #[test]
        fn unavailable_hover_requests_have_a_stable_null_result() {
            let mut server = Server::new();
            let mut output = Vec::new();
            let mut input = Cursor::new(transcript(&[
                value(r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#),
                value(
                    r#"{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"untitled:unicode","version":1,"text":"/* 😀 */ [a,]"}}}"#,
                ),
                value(
                    r#"{"jsonrpc":"2.0","id":2,"method":"textDocument/hover","params":{"textDocument":{"uri":"untitled:unicode"},"position":{"line":0,"character":0}}}"#,
                ),
                value(
                    r#"{"jsonrpc":"2.0","id":3,"method":"textDocument/hover","params":{"textDocument":{"uri":"untitled:unicode"},"position":{"line":0,"character":4}}}"#,
                ),
                value(
                    r#"{"jsonrpc":"2.0","id":4,"method":"textDocument/hover","params":{"textDocument":{"uri":"untitled:missing"},"position":{"line":0,"character":0}}}"#,
                ),
                value(r#"{"jsonrpc":"2.0","id":5,"method":"shutdown"}"#),
                value(r#"{"jsonrpc":"2.0","method":"exit"}"#),
            ]));

            assert_eq!(
                server
                    .serve(&mut input, &mut output)
                    .expect("valid transcript"),
                ServerExit::Clean
            );
            let frames = responses(&output);
            for id in [2, 3, 4] {
                assert_eq!(response_for(&frames, id)["result"], Value::Null);
            }
        }

        #[test]
        fn malformed_hover_params_receive_a_json_rpc_invalid_params_error() {
            let mut server = Server::new();
            let mut output = Vec::new();
            let mut input = Cursor::new(transcript(&[
                value(r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#),
                value(
                    r#"{"jsonrpc":"2.0","id":2,"method":"textDocument/hover","params":{"textDocument":{"uri":"untitled:query"}}}"#,
                ),
                value(r#"{"jsonrpc":"2.0","id":3,"method":"shutdown"}"#),
                value(r#"{"jsonrpc":"2.0","method":"exit"}"#),
            ]));

            assert_eq!(
                server
                    .serve(&mut input, &mut output)
                    .expect("valid transcript"),
                ServerExit::Clean
            );
            let frames = responses(&output);
            assert_eq!(
                response_for(&frames, 2),
                &value(
                    r#"{"jsonrpc":"2.0","id":2,"error":{"code":-32602,"message":"invalid params"}}"#
                )
            );
        }

        #[test]
        fn code_action_transcript_emits_a_versioned_utf16_workspace_edit_for_only_the_requested_file()
         {
            let model_uri = "untitled:milestoning-model";
            let first_uri = "untitled:alpha-query";
            let second_uri = "untitled:beta-query";
            let first = "/* 😀 */ model::Source.all()->filter(x| $x.point(/* keep é */))";
            let second = "/* β */ model::Source.all()->filter(x| $x.point(/* keep é */))";
            let mut server = Server::new();
            let mut output = Vec::new();
            let mut input = Cursor::new(transcript(&[
                value(r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#),
                configure_pmcd_model(model_uri),
                did_open(model_uri, 3, MILESTONING_MODEL),
                did_open(first_uri, 7, first),
                did_open(second_uri, 11, second),
                code_action_request(2, first_uri),
                value(r#"{"jsonrpc":"2.0","id":3,"method":"shutdown"}"#),
                value(r#"{"jsonrpc":"2.0","method":"exit"}"#),
            ]));

            assert_eq!(
                server
                    .serve(&mut input, &mut output)
                    .expect("valid multi-file code action transcript"),
                ServerExit::Clean
            );
            // Both files have a machine-applicable fix, but the request named only
            // `first_uri`: the response must never bundle an edit to `second_uri`,
            // a document the client never asked about, into the same action.
            assert_eq!(
                response_for(&responses(&output), 2)["result"],
                machine_fix_action(vec![machine_fix_document_edit(first_uri, 7, first)])
            );
        }

        #[test]
        fn stale_code_actions_are_guarded_by_their_document_versions() {
            let model_uri = "untitled:stale-model";
            let query_uri = "untitled:stale-query";
            let unversioned_uri = "untitled:unversioned-query";
            let source = "model::Source.all()->filter(x| $x.point())";
            let fixed = fixed_machine_query(source);
            let first = run_transcript(&[
                value(r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#),
                configure_pmcd_model(model_uri),
                did_open(model_uri, 1, MILESTONING_MODEL),
                did_open(query_uri, 4, source),
                code_action_request(2, query_uri),
                value(r#"{"jsonrpc":"2.0","id":6,"method":"shutdown"}"#),
                value(r#"{"jsonrpc":"2.0","method":"exit"}"#),
            ]);
            assert_eq!(
                response_for(&first, 2)["result"],
                machine_fix_action(vec![machine_fix_document_edit(query_uri, 4, source)])
            );

            // The duplicate version is ignored, so the version-4 source remains
            // eligible for the same machine fix. Keep this successful request in an
            // independent session: it is a positive document-version regression test,
            // not an intentional in-flight overlap.
            let duplicate_version = run_transcript(&[
                value(r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#),
                configure_pmcd_model(model_uri),
                did_open(model_uri, 1, MILESTONING_MODEL),
                did_open(query_uri, 4, source),
                did_change_full(query_uri, 4, &fixed),
                code_action_request(3, query_uri),
                value(r#"{"jsonrpc":"2.0","id":6,"method":"shutdown"}"#),
                value(r#"{"jsonrpc":"2.0","method":"exit"}"#),
            ]);
            assert_eq!(
                response_for(&duplicate_version, 3)["result"],
                machine_fix_action(vec![machine_fix_document_edit(query_uri, 4, source)])
            );

            let changed = run_transcript(&[
                value(r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#),
                configure_pmcd_model(model_uri),
                did_open(model_uri, 1, MILESTONING_MODEL),
                did_open(query_uri, 4, source),
                did_change_full(query_uri, 5, &fixed),
                code_action_request(4, query_uri),
                value(r#"{"jsonrpc":"2.0","id":6,"method":"shutdown"}"#),
                value(r#"{"jsonrpc":"2.0","method":"exit"}"#),
            ]);
            assert_eq!(response_for(&changed, 4)["result"], value("[]"));

            let unversioned = run_transcript(&[
                value(r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#),
                configure_pmcd_model(model_uri),
                did_open(model_uri, 1, MILESTONING_MODEL),
                did_open_without_version(unversioned_uri, source),
                code_action_request(5, unversioned_uri),
                value(r#"{"jsonrpc":"2.0","id":6,"method":"shutdown"}"#),
                value(r#"{"jsonrpc":"2.0","method":"exit"}"#),
            ]);
            assert_eq!(response_for(&unversioned, 5)["result"], value("[]"));
        }

        #[test]
        fn code_action_transcript_omits_diagnostics_without_selected_machine_fixes() {
            let model_uri = "untitled:no-action-model";
            let query_uri = "untitled:no-action-query";
            let source = "model::Source.all()->filter(x| $x.point)";
            let mut server = Server::new();
            let mut output = Vec::new();
            let mut input = Cursor::new(transcript(&[
                value(r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#),
                configure_pmcd_model(model_uri),
                did_open(model_uri, 1, MILESTONING_MODEL),
                did_open(query_uri, 2, source),
                code_action_request(2, query_uri),
                value(r#"{"jsonrpc":"2.0","id":3,"method":"shutdown"}"#),
                value(r#"{"jsonrpc":"2.0","method":"exit"}"#),
            ]));

            assert_eq!(
                server
                    .serve(&mut input, &mut output)
                    .expect("valid no-action transcript"),
                ServerExit::Clean
            );
            assert_eq!(response_for(&responses(&output), 2)["result"], value("[]"));
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
                value(r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#),
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
                response_for(&responses(&output), 9),
                &value(
                    r#"{"jsonrpc":"2.0","id":9,"error":{"code":-32601,"message":"method not found"}}"#
                )
            );
        }

        #[test]
        fn request_before_initialize_receives_server_not_initialized_error() {
            let uri = "untitled:pre-init-request";
            let mut server = Server::new();
            let mut output = Vec::new();
            let mut input = Cursor::new(transcript(&[
                hover_request(5, uri, 0),
                value(r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#),
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
            assert_eq!(
                response_for(&frames, 5),
                &value(
                    r#"{"jsonrpc":"2.0","id":5,"error":{"code":-32002,"message":"server not initialized"}}"#
                )
            );
            assert!(response_for(&frames, 1)["result"].is_object());
        }

        #[test]
        fn notification_before_initialize_is_dropped_not_processed() {
            let uri = "untitled:pre-init-notification";
            let mut server = Server::new();
            let mut output = Vec::new();
            let mut input = Cursor::new(transcript(&[
                did_open(uri, 1, "Class A{}"),
                value(r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#),
                hover_request(2, uri, 0),
                value(r#"{"jsonrpc":"2.0","id":3,"method":"shutdown"}"#),
                value(r#"{"jsonrpc":"2.0","method":"exit"}"#),
            ]));

            assert_eq!(
                server
                    .serve(&mut input, &mut output)
                    .expect("valid transcript"),
                ServerExit::Clean
            );
            assert!(server.documents.get(uri).is_none());
            assert!(published_diagnostics(&output).is_empty());
            assert_eq!(response_for(&responses(&output), 2)["result"], Value::Null);
        }

        #[test]
        fn request_after_shutdown_receives_invalid_request_error() {
            let uri = "untitled:post-shutdown-request";
            let mut server = Server::new();
            let mut output = Vec::new();
            let mut input = Cursor::new(transcript(&[
                value(r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#),
                value(r#"{"jsonrpc":"2.0","id":2,"method":"shutdown"}"#),
                hover_request(3, uri, 0),
                value(r#"{"jsonrpc":"2.0","method":"exit"}"#),
            ]));

            assert_eq!(
                server
                    .serve(&mut input, &mut output)
                    .expect("valid transcript"),
                ServerExit::Clean
            );
            let frames = responses(&output);
            assert_eq!(response_for(&frames, 2)["result"], Value::Null);
            assert_eq!(
                response_for(&frames, 3),
                &value(
                    r#"{"jsonrpc":"2.0","id":3,"error":{"code":-32600,"message":"request received after shutdown"}}"#
                )
            );
        }

        #[test]
        fn no_server_initiated_message_precedes_the_initialize_response() {
            let uri = "untitled:lifecycle-repro";
            let source = "/* 😀 */ [a,]";
            let mut server = Server::new();
            let mut output = Vec::new();
            let mut input = Cursor::new(transcript(&[
                did_open(uri, 1, source),
                hover_request(5, uri, 12),
                value(r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#),
                value(r#"{"jsonrpc":"2.0","id":99,"method":"shutdown"}"#),
                hover_request(7, uri, 12),
                value(r#"{"jsonrpc":"2.0","method":"exit"}"#),
            ]));

            assert_eq!(
                server
                    .serve(&mut input, &mut output)
                    .expect("valid transcript"),
                ServerExit::Clean
            );
            let frames = responses(&output);
            let initialize_index = frames
                .iter()
                .position(|frame| frame["id"] == 1)
                .expect("initialize response frame");
            assert!(
                frames[..initialize_index]
                    .iter()
                    .all(|frame| frame.get("method").is_none()),
                "no server-initiated notification may precede the initialize response: {frames:?}"
            );
            assert_eq!(
                frames[initialize_index]["result"]["serverInfo"]["name"],
                "pure-analyzer-lsp"
            );
            assert_eq!(
                response_for(&frames, 5),
                &value(
                    r#"{"jsonrpc":"2.0","id":5,"error":{"code":-32002,"message":"server not initialized"}}"#
                )
            );
            assert_eq!(response_for(&frames, 99)["result"], Value::Null);
            assert_eq!(
                response_for(&frames, 7),
                &value(
                    r#"{"jsonrpc":"2.0","id":7,"error":{"code":-32600,"message":"request received after shutdown"}}"#
                )
            );
            assert!(published_diagnostics(&output).is_empty());
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

        fn published_diagnostics(output: &[u8]) -> Vec<Value> {
            responses(output)
                .into_iter()
                .filter(|message| message["method"] == "textDocument/publishDiagnostics")
                .collect()
        }

        fn response_for(frames: &[Value], id: i64) -> &Value {
            let responses = frames
                .iter()
                .filter(|frame| frame["id"] == id)
                .collect::<Vec<_>>();
            assert_eq!(responses.len(), 1, "expected one response for id {id}");
            responses[0]
        }

        fn expected_hover_markup(identifier: &str) -> String {
            let explanation = explain(identifier).expect("registered explain content");
            format!(
                "**`{}`** · {} / {}\n\n{}\n\n**Limit:** {}\n\n**Remedy:** {}\n\n[Documentation]({})",
                explanation.identifier,
                explanation.kind.as_str(),
                explanation.classification.as_str(),
                explanation.meaning,
                explanation.limit,
                explanation.remedy,
                explanation.documentation_url,
            )
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

        fn did_open_without_version(uri: &str, text: &str) -> Value {
            object([
                ("jsonrpc", Value::String("2.0".to_owned())),
                ("method", Value::String("textDocument/didOpen".to_owned())),
                (
                    "params",
                    object([(
                        "textDocument",
                        object([
                            ("uri", Value::String(uri.to_owned())),
                            ("text", Value::String(text.to_owned())),
                        ]),
                    )]),
                ),
            ])
        }

        fn did_change_full(uri: &str, version: i64, text: &str) -> Value {
            object([
                ("jsonrpc", Value::String("2.0".to_owned())),
                ("method", Value::String("textDocument/didChange".to_owned())),
                (
                    "params",
                    object([
                        (
                            "textDocument",
                            object([
                                ("uri", Value::String(uri.to_owned())),
                                ("version", Value::Number(version.into())),
                            ]),
                        ),
                        (
                            "contentChanges",
                            Value::Array(vec![object([("text", Value::String(text.to_owned()))])]),
                        ),
                    ]),
                ),
            ])
        }

        fn configure_pmcd_model(uri: &str) -> Value {
            object([
                ("jsonrpc", Value::String("2.0".to_owned())),
                (
                    "method",
                    Value::String("workspace/didChangeConfiguration".to_owned()),
                ),
                (
                    "params",
                    object([(
                        "settings",
                        object([(
                            "modelDocuments",
                            Value::Array(vec![object([
                                ("uri", Value::String(uri.to_owned())),
                                ("kind", Value::String("pmcd".to_owned())),
                            ])]),
                        )]),
                    )]),
                ),
            ])
        }

        fn code_action_request(id: i64, uri: &str) -> Value {
            object([
                ("jsonrpc", Value::String("2.0".to_owned())),
                ("id", Value::Number(id.into())),
                (
                    "method",
                    Value::String("textDocument/codeAction".to_owned()),
                ),
                (
                    "params",
                    object([
                        (
                            "textDocument",
                            object([("uri", Value::String(uri.to_owned()))]),
                        ),
                        (
                            "range",
                            object([
                                ("start", definition_position(0, 0)),
                                ("end", definition_position(0, 0)),
                            ]),
                        ),
                        (
                            "context",
                            object([("diagnostics", Value::Array(Vec::new()))]),
                        ),
                    ]),
                ),
            ])
        }

        fn machine_fix_action(document_changes: Vec<Value>) -> Value {
            Value::Array(vec![object([
                (
                    "title",
                    Value::String("Apply all machine-applicable fixes".to_owned()),
                ),
                ("kind", Value::String("quickfix".to_owned())),
                (
                    "edit",
                    object([("documentChanges", Value::Array(document_changes))]),
                ),
            ])])
        }

        fn machine_fix_document_edit(uri: &str, version: i64, source: &str) -> Value {
            let insertion = source
                .rfind("))")
                .expect("machine-fix fixture has the generated call closure");
            let character = utf16_character(source, insertion);
            object([
                (
                    "textDocument",
                    object([
                        ("uri", Value::String(uri.to_owned())),
                        ("version", Value::Number(version.into())),
                    ]),
                ),
                (
                    "edits",
                    Value::Array(vec![object([
                        (
                            "range",
                            object([
                                ("start", definition_position(0, character)),
                                ("end", definition_position(0, character)),
                            ]),
                        ),
                        ("newText", Value::String("%latest".to_owned())),
                    ])]),
                ),
            ])
        }

        fn fixed_machine_query(source: &str) -> String {
            let insertion = source
                .rfind("))")
                .expect("machine-fix fixture has the generated call closure");
            let mut fixed = source.to_owned();
            fixed.insert_str(insertion, "%latest");
            fixed
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

        fn definition_request(id: i64, uri: &str, line: u32, character: u32) -> Value {
            object([
                ("jsonrpc", Value::String("2.0".to_owned())),
                ("id", Value::Number(id.into())),
                (
                    "method",
                    Value::String("textDocument/definition".to_owned()),
                ),
                (
                    "params",
                    object([
                        (
                            "textDocument",
                            object([("uri", Value::String(uri.to_owned()))]),
                        ),
                        ("position", definition_position(line, character)),
                    ]),
                ),
            ])
        }

        fn hover_request(id: i64, uri: &str, character: u32) -> Value {
            object([
                ("jsonrpc", Value::String("2.0".to_owned())),
                ("id", Value::Number(id.into())),
                ("method", Value::String("textDocument/hover".to_owned())),
                (
                    "params",
                    object([
                        (
                            "textDocument",
                            object([("uri", Value::String(uri.to_owned()))]),
                        ),
                        ("position", definition_position(0, character)),
                    ]),
                ),
            ])
        }

        fn definition_response(id: i64, result: Value) -> Value {
            object([
                ("jsonrpc", Value::String("2.0".to_owned())),
                ("id", Value::Number(id.into())),
                ("result", result),
            ])
        }

        fn definition_location(
            uri: &str,
            start_line: u32,
            start_character: u32,
            end_line: u32,
            end_character: u32,
        ) -> Value {
            object([
                ("uri", Value::String(uri.to_owned())),
                (
                    "range",
                    object([
                        ("start", definition_position(start_line, start_character)),
                        ("end", definition_position(end_line, end_character)),
                    ]),
                ),
            ])
        }

        fn definition_position(line: u32, character: u32) -> Value {
            object([
                ("line", Value::Number(line.into())),
                ("character", Value::Number(character.into())),
            ])
        }

        fn utf16_character(text: &str, offset: usize) -> u32 {
            u32::try_from(text[..offset].encode_utf16().count())
                .expect("fixture position fits protocol")
        }

        fn transcript(messages: &[Value]) -> Vec<u8> {
            let mut output = Vec::new();
            for message in messages {
                let body = serde_json::to_vec(message).expect("test JSON must serialize");
                output.extend_from_slice(
                    format!("Content-Length: {}\r\n\r\n", body.len()).as_bytes(),
                );
                output.extend_from_slice(&body);
            }
            output
        }

        fn run_transcript(messages: &[Value]) -> Vec<Value> {
            let mut server = Server::new();
            let mut output = Vec::new();
            let mut input = Cursor::new(transcript(messages));
            assert_eq!(
                server
                    .serve(&mut input, &mut output)
                    .expect("valid transcript"),
                ServerExit::Clean
            );
            responses(&output)
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
                values.push(
                    serde_json::from_slice(&input[body_start..body_end]).expect("response JSON"),
                );
                input = &input[body_end..];
            }
            values
        }
    }
}
