//! Shared validation of source-backed relational origins.

use libpure::{IrOrigin, LineColumn, SourceFile, SourceStore};
use pure_analyzer_diagnostics::{FileId, TextRange};
use serde::Serialize;

use crate::{
    RenderError,
    error::OriginRole,
    human::append_terminal_text,
    json_envelope::{JsonFile, JsonRange, json_file, json_range},
};

/// One validated query origin and its contributing model anchors.
pub(crate) struct PreparedOrigin<'a> {
    pub(crate) source: &'a SourceFile,
    pub(crate) span: TextRange,
    pub(crate) start: LineColumn,
    pub(crate) end: LineColumn,
    pub(crate) model_origins: Vec<PreparedModelAnchor<'a>>,
}

/// One validated model-definition anchor contributing to an IR origin.
pub(crate) enum PreparedModelAnchor<'a> {
    /// A source-backed model definition without a precise declaration span.
    Document {
        /// Retained model source.
        source: &'a SourceFile,
    },
    /// A source-backed model definition with a precise declaration span.
    Span {
        /// Retained model source.
        source: &'a SourceFile,
        /// Exact definition span.
        span: TextRange,
        /// One-based location at the beginning of `span`.
        start: LineColumn,
        /// One-based location at the end of `span`.
        end: LineColumn,
    },
}

/// Validate an IR origin against the immutable source snapshot used to create it.
pub(crate) fn prepare_origin<'a, R>(
    sources: &'a SourceStore,
    role: R,
    origin: &IrOrigin,
) -> Result<PreparedOrigin<'a>, RenderError>
where
    R: OriginRole,
{
    let source_span = origin.source();
    let (source, start, end) = validate_span(
        sources,
        role.clone(),
        source_span.file(),
        source_span.range(),
    )?;
    let model_origins = origin
        .model_origins()
        .iter()
        .enumerate()
        .map(|(index, model_origin)| {
            let definition = model_origin.definition();
            prepare_model_anchor(
                sources,
                role.clone().model(index),
                FileId::new(definition.source().index()),
                definition.span(),
            )
        })
        .collect::<Result<Vec<_>, _>>()?;

    Ok(PreparedOrigin {
        source,
        span: source_span.range(),
        start,
        end,
        model_origins,
    })
}

fn prepare_model_anchor<'a, R>(
    sources: &'a SourceStore,
    role: R,
    file: FileId,
    span: Option<TextRange>,
) -> Result<PreparedModelAnchor<'a>, RenderError>
where
    R: OriginRole,
{
    match span {
        Some(span) => {
            let (source, start, end) = validate_span(sources, role, file, span)?;
            Ok(PreparedModelAnchor::Span {
                source,
                span,
                start,
                end,
            })
        }
        None => {
            let source = sources
                .get(file)
                .ok_or_else(|| R::unknown_file(role, file))?;
            Ok(PreparedModelAnchor::Document { source })
        }
    }
}

fn validate_span<R>(
    sources: &SourceStore,
    role: R,
    file: FileId,
    span: TextRange,
) -> Result<(&SourceFile, LineColumn, LineColumn), RenderError>
where
    R: OriginRole,
{
    let source = sources
        .get(file)
        .ok_or_else(|| R::unknown_file(role.clone(), file))?;
    let start = span.start();
    let end = span.end();
    let invalid = || R::invalid_span(role.clone(), file, u32::from(start), u32::from(end));
    if start > end {
        return Err(invalid());
    }
    let start_location = source.line_column(start).ok_or_else(invalid)?;
    let end_location = source.line_column(end).ok_or_else(invalid)?;
    Ok((source, start_location, end_location))
}

/// Append one stable terminal-oriented origin block.
pub(crate) fn append_origin(output: &mut String, name: &str, origin: &PreparedOrigin<'_>) {
    output.push_str("  ");
    output.push_str(name);
    output.push_str(":\n");
    append_location(
        output,
        "    source",
        origin.source,
        &origin.start,
        &origin.end,
    );
    if origin.model_origins.is_empty() {
        return;
    }
    output.push_str("    model_origins:\n");
    for anchor in &origin.model_origins {
        match anchor {
            PreparedModelAnchor::Document { source } => {
                output.push_str("      - ");
                append_terminal_text(output, source.name());
                output.push_str(" (document)\n");
            }
            PreparedModelAnchor::Span {
                source, start, end, ..
            } => append_location(output, "      -", source, start, end),
        }
    }
}

fn append_location(
    output: &mut String,
    prefix: &str,
    source: &SourceFile,
    start: &LineColumn,
    end: &LineColumn,
) {
    output.push_str(prefix);
    output.push_str(": ");
    append_terminal_text(output, source.name());
    output.push(':');
    output.push_str(&start.line.to_string());
    output.push(':');
    output.push_str(&start.column.to_string());
    output.push_str("..");
    output.push_str(&end.line.to_string());
    output.push(':');
    output.push_str(&end.column.to_string());
    output.push('\n');
}

/// A versioned JSON representation of a validated IR origin.
#[derive(Serialize)]
pub(crate) struct JsonOrigin<'a> {
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

/// Convert one validated origin to the common versioned JSON shape.
pub(crate) fn json_origin<'a>(origin: &PreparedOrigin<'a>) -> JsonOrigin<'a> {
    JsonOrigin {
        source: JsonAnchor {
            file: json_file(origin.source),
            range: json_range(origin.span, origin.start, origin.end),
        },
        model_origins: origin.model_origins.iter().map(json_model_anchor).collect(),
    }
}

fn json_model_anchor<'a>(anchor: &PreparedModelAnchor<'a>) -> JsonModelAnchor<'a> {
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

#[cfg(test)]
mod tests {
    use libpure::{SourceInput, SourceStore};

    use super::validate_span;
    use crate::error::ComparisonOriginRole;
    use pure_analyzer_diagnostics::{FileId, TextRange};

    fn sources() -> SourceStore {
        SourceStore::load([SourceInput::in_memory("origin.pure", "abcdef")]).expect("source loads")
    }

    #[test]
    fn validate_span_accepts_a_zero_width_span() {
        // `text_size::TextRange`'s own constructor already rejects a
        // reversed (`start > end`) span, so `start > end` here can never
        // observe `true` for any span this codebase can construct — the
        // reachable boundary is `start == end`, a perfectly valid
        // zero-width span (an insertion point). A mutant weakening `>` to
        // `==` or `>=` turns every such span into a spurious error.
        let sources = sources();
        let span = TextRange::new(3.into(), 3.into());

        let result = validate_span(
            &sources,
            ComparisonOriginRole::Indecision,
            FileId::new(0),
            span,
        );

        assert!(
            result.is_ok(),
            "a zero-width span must validate, got: {result:?}"
        );
    }
}
