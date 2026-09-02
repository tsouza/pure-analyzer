//! Terminal-oriented rendering for M4a comparison outcomes.

use libpure::{LineColumn, OutputSchemaField, SourceFile, StructuralDifferenceKind};

use crate::{
    ComparisonRenderInput, HumanOptions, RenderError,
    comparison::{
        PreparedComparison, PreparedDifference, PreparedIndecision, PreparedModelAnchor,
        PreparedOrigin,
    },
    human::append_terminal_text,
};

const EQUIVALENT_COLOR: &str = "32";
const INDECISIVE_COLOR: &str = "33";
const NOT_EQUIVALENT_COLOR: &str = "31";

pub(crate) fn render(
    input: ComparisonRenderInput<'_>,
    options: HumanOptions,
) -> Result<String, RenderError> {
    let comparison = PreparedComparison::new(input)?;
    let mut output = String::new();
    match comparison {
        PreparedComparison::Equivalent => {
            append_status(&mut output, "equivalent", EQUIVALENT_COLOR, options.color);
        }
        PreparedComparison::NotEquivalent(difference) => {
            append_status(
                &mut output,
                "not_equivalent",
                NOT_EQUIVALENT_COLOR,
                options.color,
            );
            append_difference(&mut output, &difference);
        }
        PreparedComparison::Indecisive(indecision) => {
            append_status(&mut output, "indecisive", INDECISIVE_COLOR, options.color);
            append_indecision(&mut output, &indecision);
        }
    }
    Ok(output)
}

fn append_status(output: &mut String, status: &str, color_code: &str, color: bool) {
    if color {
        output.push_str("\x1b[1;");
        output.push_str(color_code);
        output.push('m');
    }
    output.push_str(status);
    if color {
        output.push_str("\x1b[0m");
    }
    output.push('\n');
}

fn append_difference(output: &mut String, difference: &PreparedDifference<'_>) {
    match difference.kind {
        StructuralDifferenceKind::OutputColumnCount {
            primary_count,
            secondary_count,
        } => {
            output.push_str("  difference: output_column_count\n");
            append_usize(output, "  primary_count", *primary_count);
            append_usize(output, "  secondary_count", *secondary_count);
        }
        StructuralDifferenceKind::OutputColumn { index, field } => {
            output.push_str("  difference: output_column\n");
            append_usize(output, "  index", *index);
            output.push_str("  field: ");
            output.push_str(output_schema_field_name(*field));
            output.push('\n');
        }
    }
    append_origin(output, "primary_origin", &difference.primary_origin);
    append_origin(output, "secondary_origin", &difference.secondary_origin);
}

fn append_indecision(output: &mut String, indecision: &PreparedIndecision<'_>) {
    output.push_str("  reason: ");
    output.push_str(indecision.reason_id);
    output.push_str(" — ");
    append_terminal_text(output, indecision.reason_blurb);
    output.push('\n');
    append_origin(output, "origin", &indecision.origin);
}

fn append_usize(output: &mut String, name: &str, value: usize) {
    output.push_str(name);
    output.push_str(": ");
    output.push_str(&value.to_string());
    output.push('\n');
}

fn append_origin(output: &mut String, name: &str, origin: &PreparedOrigin<'_>) {
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

const fn output_schema_field_name(field: OutputSchemaField) -> &'static str {
    match field {
        OutputSchemaField::Name => "name",
        OutputSchemaField::Type => "type",
        OutputSchemaField::Multiplicity => "multiplicity",
        OutputSchemaField::Nullability => "nullability",
    }
}
