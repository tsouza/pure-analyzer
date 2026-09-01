use std::io::{self, Write};

use libpure::ExplainContent;
use serde_json::{Map, Value};

use crate::frame::write_frame;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct PublishedPosition {
    line: u32,
    character: u32,
}

impl PublishedPosition {
    pub(crate) const fn new(line: u32, character: u32) -> Self {
        Self { line, character }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct PublishedRange {
    start: PublishedPosition,
    end: PublishedPosition,
}

impl PublishedRange {
    pub(crate) const fn new(start: PublishedPosition, end: PublishedPosition) -> Self {
        Self { start, end }
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct PublishedDiagnostic {
    range: PublishedRange,
    severity: u8,
    code: Option<String>,
    message: String,
}

impl PublishedDiagnostic {
    pub(crate) fn new(range: PublishedRange, severity: u8, code: String, message: String) -> Self {
        Self {
            range,
            severity,
            code: Some(code),
            message,
        }
    }

    pub(crate) fn without_code(range: PublishedRange, severity: u8, message: String) -> Self {
        Self {
            range,
            severity,
            code: None,
            message,
        }
    }

    fn value(&self) -> Value {
        let mut value = Map::new();
        value.insert("range".to_owned(), range_value(self.range));
        value.insert("severity".to_owned(), Value::Number(self.severity.into()));
        if let Some(code) = &self.code {
            value.insert("code".to_owned(), Value::String(code.clone()));
        }
        value.insert(
            "source".to_owned(),
            Value::String("pure-analyzer".to_owned()),
        );
        value.insert("message".to_owned(), Value::String(self.message.clone()));
        Value::Object(value)
    }
}

pub(crate) fn send_result<W: Write>(writer: &mut W, id: Value, result: Value) -> io::Result<()> {
    write_frame(
        writer,
        &object([
            ("jsonrpc", Value::String("2.0".to_owned())),
            ("id", id),
            ("result", result),
        ]),
    )
}

pub(crate) fn send_error<W: Write>(
    writer: &mut W,
    id: Value,
    code: i64,
    message: &str,
) -> io::Result<()> {
    let error = object([
        ("code", Value::Number(code.into())),
        ("message", Value::String(message.to_owned())),
    ]);
    write_frame(
        writer,
        &object([
            ("jsonrpc", Value::String("2.0".to_owned())),
            ("id", id),
            ("error", error),
        ]),
    )
}

pub(crate) fn publish_diagnostics<W: Write>(
    writer: &mut W,
    uri: &str,
    version: Option<i64>,
    diagnostics: &[PublishedDiagnostic],
) -> io::Result<()> {
    let mut params = Map::new();
    params.insert("uri".to_owned(), Value::String(uri.to_owned()));
    if let Some(version) = version {
        params.insert("version".to_owned(), Value::Number(version.into()));
    }
    params.insert(
        "diagnostics".to_owned(),
        Value::Array(diagnostics.iter().map(PublishedDiagnostic::value).collect()),
    );
    write_frame(
        writer,
        &object([
            ("jsonrpc", Value::String("2.0".to_owned())),
            (
                "method",
                Value::String("textDocument/publishDiagnostics".to_owned()),
            ),
            ("params", Value::Object(params)),
        ]),
    )
}

pub(crate) fn hover_value(
    range: PublishedRange,
    diagnostic: &ExplainContent,
    reason: Option<&ExplainContent>,
) -> Value {
    object([
        (
            "contents",
            object([
                ("kind", Value::String("markdown".to_owned())),
                ("value", Value::String(hover_markup(diagnostic, reason))),
            ]),
        ),
        ("range", range_value(range)),
    ])
}

pub(crate) fn initialization_result() -> Value {
    let server_info = object([
        ("name", Value::String("pure-analyzer-lsp".to_owned())),
        (
            "version",
            Value::String(env!("CARGO_PKG_VERSION").to_owned()),
        ),
    ]);
    let text_document_sync = object([
        ("openClose", Value::Bool(true)),
        ("change", Value::Number(2.into())),
        ("save", object([("includeText", Value::Bool(false))])),
    ]);
    object([
        (
            "capabilities",
            object([
                ("positionEncoding", Value::String("utf-16".to_owned())),
                ("hoverProvider", Value::Bool(true)),
                ("textDocumentSync", text_document_sync),
                ("definitionProvider", Value::Bool(true)),
            ]),
        ),
        ("serverInfo", server_info),
    ])
}

fn range_value(range: PublishedRange) -> Value {
    object([
        ("start", position_value(range.start)),
        ("end", position_value(range.end)),
    ])
}

fn position_value(position: PublishedPosition) -> Value {
    object([
        ("line", Value::Number(position.line.into())),
        ("character", Value::Number(position.character.into())),
    ])
}

fn hover_markup(diagnostic: &ExplainContent, reason: Option<&ExplainContent>) -> String {
    let diagnostic = explanation_markup(diagnostic);
    if let Some(reason) = reason {
        format!("{diagnostic}\n\n---\n\n{}", explanation_markup(reason))
    } else {
        diagnostic
    }
}

fn explanation_markup(explanation: &ExplainContent) -> String {
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

fn object(fields: impl IntoIterator<Item = (&'static str, Value)>) -> Value {
    Value::Object(
        fields
            .into_iter()
            .map(|(name, value)| (name.to_owned(), value))
            .collect(),
    )
}
