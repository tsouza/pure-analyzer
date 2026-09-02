//! Versioned JSON rendering for M4a comparison outcomes.

use libpure::{LineColumn, OutputSchemaField, SourceFile, SourceOrigin, StructuralDifferenceKind};
use serde::Serialize;

use crate::{
    ComparisonRenderInput, RenderError,
    comparison::{PreparedComparison, PreparedDifference, PreparedModelAnchor, PreparedOrigin},
};

const JSON_SCHEMA_VERSION: &str = "1.0";

pub(crate) fn render(input: ComparisonRenderInput<'_>) -> Result<String, RenderError> {
    let comparison = PreparedComparison::new(input)?;
    let envelope = json_envelope(&comparison);
    let mut output =
        serde_json::to_string_pretty(&envelope).map_err(|source| RenderError::Serialization {
            format: "comparison JSON",
            source,
        })?;
    output.push('\n');
    Ok(output)
}

#[derive(Serialize)]
struct JsonEnvelope<'a> {
    version: &'static str,
    #[serde(flatten)]
    result: JsonResult<'a>,
}

#[derive(Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
enum JsonResult<'a> {
    Equivalent,
    NotEquivalent {
        difference: JsonDifference<'a>,
    },
    Indecisive {
        reason: JsonReason,
        origin: JsonOrigin<'a>,
    },
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum JsonDifference<'a> {
    OutputColumnCount {
        primary_count: usize,
        secondary_count: usize,
        primary_origin: JsonOrigin<'a>,
        secondary_origin: JsonOrigin<'a>,
    },
    OutputColumn {
        index: usize,
        field: &'static str,
        primary_origin: JsonOrigin<'a>,
        secondary_origin: JsonOrigin<'a>,
    },
}

#[derive(Serialize)]
struct JsonReason {
    id: &'static str,
    blurb: &'static str,
}

#[derive(Serialize)]
struct JsonOrigin<'a> {
    source: JsonAnchor<'a>,
    model_origins: Vec<JsonModelAnchor<'a>>,
}

#[derive(Serialize)]
struct JsonAnchor<'a> {
    file: JsonFile<'a>,
    range: JsonRange,
}

#[derive(Serialize)]
struct JsonModelAnchor<'a> {
    file: JsonFile<'a>,
    #[serde(skip_serializing_if = "Option::is_none")]
    range: Option<JsonRange>,
}

#[derive(Serialize)]
struct JsonFile<'a> {
    id: u32,
    name: &'a str,
    origin: &'static str,
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

fn json_envelope<'a>(comparison: &'a PreparedComparison<'a>) -> JsonEnvelope<'a> {
    let result = match comparison {
        PreparedComparison::Equivalent => JsonResult::Equivalent,
        PreparedComparison::NotEquivalent(difference) => JsonResult::NotEquivalent {
            difference: json_difference(difference),
        },
        PreparedComparison::Indecisive(indecision) => JsonResult::Indecisive {
            reason: JsonReason {
                id: indecision.reason_id,
                blurb: indecision.reason_blurb,
            },
            origin: json_origin(&indecision.origin),
        },
    };
    JsonEnvelope {
        version: JSON_SCHEMA_VERSION,
        result,
    }
}

fn json_difference<'a>(difference: &'a PreparedDifference<'a>) -> JsonDifference<'a> {
    let primary_origin = json_origin(&difference.primary_origin);
    let secondary_origin = json_origin(&difference.secondary_origin);
    match difference.kind {
        StructuralDifferenceKind::OutputColumnCount {
            primary_count,
            secondary_count,
        } => JsonDifference::OutputColumnCount {
            primary_count: *primary_count,
            secondary_count: *secondary_count,
            primary_origin,
            secondary_origin,
        },
        StructuralDifferenceKind::OutputColumn { index, field } => JsonDifference::OutputColumn {
            index: *index,
            field: output_schema_field_name(*field),
            primary_origin,
            secondary_origin,
        },
    }
}

fn json_origin<'a>(origin: &'a PreparedOrigin<'a>) -> JsonOrigin<'a> {
    JsonOrigin {
        source: JsonAnchor {
            file: json_file(origin.source),
            range: json_range(origin.span, origin.start, origin.end),
        },
        model_origins: origin.model_origins.iter().map(json_model_anchor).collect(),
    }
}

fn json_model_anchor<'a>(anchor: &'a PreparedModelAnchor<'a>) -> JsonModelAnchor<'a> {
    match anchor {
        PreparedModelAnchor::Document { source } => JsonModelAnchor {
            file: json_file(source),
            range: None,
        },
        PreparedModelAnchor::Span {
            source,
            span,
            start,
            end,
        } => JsonModelAnchor {
            file: json_file(source),
            range: Some(json_range(*span, *start, *end)),
        },
    }
}

fn json_file(source: &SourceFile) -> JsonFile<'_> {
    JsonFile {
        id: source.id().index(),
        name: source.name(),
        origin: source_origin_name(source.origin()),
    }
}

fn json_range(range: libpure::TextRange, start: LineColumn, end: LineColumn) -> JsonRange {
    JsonRange {
        start: json_position(range.start(), start),
        end: json_position(range.end(), end),
    }
}

fn json_position(byte: libpure::TextSize, location: LineColumn) -> JsonPosition {
    JsonPosition {
        byte: u32::from(byte),
        line: location.line,
        column: location.column,
    }
}

const fn source_origin_name(origin: &SourceOrigin) -> &'static str {
    match origin {
        SourceOrigin::File { .. } => "file",
        SourceOrigin::InMemory => "memory",
        SourceOrigin::Stdin => "stdin",
    }
}

const fn output_schema_field_name(field: OutputSchemaField) -> &'static str {
    match field {
        OutputSchemaField::Name => "name",
        OutputSchemaField::Type => "type",
        OutputSchemaField::Multiplicity => "multiplicity",
        OutputSchemaField::Nullability => "nullability",
    }
}
