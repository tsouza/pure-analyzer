//! Terminal-oriented rendering for canonical-emission outcomes.

use crate::{
    CanonicalEmissionRenderInput, HumanOptions, RenderError,
    canonical_emission::{PreparedCanonicalEmission, PreparedCanonicalIndecision},
    human::append_terminal_text,
    origin::append_origin,
};

const EMITTED_COLOR: &str = "32";
const INDECISIVE_COLOR: &str = "33";

pub(crate) fn render(
    input: CanonicalEmissionRenderInput<'_>,
    options: HumanOptions,
) -> Result<String, RenderError> {
    let emission = PreparedCanonicalEmission::new(input)?;
    let mut output = String::new();
    match emission {
        PreparedCanonicalEmission::Emitted(text) => {
            append_status(&mut output, "emitted", EMITTED_COLOR, options.color);
            output.push_str("  text: ");
            append_terminal_text(&mut output, text);
            output.push('\n');
        }
        PreparedCanonicalEmission::Indecisive(indecision) => {
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

fn append_indecision(output: &mut String, indecision: &PreparedCanonicalIndecision<'_>) {
    output.push_str("  reason: ");
    output.push_str(indecision.reason_id);
    output.push_str(" — ");
    append_terminal_text(output, indecision.reason_blurb);
    output.push('\n');
    append_origin(output, "origin", &indecision.origin);
}
