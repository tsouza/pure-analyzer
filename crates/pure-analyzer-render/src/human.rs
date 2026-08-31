//! Deterministic terminal-oriented diagnostic rendering.

use pure_analyzer_diagnostics::{Applicability, FixProvenance, Severity, Verdict};

use crate::{
    HumanOptions, RenderError, RenderInput,
    input::{PreparedDiagnostic, PreparedEdit, PreparedInput, PreparedLabel, Summary},
};

pub(crate) fn render(input: RenderInput<'_>, options: HumanOptions) -> Result<String, RenderError> {
    let prepared = PreparedInput::new(input)?;
    let mut output = String::new();
    let mut previous_file = None;

    for diagnostic in &prepared.diagnostics {
        let file = diagnostic.primary.source.id();
        if previous_file != Some(file) {
            if previous_file.is_some() {
                output.push('\n');
            }
            append_terminal_text(&mut output, diagnostic.primary.source.name());
            output.push_str(":\n");
            previous_file = Some(file);
        }
        append_diagnostic(&mut output, diagnostic, options.color);
    }

    if previous_file.is_some() {
        output.push('\n');
    }
    append_summary(&mut output, prepared.summary);
    Ok(output)
}

fn append_diagnostic(output: &mut String, diagnostic: &PreparedDiagnostic<'_>, color: bool) {
    let severity = diagnostic.diagnostic.severity;
    if color {
        output.push_str("\x1b[1;");
        output.push_str(severity_color(severity));
        output.push('m');
    }
    output.push_str("  ");
    output.push_str(severity_name(severity));
    output.push('[');
    output.push_str(diagnostic.diagnostic.code.as_str());
    output.push_str("]: ");
    append_terminal_text(output, &diagnostic.diagnostic.message);
    if color {
        output.push_str("\x1b[0m");
    }
    output.push('\n');

    append_label(output, "primary", &diagnostic.primary);
    for label in &diagnostic.secondary {
        append_label(output, "secondary", label);
    }
    if let Some(fix) = &diagnostic.fix {
        output.push_str("    = fix: ");
        append_terminal_text(output, &fix.fix.title);
        output.push_str(" [");
        output.push_str(applicability_name(fix.fix.applicability));
        output.push_str(", ");
        output.push_str(provenance_name(fix.fix.provenance));
        output.push_str("]\n");
        for edit in &fix.edits {
            append_edit(output, edit);
        }
    }
    if let Some(verdict) = &diagnostic.diagnostic.verdict {
        output.push_str("    = verdict: ");
        append_verdict_name(output, verdict);
        output.push('\n');
    }
    if let Some(reason) = diagnostic.diagnostic.reason {
        output.push_str("    = reason: ");
        output.push_str(reason.id());
        output.push_str(" — ");
        output.push_str(reason.blurb());
        output.push('\n');
    }
    if let Some(url) = &diagnostic.diagnostic.url {
        output.push_str("    = docs: ");
        append_terminal_text(output, url);
        output.push('\n');
    }
}

fn append_label(output: &mut String, role: &str, label: &PreparedLabel<'_>) {
    output.push_str("    --> ");
    append_terminal_text(output, label.source.name());
    output.push(':');
    output.push_str(&label.start.line.to_string());
    output.push(':');
    output.push_str(&label.start.column.to_string());
    output.push_str("..");
    output.push_str(&label.end.line.to_string());
    output.push(':');
    output.push_str(&label.end.column.to_string());
    output.push_str(" (");
    output.push_str(role);
    output.push_str(")\n");

    let (line, padding, width) = annotated_line(label);
    output.push_str("      |");
    output.push('\n');
    output.push_str(&format!("{:>5} | ", label.start.line));
    append_terminal_text(output, line);
    output.push('\n');
    output.push_str("      | ");
    output.push_str(&" ".repeat(padding));
    output.push_str(&"^".repeat(width));
    output.push(' ');
    output.push_str(role);
    if !label.note.is_empty() {
        output.push_str(": ");
        append_terminal_text(output, label.note);
    }
    output.push('\n');
}

fn annotated_line<'a>(label: &PreparedLabel<'a>) -> (&'a str, usize, usize) {
    let text = label.source.text();
    let start = usize::from(label.span.start());
    let end = usize::from(label.span.end());
    let line_start = text[..start].rfind('\n').map_or(0, |index| index + 1);
    let line_end = text[start..]
        .find('\n')
        .map_or(text.len(), |index| start + index);
    let highlighted_end = end.min(line_end);
    let padding = terminal_text_width(&text[line_start..start]);
    let width = terminal_text_width(&text[start..highlighted_end]).max(1);
    (&text[line_start..line_end], padding, width)
}

fn append_edit(output: &mut String, edit: &PreparedEdit<'_>) {
    output.push_str("      replace ");
    append_terminal_text(output, edit.source.name());
    output.push(':');
    output.push_str(&edit.start.line.to_string());
    output.push(':');
    output.push_str(&edit.start.column.to_string());
    output.push_str(".. ");
    output.push_str(&edit.end.line.to_string());
    output.push(':');
    output.push_str(&edit.end.column.to_string());
    output.push_str(" with ");
    append_quoted_terminal_text(output, &edit.edit.new_text);
    output.push('\n');
}

fn append_summary(output: &mut String, summary: Summary) {
    output.push_str("summary: ");
    output.push_str(&summary.errors.to_string());
    output.push_str(" errors, ");
    output.push_str(&summary.warnings.to_string());
    output.push_str(" warnings, ");
    output.push_str(&summary.infos.to_string());
    output.push_str(" info, ");
    output.push_str(&summary.hints.to_string());
    output.push_str(" hints (");
    output.push_str(&summary.total.to_string());
    output.push_str(" total)\n");
}

const fn severity_name(severity: Severity) -> &'static str {
    match severity {
        Severity::Error => "error",
        Severity::Warning => "warning",
        Severity::Info => "info",
        Severity::Hint => "hint",
    }
}

const fn severity_color(severity: Severity) -> &'static str {
    match severity {
        Severity::Error => "31",
        Severity::Warning => "33",
        Severity::Info => "34",
        Severity::Hint => "36",
    }
}

const fn applicability_name(applicability: Applicability) -> &'static str {
    match applicability {
        Applicability::MachineApplicable => "machine_applicable",
        Applicability::Suggested => "suggested",
        Applicability::Unsafe => "unsafe",
    }
}

const fn provenance_name(provenance: FixProvenance) -> &'static str {
    match provenance {
        FixProvenance::SyntaxOnly => "syntax_only",
        FixProvenance::ModelDependent => "model_dependent",
        FixProvenance::SingleArityProven => "single_arity_proven",
    }
}

fn append_verdict_name(output: &mut String, verdict: &Verdict) {
    match verdict {
        Verdict::Equivalent => output.push_str("equivalent"),
        Verdict::NotEquivalent { witness } => {
            output.push_str("not_equivalent; witness: ");
            append_terminal_text(output, witness);
        }
        Verdict::Indecisive => output.push_str("indecisive"),
    }
}

fn append_quoted_terminal_text(output: &mut String, text: &str) {
    output.push('"');
    for character in text.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            _ => append_terminal_character(output, character),
        }
    }
    output.push('"');
}

fn append_terminal_text(output: &mut String, text: &str) {
    for character in text.chars() {
        append_terminal_character(output, character);
    }
}

fn append_terminal_character(output: &mut String, character: char) {
    match character {
        '\0' => output.push_str("\\0"),
        '\t' => output.push_str("\\t"),
        '\n' => output.push_str("\\n"),
        '\r' => output.push_str("\\r"),
        character if character.is_control() => output.extend(character.escape_unicode()),
        _ => output.push(character),
    }
}

fn terminal_text_width(text: &str) -> usize {
    text.chars().map(terminal_character_width).sum()
}

fn terminal_character_width(character: char) -> usize {
    match character {
        '\0' | '\t' | '\n' | '\r' => 2,
        character if character.is_control() => character.escape_unicode().count(),
        _ => 1,
    }
}
