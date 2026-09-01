use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, Write};

use libpure::{
    AnalysisDriver, DefinitionPosition, DefinitionResult, DefinitionTarget, Diagnostic,
    DriverError, FileId, LintRequest, ModelError, ModelInput, Severity, SourceInput, SourceRequest,
    TextRange, explain,
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
) -> Result<RequestWork, HoverError> {
    let request = hover_request(params).ok_or(HoverError::InvalidParams)?;
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
    publish_current_diagnostics(server, writer)
}

pub(crate) fn definition_work(server: &Server, params: Option<&Value>) -> Option<RequestWork> {
    let (uri, position) = definition_params(params)?;
    Some(RequestWork::Definition {
        snapshot: AnalysisSnapshot::capture(server),
        uri: uri.to_owned(),
        position,
    })
}

pub(crate) fn code_actions_work(server: &Server, params: Option<&Value>) -> Option<RequestWork> {
    let uri = code_action_uri(params)?;
    Some(RequestWork::CodeActions {
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum HoverError {
    InvalidParams,
}

fn hover_request(params: Option<&Value>) -> Option<HoverRequest<'_>> {
    let text_document = params?.get("textDocument")?;
    Some(HoverRequest {
        uri: text_document.get("uri")?.as_str()?,
        position: protocol_position(params?.get("position")?)?,
    })
}

fn publish_current_diagnostics<W: Write>(server: &Server, writer: &mut W) -> io::Result<()> {
    let snapshot = AnalysisSnapshot::capture(server);
    let diagnostics = snapshot.diagnostics();
    if !snapshot.is_current(server) {
        return Ok(());
    }
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
        if changes.is_empty() || !changes.iter().any(|change| change.file == requested_file) {
            return Some(Vec::new());
        }

        let document_changes = changes
            .iter()
            .map(|change| {
                let uri = files.get(&change.file)?;
                let document = self.documents.get(uri)?;
                versioned_document_edit(document, &change.before, &change.after)
            })
            .collect::<Option<Vec<_>>>()?;
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
                u32::try_from(index)
                    .ok()
                    .map(|index| (FileId::new(index), route.uri.clone()))
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
    let reason_content = diagnostic
        .reason
        .map(|reason| explain(reason.id()))
        .transpose()
        .ok()?;
    Some(hover_value(
        diagnostic_range(document, diagnostic)?,
        diagnostic_content,
        reason_content,
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

fn hover_sort_key(diagnostic: &Diagnostic) -> (usize, usize, usize, &str, &str, &str) {
    let start = usize::from(diagnostic.primary.span.start());
    let end = usize::from(diagnostic.primary.span.end());
    (
        end.saturating_sub(start),
        start,
        end,
        diagnostic.code.as_str(),
        diagnostic.reason.map_or("", |reason| reason.id()),
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
    use pure_analyzer_diagnostics::{Label, ReasonCode};
    use serde_json::Value;

    use super::{AnalysisSnapshot, diagnostic_hover};
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
    fn diagnostic_hover_appends_the_attached_reason_explanation() {
        let document =
            DocumentSnapshot::new("untitled:query".to_owned(), "query".to_owned(), Some(1));
        let diagnostic = Diagnostic::builder(
            DiagCode::EquivalenceVerdict,
            Severity::Info,
            "not used by hover markup",
            Label::new(FileId::new(0), TextRange::new(0.into(), 5.into())),
        )
        .reason(ReasonCode::IndUnparseable)
        .build();

        let hover = diagnostic_hover(&document, &diagnostic).expect("registered explanations");
        assert_eq!(
            hover["range"],
            value(r#"{"start":{"line":0,"character":0},"end":{"line":0,"character":5}}"#)
        );
        let diagnostic = explain("PUR3001").expect("registered diagnostic");
        let reason = explain("IND_UNPARSEABLE").expect("registered reason");
        assert_eq!(
            hover["contents"]["value"],
            format!(
                "**`{}`** · {} / {}\n\n{}\n\n**Limit:** {}\n\n**Remedy:** {}\n\n[Documentation]({})\n\n---\n\n**`{}`** · {} / {}\n\n{}\n\n**Limit:** {}\n\n**Remedy:** {}\n\n[Documentation]({})",
                diagnostic.identifier,
                diagnostic.kind.as_str(),
                diagnostic.classification.as_str(),
                diagnostic.meaning,
                diagnostic.limit,
                diagnostic.remedy,
                diagnostic.documentation_url,
                reason.identifier,
                reason.kind.as_str(),
                reason.classification.as_str(),
                reason.meaning,
                reason.limit,
                reason.remedy,
                reason.documentation_url,
            )
        );
    }

    fn value(source: &str) -> Value {
        serde_json::from_str(source).expect("test JSON must parse")
    }
}
