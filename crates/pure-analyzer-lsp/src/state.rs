use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, Write};

use libpure::{
    AnalysisDriver, DefinitionPosition, DefinitionResult, DefinitionTarget, Diagnostic,
    DriverError, FileId, LintRequest, ModelError, ModelInput, Severity, SourceInput, SourceRequest,
    TextRange, explain, model_source_file_id,
};
use serde_json::{Map, Value};

use crate::{
    DocumentSnapshot, RequestId, Server,
    document::{ContentChange, ProtocolPosition, ProtocolRange, byte_offset, utf16_position},
    response::{
        PublishedDiagnostic, PublishedPosition, PublishedRange, hover_value, publish_diagnostics,
    },
    workspace::ModelDocumentKind,
};

const APPLY_MACHINE_FIXES_TITLE: &str = "Apply all machine-applicable fixes";
const QUICK_FIX_KIND: &str = "quickfix";

pub(crate) fn cancel(server: &mut Server, params: Option<&Value>) {
    if let Some(request) = params
        .and_then(|value| value.get("id"))
        .and_then(RequestId::from_json)
    {
        server.cancellation.cancel(request);
    }
}

/// A front-end request detached from the mutable server before analysis begins.
///
/// Every variant owns an immutable analysis snapshot, so workers can never
/// observe a later document or configuration revision while computing.
#[derive(Debug)]
pub(crate) enum RequestWork {
    /// A hover request at one protocol position.
    Hover {
        snapshot: AnalysisSnapshot,
        uri: String,
        position: ProtocolPosition,
    },
    /// A go-to-definition request at one protocol position.
    Definition {
        snapshot: AnalysisSnapshot,
        uri: String,
        position: ProtocolPosition,
    },
    /// A code-action request for one document.
    CodeActions {
        snapshot: AnalysisSnapshot,
        uri: String,
    },
}

/// The detached outcome of one LSP request worker.
#[derive(Debug)]
pub(crate) struct RequestCompletion {
    snapshot: AnalysisSnapshot,
    stale_result: Value,
    result: Value,
}

impl RequestWork {
    pub(crate) fn execute(self) -> RequestCompletion {
        match self {
            Self::Hover {
                snapshot,
                uri,
                position,
            } => RequestCompletion {
                result: snapshot.hover(&uri, position).unwrap_or(Value::Null),
                snapshot,
                stale_result: Value::Null,
            },
            Self::Definition {
                snapshot,
                uri,
                position,
            } => RequestCompletion {
                result: snapshot.definition(&uri, position),
                snapshot,
                stale_result: Value::Null,
            },
            Self::CodeActions { snapshot, uri } => RequestCompletion {
                result: Value::Array(snapshot.code_actions(&uri).unwrap_or_default()),
                snapshot,
                stale_result: Value::Array(Vec::new()),
            },
        }
    }
}

impl RequestCompletion {
    pub(crate) fn is_current(&self, server: &Server) -> bool {
        self.snapshot.is_current(server)
    }

    pub(crate) fn into_result(self) -> Value {
        self.result
    }

    pub(crate) fn stale_result(&self) -> &Value {
        &self.stale_result
    }
}

pub(crate) fn hover_work(
    server: &Server,
    params: Option<&Value>,
) -> Result<RequestWork, RequestParamsError> {
    let request = hover_request(params).ok_or(RequestParamsError::InvalidParams)?;
    Ok(RequestWork::Hover {
        snapshot: AnalysisSnapshot::capture(server),
        uri: request.uri.to_owned(),
        position: request.position,
    })
}

pub(crate) fn open_document<W: Write>(
    server: &mut Server,
    params: Option<&Value>,
    writer: &mut W,
) -> io::Result<()> {
    let Some(document) = params.and_then(|value| value.get("textDocument")) else {
        return Ok(());
    };
    let (Some(uri), Some(text)) = (
        document.get("uri").and_then(Value::as_str),
        document.get("text").and_then(Value::as_str),
    ) else {
        return Ok(());
    };
    if server.documents.replace(DocumentSnapshot::new(
        uri.to_owned(),
        text.to_owned(),
        document.get("version").and_then(Value::as_i64),
    )) {
        publish_current_diagnostics(server, writer)?;
    }
    Ok(())
}

pub(crate) fn change_document<W: Write>(
    server: &mut Server,
    params: Option<&Value>,
    writer: &mut W,
) -> io::Result<()> {
    let Some(document) = params.and_then(|value| value.get("textDocument")) else {
        return Ok(());
    };
    let (Some(uri), Some(version)) = (
        document.get("uri").and_then(Value::as_str),
        document.get("version").and_then(Value::as_i64),
    ) else {
        return Ok(());
    };
    let Some(changes) = content_changes(params) else {
        return Ok(());
    };
    if server.documents.apply_changes(uri, version, &changes) {
        publish_current_diagnostics(server, writer)?;
    }
    Ok(())
}

pub(crate) fn save_document<W: Write>(
    server: &mut Server,
    params: Option<&Value>,
    writer: &mut W,
) -> io::Result<()> {
    let Some(uri) = params
        .and_then(|value| value.get("textDocument"))
        .and_then(|value| value.get("uri"))
        .and_then(Value::as_str)
    else {
        return Ok(());
    };
    if server.documents.get(uri).is_some() {
        publish_current_diagnostics(server, writer)?;
    }
    Ok(())
}

pub(crate) fn close_document<W: Write>(
    server: &mut Server,
    params: Option<&Value>,
    writer: &mut W,
) -> io::Result<()> {
    let Some(uri) = params
        .and_then(|value| value.get("textDocument"))
        .and_then(|value| value.get("uri"))
        .and_then(Value::as_str)
    else {
        return Ok(());
    };
    let Some(document) = server.documents.remove(uri) else {
        return Ok(());
    };
    publish_diagnostics(writer, document.uri(), document.version(), &[])?;
    publish_current_diagnostics(server, writer)
}

pub(crate) fn update_configuration<W: Write>(
    server: &mut Server,
    params: Option<&Value>,
    writer: &mut W,
) -> io::Result<()> {
    let Some(settings) = params.and_then(|value| value.get("settings")).cloned() else {
        return Ok(());
    };
    server.configuration.replace(settings);
    warn_unopened_model_documents(server);
    publish_current_diagnostics(server, writer)
}

/// Warn once, at the moment `modelDocuments` is (re)configured, about any
/// route naming a URI the client has not opened. Analysis silently excludes
/// such a route (there is nothing to load), so without this the model is
/// simply missing from every subsequent diagnostic with no indication why.
fn warn_unopened_model_documents(server: &Server) {
    for uri in unopened_model_document_uris(server) {
        tracing::warn!(
            uri,
            "configured modelDocuments entry is not an open document; excluded from analysis \
             until the client opens it"
        );
    }
}

/// Every configured `modelDocuments` URI that is not a currently open document.
fn unopened_model_document_uris(server: &Server) -> Vec<&str> {
    server
        .configuration
        .model_documents()
        .iter()
        .map(|route| route.uri())
        .filter(|uri| server.documents.get(uri).is_none())
        .collect()
}

pub(crate) fn definition_work(
    server: &Server,
    params: Option<&Value>,
) -> Result<RequestWork, RequestParamsError> {
    let (uri, position) = definition_params(params).ok_or(RequestParamsError::InvalidParams)?;
    Ok(RequestWork::Definition {
        snapshot: AnalysisSnapshot::capture(server),
        uri: uri.to_owned(),
        position,
    })
}

pub(crate) fn code_actions_work(
    server: &Server,
    params: Option<&Value>,
) -> Result<RequestWork, RequestParamsError> {
    let uri = code_action_uri(params).ok_or(RequestParamsError::InvalidParams)?;
    Ok(RequestWork::CodeActions {
        snapshot: AnalysisSnapshot::capture(server),
        uri: uri.to_owned(),
    })
}

fn definition_params(params: Option<&Value>) -> Option<(&str, ProtocolPosition)> {
    let params = params?;
    let uri = params.get("textDocument")?.get("uri")?.as_str()?;
    let position = protocol_position(params.get("position")?)?;
    Some((uri, position))
}

/// Validate a `textDocument/codeAction` request's shape and return its
/// document URI.
///
/// `range` and `context.diagnostics` are validated (rejecting a malformed
/// request per the LSP spec) but their values are not threaded any further:
/// `libpure`'s `FixPlan` plans and previews a whole file's
/// machine-applicable fixes at once, with no per-diagnostic selection, so
/// there is nothing downstream yet that could narrow a code action to one
/// cursor position or one `context.diagnostics` entry within a document.
/// `AnalysisSnapshot`'s own `code_actions` instead scopes the returned
/// action to `uri`'s own file, so it never bundles an edit to a *different*
/// document into the same action.
fn code_action_uri(params: Option<&Value>) -> Option<&str> {
    let params = params?;
    let uri = params.get("textDocument")?.get("uri")?.as_str()?;
    let _ = protocol_range(params.get("range")?)?;
    let _ = params.get("context")?.get("diagnostics")?.as_array()?;
    Some(uri)
}

fn definition_location(
    snapshot: &AnalysisSnapshot,
    files: &BTreeMap<FileId, String>,
    target: DefinitionTarget,
) -> Option<Value> {
    let span = target.span()?;
    let uri = files.get(&target.file())?;
    let document = snapshot.documents.get(uri)?;
    let start = utf16_position(document.text(), usize::from(span.start()))?;
    let end = utf16_position(document.text(), usize::from(span.end()))?;
    let mut range = Map::new();
    range.insert("start".to_owned(), protocol_position_value(start));
    range.insert("end".to_owned(), protocol_position_value(end));
    let mut location = Map::new();
    location.insert("uri".to_owned(), Value::String(uri.clone()));
    location.insert("range".to_owned(), Value::Object(range));
    Some(Value::Object(location))
}

fn protocol_position_value(position: ProtocolPosition) -> Value {
    let mut value = Map::new();
    value.insert("line".to_owned(), Value::Number(position.line().into()));
    value.insert(
        "character".to_owned(),
        Value::Number(position.character().into()),
    );
    Value::Object(value)
}

fn protocol_range_value(start: ProtocolPosition, end: ProtocolPosition) -> Value {
    object([
        ("start", protocol_position_value(start)),
        ("end", protocol_position_value(end)),
    ])
}

fn object(fields: impl IntoIterator<Item = (&'static str, Value)>) -> Value {
    Value::Object(
        fields
            .into_iter()
            .map(|(name, value)| (name.to_owned(), value))
            .collect(),
    )
}

fn content_changes(params: Option<&Value>) -> Option<Vec<ContentChange>> {
    let changes = params?.get("contentChanges")?.as_array()?;
    if changes.is_empty() {
        return None;
    }
    changes.iter().map(content_change).collect()
}

fn content_change(change: &Value) -> Option<ContentChange> {
    let text = change.get("text")?.as_str()?.to_owned();
    let range = match change.get("range") {
        Some(range) => Some(protocol_range(range)?),
        None => None,
    };
    let range_length = match change.get("rangeLength") {
        Some(value) => Some(u32::try_from(value.as_u64()?).ok()?),
        None => None,
    };
    Some(ContentChange::new(range, range_length, text))
}

fn protocol_range(range: &Value) -> Option<ProtocolRange> {
    Some(ProtocolRange::new(
        protocol_position(range.get("start")?)?,
        protocol_position(range.get("end")?)?,
    ))
}

fn protocol_position(position: &Value) -> Option<ProtocolPosition> {
    Some(ProtocolPosition::new(
        u32::try_from(position.get("line")?.as_u64()?).ok()?,
        u32::try_from(position.get("character")?.as_u64()?).ok()?,
    ))
}

struct HoverRequest<'a> {
    uri: &'a str,
    position: ProtocolPosition,
}

/// The reason a request's parameters could not be turned into `RequestWork`.
///
/// Shared by every request kind (hover, definition, code actions) so a
/// malformed request surfaces the same `-32602 invalid params` protocol error
/// regardless of which handler received it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RequestParamsError {
    /// The request's `params` were missing or did not match the expected shape.
    InvalidParams,
}

fn hover_request(params: Option<&Value>) -> Option<HoverRequest<'_>> {
    let text_document = params?.get("textDocument")?;
    Some(HoverRequest {
        uri: text_document.get("uri")?.as_str()?,
        position: protocol_position(params?.get("position")?)?,
    })
}

/// Compute diagnostics for every open document and publish them.
///
/// Unlike hover/definition/codeAction, this runs synchronously on the
/// coordinator thread: every caller (`open_document`, `change_document`,
/// `save_document`, `close_document`, `update_configuration`) already holds
/// `&mut Server` and calls this inline, with no worker thread and no
/// intervening event-loop turn between the snapshot capture below and the
/// publish that follows. A currency check against `server` here would
/// therefore always compare a snapshot to the very immutable borrow it was
/// taken from — it can never observe a later revision, so one is
/// deliberately not kept (a check that can never fail reads as protection
/// it does not provide). This is an accepted asymmetry with the read-only
/// `RequestScheduler` path: keeping the hottest path (a lint on every edit)
/// simple and synchronous costs blocking the coordinator loop — including
/// `$/cancelRequest` handling for other in-flight requests — until the lint
/// completes.
fn publish_current_diagnostics<W: Write>(server: &Server, writer: &mut W) -> io::Result<()> {
    let snapshot = AnalysisSnapshot::capture(server);
    let diagnostics = snapshot.diagnostics();
    for document in snapshot.documents.values() {
        let findings = diagnostics
            .get(document.uri())
            .map_or(&[][..], Vec::as_slice);
        publish_diagnostics(writer, document.uri(), document.version(), findings)?;
    }
    Ok(())
}

#[derive(Clone, Debug)]
pub(crate) struct AnalysisSnapshot {
    document_revision: u64,
    configuration_revision: u64,
    documents: BTreeMap<String, DocumentSnapshot>,
    models: Vec<ModelRoute>,
}

impl AnalysisSnapshot {
    fn capture(server: &Server) -> Self {
        let documents = server
            .documents
            .iter()
            .map(|document| (document.uri().to_owned(), document.clone()))
            .collect::<BTreeMap<_, _>>();
        let models = server
            .configuration
            .model_documents()
            .iter()
            .filter(|route| documents.contains_key(route.uri()))
            .map(|route| ModelRoute {
                uri: route.uri().to_owned(),
                kind: route.kind(),
            })
            .collect();
        Self {
            document_revision: server.documents.revision(),
            configuration_revision: server.configuration.revision(),
            documents,
            models,
        }
    }

    fn is_current(&self, server: &Server) -> bool {
        self.document_revision == server.documents.revision()
            && self.configuration_revision == server.configuration.revision()
            && self.documents.len() == server.documents.len()
            && self
                .documents
                .iter()
                .all(|(uri, document)| server.documents.get(uri) == Some(document))
    }

    fn diagnostics(&self) -> BTreeMap<String, Vec<PublishedDiagnostic>> {
        let mut diagnostics = self
            .documents
            .keys()
            .map(|uri| (uri.clone(), Vec::new()))
            .collect::<BTreeMap<_, Vec<_>>>();
        if let Some((request, files)) = self.request() {
            match AnalysisDriver.lint(&request) {
                Ok(output) => {
                    self.append_diagnostics(&mut diagnostics, output.diagnostics(), &files)
                }
                Err(error) => self.report_driver_error(&mut diagnostics, error),
            }
        } else {
            self.validate_model_documents(&mut diagnostics);
        }
        for findings in diagnostics.values_mut() {
            findings.sort();
        }
        diagnostics
    }

    fn hover(&self, uri: &str, position: ProtocolPosition) -> Option<Value> {
        let document = self.documents.get(uri)?;
        let offset = byte_offset(document.text(), position)?;
        let (findings, files) = self.hover_findings()?;
        let diagnostic = findings
            .iter()
            .filter(|diagnostic| {
                files
                    .get(&diagnostic.primary.file)
                    .is_some_and(|file| file == uri)
            })
            .filter(|diagnostic| diagnostic_contains(diagnostic.primary.span, offset))
            .min_by(|left, right| hover_sort_key(left).cmp(&hover_sort_key(right)))?;
        diagnostic_hover(document, diagnostic)
    }

    fn definition(&self, uri: &str, position: ProtocolPosition) -> Value {
        let Some(document) = self.documents.get(uri) else {
            return Value::Null;
        };
        let Some(offset) = byte_offset(document.text(), position) else {
            return Value::Null;
        };
        let Some((request, files)) = self.request() else {
            return Value::Null;
        };
        let Some(file) = files
            .iter()
            .find_map(|(file, candidate)| (candidate == uri).then_some(*file))
        else {
            return Value::Null;
        };
        let Ok(offset) = u32::try_from(offset) else {
            return Value::Null;
        };

        match AnalysisDriver.definition(&request, DefinitionPosition::new(file, offset.into())) {
            Ok(DefinitionResult::Found(target)) => {
                definition_location(self, &files, target).unwrap_or(Value::Null)
            }
            Ok(DefinitionResult::Unavailable(_)) | Err(_) => Value::Null,
        }
    }

    fn code_actions(&self, uri: &str) -> Option<Vec<Value>> {
        let (request, files) = self.request()?;
        let requested_file = files
            .iter()
            .find_map(|(file, candidate)| (candidate == uri).then_some(*file))?;
        let output = AnalysisDriver.lint(&request).ok()?;
        let sources = output
            .sources()
            .files()
            .map(|source| (source.id(), source.text().to_owned()))
            .collect::<BTreeMap<_, _>>();
        let plan = output.plan_fixes().ok()?;
        let changes = plan.preview(&sources).ok()?;
        // Scoped to `requested_file`: a client asking for code actions on one
        // document must never receive an edit to a different one bundled
        // into the same action. `FixPlan` itself plans whole files, not
        // individual diagnostics, so an action here still applies every
        // machine-applicable fix *in the requested file*, not only the one
        // diagnostic under the client's cursor or in `context.diagnostics`.
        let requested_change = changes
            .iter()
            .find(|change| change.file == requested_file)?;
        let uri = files.get(&requested_change.file)?;
        let document = self.documents.get(uri)?;
        let document_changes = vec![versioned_document_edit(
            document,
            &requested_change.before,
            &requested_change.after,
        )?];
        Some(vec![code_action_value(document_changes)])
    }

    fn hover_findings(&self) -> Option<(Vec<Diagnostic>, BTreeMap<FileId, String>)> {
        if let Some((request, files)) = self.request() {
            return AnalysisDriver
                .lint(&request)
                .ok()
                .map(|output| (output.diagnostics().to_vec(), files));
        }
        let models = self.model_inputs();
        AnalysisDriver
            .validate_models(&models)
            .ok()
            .map(|diagnostics| (diagnostics, self.model_files()))
    }

    fn report_driver_error(
        &self,
        diagnostics: &mut BTreeMap<String, Vec<PublishedDiagnostic>>,
        error: DriverError,
    ) {
        let DriverError::ModelLoad { source } = error else {
            return;
        };
        let Some(uri) = self.model_error_uri(&source) else {
            return;
        };
        let Some(findings) = diagnostics.get_mut(uri) else {
            return;
        };
        let position = PublishedPosition::new(0, 0);
        findings.push(PublishedDiagnostic::without_code(
            PublishedRange::new(position, position),
            lsp_severity(Severity::Error),
            source.to_string(),
        ));
    }

    fn model_error_uri<'a>(&'a self, error: &ModelError) -> Option<&'a str> {
        match error {
            ModelError::Json { source_name, .. }
            | ModelError::PureParse { source_name, .. }
            | ModelError::InvalidDocument { source_name, .. }
            | ModelError::InvalidElement { source_name, .. } => self
                .models
                .iter()
                .find(|route| route.uri == source_name.as_str())
                .map(|route| route.uri.as_str()),
            ModelError::InvalidMergedGraph { source_id, .. } => usize::try_from(source_id.index())
                .ok()
                .and_then(|index| self.models.get(index))
                .map(|route| route.uri.as_str()),
            ModelError::TooManySources { index } => {
                self.models.get(*index).map(|route| route.uri.as_str())
            }
            ModelError::Read { .. } | ModelError::TooManyClasses { .. } => None,
        }
    }

    fn request(&self) -> Option<(LintRequest, BTreeMap<FileId, String>)> {
        let model_uris = self
            .models
            .iter()
            .map(|route| route.uri.as_str())
            .collect::<BTreeSet<_>>();
        let queries = self
            .documents
            .values()
            .filter(|document| !model_uris.contains(document.uri()))
            .collect::<Vec<_>>();
        if queries.is_empty() {
            return None;
        }
        let models = self.model_inputs();
        let request = LintRequest::new(
            SourceRequest::new(
                queries
                    .iter()
                    .map(|document| SourceInput::in_memory(document.uri(), document.text())),
            ),
            models,
        );
        let mut files = BTreeMap::new();
        for (index, route) in self.models.iter().enumerate() {
            files.insert(request.model_file_id(index)?, route.uri.clone());
        }
        for (index, document) in queries.iter().enumerate() {
            files.insert(request.query_file_id(index)?, document.uri().to_owned());
        }
        Some((request, files))
    }

    fn validate_model_documents(
        &self,
        diagnostics: &mut BTreeMap<String, Vec<PublishedDiagnostic>>,
    ) {
        let models = self.model_inputs();
        if models.is_empty() {
            return;
        }
        match AnalysisDriver.validate_models(&models) {
            Ok(model_diagnostics) => {
                self.append_diagnostics(diagnostics, &model_diagnostics, &self.model_files());
            }
            Err(error) => self.report_driver_error(diagnostics, error),
        }
    }

    fn append_diagnostics(
        &self,
        diagnostics: &mut BTreeMap<String, Vec<PublishedDiagnostic>>,
        findings: &[Diagnostic],
        files: &BTreeMap<FileId, String>,
    ) {
        for diagnostic in findings {
            let Some(uri) = files.get(&diagnostic.primary.file) else {
                continue;
            };
            let Some(document) = self.documents.get(uri) else {
                continue;
            };
            let Some(rendered) = render_diagnostic(document, diagnostic) else {
                continue;
            };
            if let Some(findings) = diagnostics.get_mut(uri) {
                findings.push(rendered);
            }
        }
    }

    fn model_files(&self) -> BTreeMap<FileId, String> {
        self.models
            .iter()
            .enumerate()
            .filter_map(|(index, route)| {
                model_source_file_id(index).map(|file_id| (file_id, route.uri.clone()))
            })
            .collect()
    }

    fn model_inputs(&self) -> Vec<ModelInput> {
        self.models
            .iter()
            .filter_map(|route| {
                self.documents
                    .get(&route.uri)
                    .map(|document| (route, document))
            })
            .map(|(route, document)| match route.kind {
                ModelDocumentKind::Pmcd => {
                    ModelInput::pmcd(SourceInput::in_memory(document.uri(), document.text()))
                }
                ModelDocumentKind::Pure => {
                    ModelInput::pure(SourceInput::in_memory(document.uri(), document.text()))
                }
            })
            .collect()
    }
}

fn code_action_value(document_changes: Vec<Value>) -> Value {
    object([
        ("title", Value::String(APPLY_MACHINE_FIXES_TITLE.to_owned())),
        ("kind", Value::String(QUICK_FIX_KIND.to_owned())),
        (
            "edit",
            object([("documentChanges", Value::Array(document_changes))]),
        ),
    ])
}

fn versioned_document_edit(
    document: &DocumentSnapshot,
    before: &str,
    after: &str,
) -> Option<Value> {
    let version = document.version()?;
    let edit = workspace_text_edit(document, before, after)?;
    Some(object([
        (
            "textDocument",
            object([
                ("uri", Value::String(document.uri().to_owned())),
                ("version", Value::Number(version.into())),
            ]),
        ),
        ("edits", Value::Array(vec![edit])),
    ]))
}

fn workspace_text_edit(document: &DocumentSnapshot, before: &str, after: &str) -> Option<Value> {
    if before == after || document.text() != before {
        return None;
    }
    let (start_byte, before_end, after_end) = replacement_bounds(before, after);
    let start = utf16_position(before, start_byte)?;
    let end = utf16_position(before, before_end)?;
    let new_text = after.get(start_byte..after_end)?.to_owned();
    Some(object([
        ("range", protocol_range_value(start, end)),
        ("newText", Value::String(new_text)),
    ]))
}

fn replacement_bounds(before: &str, after: &str) -> (usize, usize, usize) {
    let prefix = shared_prefix_len(before, after);
    let mut before_end = before.len();
    let mut after_end = after.len();
    while before_end > prefix && after_end > prefix {
        let Some(before_character) = before[..before_end].chars().next_back() else {
            break;
        };
        let Some(after_character) = after[..after_end].chars().next_back() else {
            break;
        };
        if before_character != after_character {
            break;
        }
        before_end = before_end.saturating_sub(before_character.len_utf8());
        after_end = after_end.saturating_sub(after_character.len_utf8());
    }
    (prefix, before_end, after_end)
}

fn shared_prefix_len(before: &str, after: &str) -> usize {
    let mut length = 0_usize;
    for (before_character, after_character) in before.chars().zip(after.chars()) {
        if before_character != after_character {
            break;
        }
        length = length.saturating_add(before_character.len_utf8());
    }
    length
}

#[derive(Clone, Debug)]
struct ModelRoute {
    uri: String,
    kind: ModelDocumentKind,
}

fn render_diagnostic(
    document: &DocumentSnapshot,
    diagnostic: &Diagnostic,
) -> Option<PublishedDiagnostic> {
    let range = diagnostic_range(document, diagnostic)?;
    Some(PublishedDiagnostic::new(
        range,
        lsp_severity(diagnostic.severity),
        diagnostic.code.as_str().to_owned(),
        diagnostic.message.clone(),
    ))
}

fn diagnostic_hover(document: &DocumentSnapshot, diagnostic: &Diagnostic) -> Option<Value> {
    let diagnostic_content = explain(diagnostic.code.as_str()).ok()?;
    Some(hover_value(
        diagnostic_range(document, diagnostic)?,
        diagnostic_content,
    ))
}

fn diagnostic_range(
    document: &DocumentSnapshot,
    diagnostic: &Diagnostic,
) -> Option<PublishedRange> {
    let start = utf16_position(
        document.text(),
        usize::from(diagnostic.primary.span.start()),
    )?;
    let end = utf16_position(document.text(), usize::from(diagnostic.primary.span.end()))?;
    Some(PublishedRange::new(
        PublishedPosition::new(start.line(), start.character()),
        PublishedPosition::new(end.line(), end.character()),
    ))
}

fn diagnostic_contains(span: TextRange, offset: usize) -> bool {
    let start = usize::from(span.start());
    let end = usize::from(span.end());
    (start == end && offset == start) || (start <= offset && offset < end)
}

fn hover_sort_key(diagnostic: &Diagnostic) -> (usize, usize, usize, &str, &str) {
    let start = usize::from(diagnostic.primary.span.start());
    let end = usize::from(diagnostic.primary.span.end());
    (
        end.saturating_sub(start),
        start,
        end,
        diagnostic.code.as_str(),
        diagnostic.message.as_str(),
    )
}

const fn lsp_severity(severity: Severity) -> u8 {
    match severity {
        Severity::Error => 1,
        Severity::Warning => 2,
        Severity::Info => 3,
        Severity::Hint => 4,
    }
}

#[cfg(test)]
mod tests {
    use libpure::{DiagCode, Diagnostic, FileId, Severity, TextRange, explain};
    use pure_analyzer_diagnostics::Label;
    use serde_json::Value;

    use super::{
        AnalysisSnapshot, diagnostic_contains, diagnostic_hover, hover_sort_key,
        replacement_bounds, shared_prefix_len, unopened_model_document_uris,
        warn_unopened_model_documents, workspace_text_edit,
    };
    use crate::{DocumentSnapshot, Server};

    #[test]
    fn snapshot_refuses_stale_document_and_configuration_state() {
        let uri = "untitled:query";
        let mut server = Server::new();
        assert!(server.documents.replace(DocumentSnapshot::new(
            uri.to_owned(),
            "[first]".to_owned(),
            Some(1),
        )));

        let document_snapshot = AnalysisSnapshot::capture(&server);
        assert!(document_snapshot.is_current(&server));
        assert!(server.documents.replace(DocumentSnapshot::new(
            uri.to_owned(),
            "[second]".to_owned(),
            Some(2),
        )));
        assert!(!document_snapshot.is_current(&server));

        let configuration_snapshot = AnalysisSnapshot::capture(&server);
        assert!(configuration_snapshot.is_current(&server));
        server
            .configuration
            .replace(value(r#"{"modelDocuments":[]}"#));
        assert!(!configuration_snapshot.is_current(&server));
    }

    #[test]
    fn diagnostic_hover_renders_the_registered_explanation() {
        let document =
            DocumentSnapshot::new("untitled:query".to_owned(), "query".to_owned(), Some(1));
        let diagnostic = Diagnostic::builder(
            DiagCode::UnknownProperty,
            Severity::Info,
            "not used by hover markup",
            Label::new(FileId::new(0), TextRange::new(0.into(), 5.into())),
        )
        .build();

        let hover = diagnostic_hover(&document, &diagnostic).expect("registered explanations");
        assert_eq!(
            hover["range"],
            value(r#"{"start":{"line":0,"character":0},"end":{"line":0,"character":5}}"#)
        );
        assert_eq!(
            hover["contents"]["kind"],
            Value::String("markdown".to_owned())
        );

        // Assert every field of the registered explanation surfaces in the
        // rendered markup, without mirroring `explanation_markup`'s own
        // format string: a change to how those fields are laid out should
        // not itself break this test, only a change to which content it
        // carries.
        let expected = explain("PUR2002").expect("registered diagnostic");
        let markup = hover["contents"]["value"]
            .as_str()
            .expect("hover markup is a string");
        for field in [
            expected.identifier,
            expected.kind.as_str(),
            expected.classification.as_str(),
            expected.meaning,
            expected.limit,
            expected.remedy,
            expected.documentation_url,
        ] {
            assert!(
                markup.contains(field),
                "hover markup missing {field:?}: {markup}"
            );
        }
    }

    #[test]
    fn unopened_model_document_uris_reports_only_configured_routes_not_currently_open() {
        let mut server = Server::new();
        assert!(server.documents.replace(DocumentSnapshot::new(
            "untitled:open-model".to_owned(),
            "Class A {}".to_owned(),
            Some(1),
        )));
        server.configuration.replace(value(
            r#"{"modelDocuments":[
                {"uri":"untitled:open-model","kind":"pure"},
                {"uri":"untitled:never-opened","kind":"pmcd"}
            ]}"#,
        ));

        assert_eq!(
            unopened_model_document_uris(&server),
            vec!["untitled:never-opened"]
        );
    }

    #[test]
    fn warn_unopened_model_documents_logs_each_excluded_uri() {
        use std::{
            io,
            sync::{Arc, Mutex},
        };

        #[derive(Clone, Default)]
        struct CapturedLog(Arc<Mutex<Vec<u8>>>);

        impl io::Write for CapturedLog {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                self.0
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .extend_from_slice(bytes);
                Ok(bytes.len())
            }

            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }

        let mut server = Server::new();
        server.configuration.replace(value(
            r#"{"modelDocuments":[{"uri":"untitled:never-opened","kind":"pure"}]}"#,
        ));

        let captured = CapturedLog::default();
        let for_writer = captured.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(move || for_writer.clone())
            .with_ansi(false)
            .finish();

        // `warn_unopened_model_documents` has no return value; a mutant that
        // replaces its body with `()` is only observable through the
        // `tracing::warn!` side effect it is supposed to have, so this
        // captures the subscriber's actual output rather than a call count.
        tracing::subscriber::with_default(subscriber, || {
            warn_unopened_model_documents(&server);
        });

        let log = String::from_utf8(captured.0.lock().unwrap_or_else(|p| p.into_inner()).clone())
            .expect("log output is UTF-8");
        assert!(
            log.contains("untitled:never-opened"),
            "expected the unopened URI in the warning, got: {log}"
        );
        assert!(
            log.contains("excluded from analysis"),
            "expected the warning message, got: {log}"
        );
    }

    #[test]
    fn hover_sort_key_prefers_the_narrowest_enclosing_span_regardless_of_code_order() {
        let narrow = diagnostic_at(DiagCode::UnresolvedModelAssociation, 10, 12, "narrow");
        let wide = diagnostic_at(DiagCode::BadToken, 5, 20, "wide");

        assert!(hover_sort_key(&narrow) < hover_sort_key(&wide));
        assert_eq!(narrowest(&[&wide, &narrow]), &narrow);
    }

    #[test]
    fn hover_sort_key_breaks_equal_width_ties_by_the_earlier_start() {
        let earlier = diagnostic_at(DiagCode::UnresolvedModelAssociation, 4, 9, "earlier");
        let later = diagnostic_at(DiagCode::BadToken, 10, 15, "later");
        assert_eq!(
            span_width(&earlier),
            span_width(&later),
            "fixture widths must tie"
        );

        assert!(hover_sort_key(&earlier) < hover_sort_key(&later));
        assert_eq!(narrowest(&[&later, &earlier]), &earlier);
    }

    #[test]
    fn hover_sort_key_breaks_identical_span_ties_by_the_lexically_smaller_code() {
        // The message ordering is deliberately the *opposite* of the code
        // ordering: `lower_code` only wins because the code field is
        // compared, not because its message happens to sort first.
        let higher_code = diagnostic_at(DiagCode::BadToken, 0, 5, "aaa");
        let lower_code = diagnostic_at(DiagCode::UnterminatedIsland, 0, 5, "zzz");

        assert!(hover_sort_key(&lower_code) < hover_sort_key(&higher_code));
        assert_eq!(narrowest(&[&higher_code, &lower_code]), &lower_code);
    }

    #[test]
    fn hover_sort_key_breaks_identical_span_and_code_ties_by_the_lexically_smaller_message() {
        let later_message = diagnostic_at(DiagCode::BadToken, 0, 5, "zzz");
        let earlier_message = diagnostic_at(DiagCode::BadToken, 0, 5, "aaa");

        assert!(hover_sort_key(&earlier_message) < hover_sort_key(&later_message));
        assert_eq!(
            narrowest(&[&later_message, &earlier_message]),
            &earlier_message
        );
    }

    #[test]
    fn diagnostic_contains_treats_the_span_end_as_exclusive_and_the_start_as_inclusive() {
        let span = TextRange::new(3.into(), 7.into());
        assert!(!diagnostic_contains(span, 2));
        assert!(diagnostic_contains(span, 3));
        assert!(diagnostic_contains(span, 6));
        assert!(!diagnostic_contains(span, 7));
    }

    #[test]
    fn diagnostic_contains_matches_a_zero_width_span_only_at_its_own_offset() {
        let span = TextRange::new(5.into(), 5.into());
        assert!(!diagnostic_contains(span, 4));
        assert!(diagnostic_contains(span, 5));
        assert!(!diagnostic_contains(span, 6));
    }

    #[test]
    fn workspace_text_edit_is_none_for_an_unchanged_document() {
        let document =
            DocumentSnapshot::new("untitled:query".to_owned(), "same".to_owned(), Some(1));
        // `before == after`: no edit is needed, regardless of the document's
        // live text. Guards the `before == after || text != before` OR: a
        // mutant weakening it to `&&` would only refuse here if the document
        // had *also* drifted, so it would instead synthesize a spurious
        // zero-width edit.
        assert_eq!(workspace_text_edit(&document, "same", "same"), None);
    }

    #[test]
    fn workspace_text_edit_is_none_against_a_stale_document_snapshot() {
        let document =
            DocumentSnapshot::new("untitled:query".to_owned(), "current".to_owned(), Some(1));
        // The live document text no longer matches `before`: computing a
        // diff against it would produce an edit against content the client
        // no longer has. Guards the other side of the OR above.
        assert_eq!(
            workspace_text_edit(&document, "stale", "new"),
            None,
            "a diff computed against a stale snapshot must be refused"
        );
    }

    #[test]
    fn replacement_bounds_isolates_a_pure_append() {
        assert_eq!(replacement_bounds("abc", "abcd"), (3, 3, 4));
    }

    #[test]
    fn replacement_bounds_isolates_a_pure_prepend_at_byte_zero() {
        assert_eq!(replacement_bounds("bcd", "abcd"), (0, 0, 1));
    }

    #[test]
    fn replacement_bounds_treats_a_disjoint_replacement_as_a_full_span_swap() {
        assert_eq!(replacement_bounds("abc", "xyz"), (0, 3, 3));
    }

    #[test]
    fn replacement_bounds_shrinks_around_a_multi_byte_edit_at_character_boundaries() {
        assert_eq!(replacement_bounds("a😀b", "a😀c"), (5, 6, 6));
    }

    #[test]
    fn replacement_bounds_walks_a_multi_byte_character_inside_the_matched_suffix() {
        // The shared suffix ("π" then "y") is only reached by *matching*
        // through the two-byte "π", not by a mismatch at the boundary — this
        // exercises the loop's `len_utf8` step itself, unlike a fixture where
        // the multi-byte character only ever appears in the shared prefix.
        assert_eq!(replacement_bounds("xπy", "zπy"), (0, 1, 1));
    }

    #[test]
    fn replacement_bounds_stops_the_before_side_exactly_at_the_shared_prefix() {
        // The shared suffix walk must stop as soon as `before_end` reaches
        // `prefix`, even though `after_end` is still above it and the next
        // characters on both sides happen to coincide ('a' inside the
        // prefix vs. the leading 'a' just past it in `after`). A `>` mutated
        // to `>=` on the `before_end` bound — or the guard's `&&` weakened
        // to `||` — lets the walk take one extra, wrong step here.
        assert_eq!(replacement_bounds("aXY", "aaXY"), (1, 1, 2));
    }

    #[test]
    fn replacement_bounds_stops_the_after_side_exactly_at_the_shared_prefix() {
        // Mirror of the above with the roles swapped, isolating a `>`
        // mutated to `>=` on the `after_end` bound specifically.
        assert_eq!(replacement_bounds("aaXY", "aXY"), (1, 2, 1));
    }

    #[test]
    fn shared_prefix_len_counts_bytes_not_characters() {
        assert_eq!(shared_prefix_len("a😀b", "a😀c"), 5);
        assert_eq!(shared_prefix_len("abc", "xyz"), 0);
        assert_eq!(shared_prefix_len("abc", "abc"), 3);
    }

    fn span_width(diagnostic: &Diagnostic) -> usize {
        usize::from(diagnostic.primary.span.end()) - usize::from(diagnostic.primary.span.start())
    }

    fn diagnostic_at(code: DiagCode, start: u32, end: u32, message: &str) -> Diagnostic {
        Diagnostic::builder(
            code,
            Severity::Info,
            message,
            Label::new(FileId::new(0), TextRange::new(start.into(), end.into())),
        )
        .build()
    }

    /// Selects the same way `AnalysisSnapshot::hover` selects among the
    /// diagnostics that contain the hovered offset.
    fn narrowest<'a>(diagnostics: &[&'a Diagnostic]) -> &'a Diagnostic {
        diagnostics
            .iter()
            .copied()
            .min_by(|left, right| hover_sort_key(left).cmp(&hover_sort_key(right)))
            .expect("non-empty diagnostic slice")
    }

    fn value(source: &str) -> Value {
        serde_json::from_str(source).expect("test JSON must parse")
    }
}
