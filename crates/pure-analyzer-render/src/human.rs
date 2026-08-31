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
            output.push_str(diagnostic.primary.source.name());
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
    let header = format!(
        "  {}[{}]: {}",
        severity_name(severity),
        diagnostic.diagnostic.code.as_str(),
        diagnostic.diagnostic.message
    );
    if color {
        output.push_str("\x1b[1;");
        output.push_str(severity_color(severity));
        output.push('m');
        output.push_str(&header);
        output.push_str("\x1b[0m\n");
    } else {
        output.push_str(&header);
        output.push('\n');
    }

    append_label(output, "primary", &diagnostic.primary);
    for label in &diagnostic.secondary {
        append_label(output, "secondary", label);
    }
    if let Some(fix) = &diagnostic.fix {
        output.push_str("    = fix: ");
        output.push_str(&fix.fix.title);
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
        output.push_str(&verdict_name(verdict));
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
        output.push_str(url);
        output.push('\n');
    }
}

fn append_label(output: &mut String, role: &str, label: &PreparedLabel<'_>) {
    output.push_str("    --> ");
    output.push_str(label.source.name());
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
    output.push_str(line);
    output.push('\n');
    output.push_str("      | ");
    output.push_str(&" ".repeat(padding));
    output.push_str(&"^".repeat(width));
    output.push(' ');
    output.push_str(role);
    if !label.note.is_empty() {
        output.push_str(": ");
        output.push_str(label.note);
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
    let padding = text[line_start..start].chars().count();
    let width = text[start..highlighted_end].chars().count().max(1);
    (&text[line_start..line_end], padding, width)
}

fn append_edit(output: &mut String, edit: &PreparedEdit<'_>) {
    output.push_str("      replace ");
    output.push_str(edit.source.name());
    output.push(':');
    output.push_str(&edit.start.line.to_string());
    output.push(':');
    output.push_str(&edit.start.column.to_string());
    output.push_str(".. ");
    output.push_str(&edit.end.line.to_string());
    output.push(':');
    output.push_str(&edit.end.column.to_string());
    output.push_str(" with ");
    output.push_str(&format!("{:?}", edit.edit.new_text));
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

fn verdict_name(verdict: &Verdict) -> String {
    match verdict {
        Verdict::Equivalent => "equivalent".to_owned(),
        Verdict::NotEquivalent { witness } => format!("not_equivalent; witness: {witness}"),
        Verdict::Indecisive => "indecisive".to_owned(),
    }
}
