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
#[derive(Debug)]
pub struct Server {
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
    pub fn new() -> Self {
        Self {
            cancellation: CancellationRegistry::default(),
            configuration: WorkspaceConfiguration::default(),
            documents: DocumentStore::default(),
            lifecycle: Lifecycle::New,
            #[cfg(test)]
            request_barrier: None,
        }
    }

    /// Return the front-end document store.
    #[must_use]
    pub const fn documents(&self) -> &DocumentStore {
        &self.documents
    }

    /// Return the front-end workspace configuration boundary.
    #[must_use]
    pub const fn configuration(&self) -> &WorkspaceConfiguration {
        &self.configuration
    }

    /// Return the front-end cancellation boundary.
    #[must_use]
    pub const fn cancellation(&self) -> &CancellationRegistry {
        &self.cancellation
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
    pub fn serve<R: BufRead + Send, W: Write>(
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
}
