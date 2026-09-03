//! Versioned JSON diagnostic envelope.

use libpure::{LineColumn, SourceFile, SourceOrigin};
use pure_analyzer_diagnostics::{Applicability, FixProvenance, ReasonCode, Severity, TextRange};
use serde::Serialize;

use crate::{
    RenderError, RenderInput,
    input::{PreparedDiagnostic, PreparedEdit, PreparedInput, PreparedLabel, Summary},
};

const JSON_SCHEMA_VERSION: &str = "1.0";

pub(crate) fn render(input: RenderInput<'_>) -> Result<String, RenderError> {
    let prepared = PreparedInput::new(input)?;
    let envelope = JsonEnvelope {
        version: JSON_SCHEMA_VERSION,
        files: prepared.files.into_iter().map(json_file).collect(),
        diagnostics: prepared.diagnostics.iter().map(json_diagnostic).collect(),
        summary: json_summary(prepared.summary),
    };
    let mut output =
        serde_json::to_string_pretty(&envelope).map_err(|source| RenderError::Serialization {
            format: "JSON",
            source,
        })?;
    output.push('\n');
    Ok(output)
}

#[derive(Serialize)]
struct JsonEnvelope<'a> {
    version: &'static str,
    files: Vec<JsonFile<'a>>,
    diagnostics: Vec<JsonDiagnostic<'a>>,
    summary: JsonSummary,
}

#[derive(Serialize)]
struct JsonFile<'a> {
    id: u32,
    name: &'a str,
    origin: &'static str,
}

#[derive(Serialize)]
struct JsonDiagnostic<'a> {
    code: &'static str,
    severity: &'static str,
    message: &'a str,
    primary: JsonLabel<'a>,
    secondary: Vec<JsonLabel<'a>>,
    fix: Option<JsonFix<'a>>,
    reason: Option<ReasonCode>,
    url: Option<&'a str>,
}

#[derive(Serialize)]
struct JsonLabel<'a> {
    file: u32,
    range: JsonRange,
    note: &'a str,
}

#[derive(Serialize)]
struct JsonRange {
    start: JsonPosition,
    end: JsonPosition,
}

#[derive(Serialize)]
struct JsonPosition {
    byte: u32,
    line: usize,
    column: usize,
}

#[derive(Serialize)]
struct JsonFix<'a> {
    title: &'a str,
    applicability: Applicability,
    provenance: FixProvenance,
    edits: Vec<JsonEdit<'a>>,
}

#[derive(Serialize)]
struct JsonEdit<'a> {
    file: u32,
    range: JsonRange,
    replacement: &'a str,
}

#[derive(Serialize)]
struct JsonSummary {
    errors: usize,
    warnings: usize,
    info: usize,
    hints: usize,
    total: usize,
}

fn json_file(source: &SourceFile) -> JsonFile<'_> {
    JsonFile {
        id: source.id().index(),
        name: source.name(),
        origin: origin_name(source.origin()),
    }
}

const fn origin_name(origin: &SourceOrigin) -> &'static str {
    match origin {
        SourceOrigin::File { .. } => "file",
        SourceOrigin::InMemory => "memory",
        SourceOrigin::Stdin => "stdin",
    }
}

fn json_diagnostic<'a>(diagnostic: &PreparedDiagnostic<'a>) -> JsonDiagnostic<'a> {
    JsonDiagnostic {
        code: diagnostic.diagnostic.code.as_str(),
        severity: severity_name(diagnostic.diagnostic.severity),
        message: &diagnostic.diagnostic.message,
        primary: json_label(&diagnostic.primary),
        secondary: diagnostic.secondary.iter().map(json_label).collect(),
        fix: diagnostic.fix.as_ref().map(json_fix),
        reason: diagnostic.diagnostic.reason,
        url: diagnostic.diagnostic.url.as_deref(),
    }
}

fn json_label<'a>(label: &PreparedLabel<'a>) -> JsonLabel<'a> {
    JsonLabel {
        file: label.source.id().index(),
        range: json_range(label.span, label.start, label.end),
        note: label.note,
    }
}

fn json_fix<'a>(fix: &crate::input::PreparedFix<'a>) -> JsonFix<'a> {
    JsonFix {
        title: &fix.fix.title,
        applicability: fix.fix.applicability,
        provenance: fix.fix.provenance,
        edits: fix.edits.iter().map(json_edit).collect(),
    }
}

fn json_edit<'a>(edit: &PreparedEdit<'a>) -> JsonEdit<'a> {
    JsonEdit {
        file: edit.source.id().index(),
        range: json_range(edit.edit.span, edit.start, edit.end),
        replacement: &edit.edit.new_text,
    }
}

fn json_range(range: TextRange, start: LineColumn, end: LineColumn) -> JsonRange {
    JsonRange {
        start: json_position(range.start(), start),
        end: json_position(range.end(), end),
    }
}

fn json_position(byte: pure_analyzer_diagnostics::TextSize, location: LineColumn) -> JsonPosition {
    JsonPosition {
        byte: u32::from(byte),
        line: location.line,
        column: location.column,
    }
}

const fn severity_name(severity: Severity) -> &'static str {
    match severity {
        Severity::Error => "error",
        Severity::Warning => "warning",
        Severity::Info => "info",
        Severity::Hint => "hint",
    }
}

const fn json_summary(summary: Summary) -> JsonSummary {
    JsonSummary {
        errors: summary.errors,
        warnings: summary.warnings,
        info: summary.infos,
        hints: summary.hints,
        total: summary.total,
    }
}
