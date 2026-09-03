//! SARIF 2.1.0 diagnostic log rendering.

use std::collections::BTreeMap;

use libpure::{LineColumn, SourceFile};
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use pure_analyzer_diagnostics::{
    Applicability, DiagCode, DiagFamily, FixProvenance, ReasonCode, Severity, TextRange,
};
use serde::Serialize;

use crate::{
    RenderError, RenderInput,
    input::{PreparedDiagnostic, PreparedEdit, PreparedInput, PreparedLabel},
};

const SARIF_SCHEMA: &str = "https://json.schemastore.org/sarif-2.1.0.json";
const SARIF_VERSION: &str = "2.1.0";
const SARIF_COLUMN_KIND: &str = "unicodeCodePoints";
const PROJECT_URI: &str = "https://github.com/tsouza/pure-analyzer";
const DOCUMENTATION_URI: &str = "https://github.com/tsouza/pure-analyzer/tree/main/docs";
const ARTIFACT_PATH_ENCODE_SET: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'/')
    .remove(b'~');

pub(crate) fn render(input: RenderInput<'_>) -> Result<String, RenderError> {
    let prepared = PreparedInput::new(input)?;
    let log = SarifLog {
        schema: SARIF_SCHEMA,
        version: SARIF_VERSION,
        runs: vec![SarifRun {
            tool: SarifTool {
                driver: SarifDriver {
                    name: "pure-analyzer",
                    information_uri: PROJECT_URI,
                    rules: sarif_rules(&prepared.diagnostics),
                },
            },
            column_kind: SARIF_COLUMN_KIND,
            results: prepared.diagnostics.iter().map(sarif_result).collect(),
            invocations: vec![SarifInvocation {
                execution_successful: true,
            }],
        }],
    };
    let mut output =
        serde_json::to_string_pretty(&log).map_err(|source| RenderError::Serialization {
            format: "SARIF",
            source,
        })?;
    output.push('\n');
    Ok(output)
}

#[derive(Serialize)]
struct SarifLog<'a> {
    #[serde(rename = "$schema")]
    schema: &'static str,
    version: &'static str,
    runs: Vec<SarifRun<'a>>,
}

#[derive(Serialize)]
struct SarifRun<'a> {
    tool: SarifTool<'a>,
    #[serde(rename = "columnKind")]
    column_kind: &'static str,
    results: Vec<SarifResult<'a>>,
    invocations: Vec<SarifInvocation>,
}

#[derive(Serialize)]
struct SarifTool<'a> {
    driver: SarifDriver<'a>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SarifDriver<'a> {
    name: &'static str,
    information_uri: &'static str,
    rules: Vec<SarifRule<'a>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SarifRule<'a> {
    id: &'static str,
    name: &'static str,
    short_description: SarifMessage<'a>,
    help_uri: &'static str,
    default_configuration: SarifConfiguration,
    properties: SarifRuleProperties,
}

#[derive(Serialize)]
struct SarifConfiguration {
    level: &'static str,
}

#[derive(Serialize)]
struct SarifRuleProperties {
    family: &'static str,
}

#[derive(Serialize)]
struct SarifResult<'a> {
    #[serde(rename = "ruleId")]
    rule_id: &'static str,
    level: &'static str,
    message: SarifMessage<'a>,
    locations: Vec<SarifLocation<'a>>,
    #[serde(rename = "relatedLocations", skip_serializing_if = "Vec::is_empty")]
    related_locations: Vec<SarifRelatedLocation<'a>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    fixes: Vec<SarifFix<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    properties: Option<SarifResultProperties<'a>>,
}

#[derive(Serialize)]
struct SarifMessage<'a> {
    text: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SarifLocation<'a> {
    physical_location: SarifPhysicalLocation,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<SarifMessage<'a>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SarifRelatedLocation<'a> {
    id: usize,
    physical_location: SarifPhysicalLocation,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<SarifMessage<'a>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SarifPhysicalLocation {
    artifact_location: SarifArtifactLocation,
    region: SarifRegion,
}

#[derive(Serialize)]
struct SarifArtifactLocation {
    uri: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SarifRegion {
    start_line: usize,
    start_column: usize,
    end_line: usize,
    end_column: usize,
    byte_offset: u32,
    byte_length: u32,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SarifFix<'a> {
    description: SarifMessage<'a>,
    artifact_changes: Vec<SarifArtifactChange<'a>>,
    properties: SarifFixProperties,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SarifArtifactChange<'a> {
    artifact_location: SarifArtifactLocation,
    replacements: Vec<SarifReplacement<'a>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SarifReplacement<'a> {
    deleted_region: SarifRegion,
    inserted_content: SarifInsertedContent<'a>,
}

#[derive(Serialize)]
struct SarifInsertedContent<'a> {
    text: &'a str,
}

#[derive(Serialize)]
struct SarifFixProperties {
    applicability: Applicability,
    provenance: FixProvenance,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SarifResultProperties<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<ReasonCode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    documentation_url: Option<&'a str>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SarifInvocation {
    execution_successful: bool,
}

fn sarif_rules<'a>(diagnostics: &[PreparedDiagnostic<'a>]) -> Vec<SarifRule<'a>> {
    let mut rules = BTreeMap::<DiagCode, Severity>::new();
    for diagnostic in diagnostics {
        rules
            .entry(diagnostic.diagnostic.code)
            .and_modify(|severity| *severity = (*severity).min(diagnostic.diagnostic.severity))
            .or_insert(diagnostic.diagnostic.severity);
    }
    rules
        .into_iter()
        .map(|(code, severity)| SarifRule {
            id: code.as_str(),
            name: code.as_str(),
            short_description: SarifMessage {
                text: rule_description(code),
            },
            help_uri: DOCUMENTATION_URI,
            default_configuration: SarifConfiguration {
                level: sarif_level(severity),
            },
            properties: SarifRuleProperties {
                family: family_name(code.family()),
            },
        })
        .collect()
}

const fn rule_description(code: DiagCode) -> &'static str {
    match code {
        DiagCode::UnterminatedIsland => "unterminated embedded language island",
        DiagCode::BadToken => "unrecognized source token",
        DiagCode::MalformedSyntax => "malformed supported syntax",
        DiagCode::ParenthesizedTuple => "unsupported parenthesized tuple",
        DiagCode::IllegalBracketIndex => "illegal bracket index",
        DiagCode::MalformedMilestoningArguments => "malformed milestoning arguments",
        DiagCode::UnknownJoinKind => "unknown join kind",
        DiagCode::WrongMilestoningArity => "wrong milestoning arity",
        DiagCode::UnknownProperty => "unknown property",
        DiagCode::UnknownSource => "unknown navigation source",
        DiagCode::ModelMergeConflict => "model merge conflict",
        DiagCode::DuplicateModelDeclaration => "duplicate model declaration",
        DiagCode::UnresolvedModelAssociation => "unresolved model association",
    }
}

fn sarif_result<'a>(diagnostic: &PreparedDiagnostic<'a>) -> SarifResult<'a> {
    SarifResult {
        rule_id: diagnostic.diagnostic.code.as_str(),
        level: sarif_level(diagnostic.diagnostic.severity),
        message: SarifMessage {
            text: &diagnostic.diagnostic.message,
        },
        locations: vec![sarif_primary_location(&diagnostic.primary)],
        related_locations: diagnostic
            .secondary
            .iter()
            .enumerate()
            .map(|(index, label)| sarif_related_location(index, label))
            .collect(),
        fixes: diagnostic.fix.iter().map(sarif_fix).collect(),
        properties: result_properties(diagnostic),
    }
}

fn sarif_primary_location<'a>(label: &PreparedLabel<'a>) -> SarifLocation<'a> {
    SarifLocation {
        physical_location: physical_location(label.source, label.span, label.start, label.end),
        message: (!label.note.is_empty()).then_some(SarifMessage { text: label.note }),
    }
}

fn sarif_related_location<'a>(index: usize, label: &PreparedLabel<'a>) -> SarifRelatedLocation<'a> {
    SarifRelatedLocation {
        id: index + 1,
        physical_location: physical_location(label.source, label.span, label.start, label.end),
        message: (!label.note.is_empty()).then_some(SarifMessage { text: label.note }),
    }
}

fn sarif_fix<'a>(fix: &crate::input::PreparedFix<'a>) -> SarifFix<'a> {
    SarifFix {
        description: SarifMessage {
            text: &fix.fix.title,
        },
        artifact_changes: fix.edits.iter().map(sarif_artifact_change).collect(),
        properties: SarifFixProperties {
            applicability: fix.fix.applicability,
            provenance: fix.fix.provenance,
        },
    }
}

fn sarif_artifact_change<'a>(edit: &PreparedEdit<'a>) -> SarifArtifactChange<'a> {
    SarifArtifactChange {
        artifact_location: SarifArtifactLocation {
            uri: artifact_uri(edit.source.name()),
        },
        replacements: vec![SarifReplacement {
            deleted_region: sarif_region(edit.source, edit.edit.span, edit.start, edit.end),
            inserted_content: SarifInsertedContent {
                text: &edit.edit.new_text,
            },
        }],
    }
}

fn result_properties<'a>(diagnostic: &PreparedDiagnostic<'a>) -> Option<SarifResultProperties<'a>> {
    let properties = SarifResultProperties {
        reason: diagnostic.diagnostic.reason,
        documentation_url: diagnostic.diagnostic.url.as_deref(),
    };
    (properties.reason.is_some() || properties.documentation_url.is_some()).then_some(properties)
}

fn physical_location(
    source: &SourceFile,
    span: TextRange,
    start: LineColumn,
    end: LineColumn,
) -> SarifPhysicalLocation {
    SarifPhysicalLocation {
        artifact_location: SarifArtifactLocation {
            uri: artifact_uri(source.name()),
        },
        region: sarif_region(source, span, start, end),
    }
}

fn artifact_uri(name: &str) -> String {
    let normalized = name.replace('\\', "/");
    utf8_percent_encode(&normalized, ARTIFACT_PATH_ENCODE_SET).to_string()
}

fn sarif_region(
    source: &SourceFile,
    span: TextRange,
    start: LineColumn,
    end: LineColumn,
) -> SarifRegion {
    let start_byte = u32::from(span.start());
    let end_byte = u32::from(span.end());
    SarifRegion {
        start_line: start.line,
        start_column: code_point_column(source, usize::from(span.start())),
        end_line: end.line,
        end_column: code_point_column(source, usize::from(span.end())),
        byte_offset: start_byte,
        byte_length: end_byte - start_byte,
    }
}

fn code_point_column(source: &SourceFile, offset: usize) -> usize {
    let text = source.text();
    // `PreparedInput` has already established that `offset` is a UTF-8 boundary.
    let prefix = &text[..offset];
    let line_start = prefix.rfind('\n').map_or(0, |newline| newline + 1);
    text[line_start..offset].chars().count() + 1
}

const fn sarif_level(severity: Severity) -> &'static str {
    match severity {
        Severity::Error => "error",
        Severity::Warning => "warning",
        Severity::Info => "note",
        Severity::Hint => "none",
    }
}

const fn family_name(family: DiagFamily) -> &'static str {
    match family {
        DiagFamily::Lexer => "lexer",
        DiagFamily::Parser => "parser",
        DiagFamily::Lint => "lint",
        DiagFamily::Tool => "tool",
    }
}
