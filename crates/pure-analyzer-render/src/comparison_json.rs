//! Versioned JSON rendering for M4a comparison outcomes.

use libpure::{OutputSchemaField, StructuralDifferenceKind};
use serde::Serialize;

use crate::{
    ComparisonRenderInput, RenderError,
    comparison::{PreparedComparison, PreparedDifference},
    json_envelope::{JSON_SCHEMA_VERSION, JsonReason},
    origin::{JsonOrigin, json_origin},
};

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

const fn output_schema_field_name(field: OutputSchemaField) -> &'static str {
    match field {
        OutputSchemaField::Name => "name",
        OutputSchemaField::Type => "type",
        OutputSchemaField::Multiplicity => "multiplicity",
        OutputSchemaField::Nullability => "nullability",
    }
}
