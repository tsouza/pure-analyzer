//! Versioned JSON rendering for canonical-emission outcomes.

use serde::Serialize;

use crate::{
    CanonicalEmissionRenderInput, RenderError,
    canonical_emission::{PreparedCanonicalEmission, PreparedCanonicalIndecision},
    origin::{JsonOrigin, json_origin},
};

const JSON_SCHEMA_VERSION: &str = "1.0";

pub(crate) fn render(input: CanonicalEmissionRenderInput<'_>) -> Result<String, RenderError> {
    let emission = PreparedCanonicalEmission::new(input)?;
    let envelope = json_envelope(&emission);
    let mut output =
        serde_json::to_string_pretty(&envelope).map_err(|source| RenderError::Serialization {
            format: "canonical-emission JSON",
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
    Emitted {
        text: &'a str,
    },
    Indecisive {
        reason: JsonReason,
        origin: JsonOrigin<'a>,
    },
}

#[derive(Serialize)]
struct JsonReason {
    id: &'static str,
    blurb: &'static str,
}

fn json_envelope<'a>(emission: &'a PreparedCanonicalEmission<'a>) -> JsonEnvelope<'a> {
    let result = match emission {
        PreparedCanonicalEmission::Emitted(text) => JsonResult::Emitted { text: *text },
        PreparedCanonicalEmission::Indecisive(indecision) => JsonResult::Indecisive {
            reason: JsonReason {
                id: indecision.reason_id,
                blurb: indecision.reason_blurb,
            },
            origin: json_indecision_origin(indecision),
        },
    };
    JsonEnvelope {
        version: JSON_SCHEMA_VERSION,
        result,
    }
}

fn json_indecision_origin<'a>(indecision: &PreparedCanonicalIndecision<'a>) -> JsonOrigin<'a> {
    json_origin(&indecision.origin)
}
