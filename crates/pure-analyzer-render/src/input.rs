//! Validation and canonical ordering shared by every renderer.

use std::cmp::Ordering;

use libpure::{LineColumn, SourceFile, SourceStore};
use pure_analyzer_diagnostics::{
    Applicability, Diagnostic, FileId, Fix, FixProvenance, Label, Severity, TextEdit, TextRange,
    Verdict,
};

use crate::{RenderError, RenderInput, SpanKind};

/// Validated, canonically ordered input used by output implementations.
pub(crate) struct PreparedInput<'a> {
    pub(crate) files: Vec<&'a SourceFile>,
    pub(crate) diagnostics: Vec<PreparedDiagnostic<'a>>,
    pub(crate) summary: Summary,
}

/// A finding paired with locations known to be valid for its source snapshot.
pub(crate) struct PreparedDiagnostic<'a> {
    pub(crate) diagnostic: &'a Diagnostic,
    pub(crate) primary: PreparedLabel<'a>,
    pub(crate) secondary: Vec<PreparedLabel<'a>>,
    pub(crate) fix: Option<PreparedFix<'a>>,
}

/// A validated diagnostic label.
pub(crate) struct PreparedLabel<'a> {
    pub(crate) source: &'a SourceFile,
    pub(crate) span: TextRange,
    pub(crate) start: LineColumn,
    pub(crate) end: LineColumn,
    pub(crate) note: &'a str,
}

/// A validated structured fix.
pub(crate) struct PreparedFix<'a> {
    pub(crate) fix: &'a Fix,
    pub(crate) edits: Vec<PreparedEdit<'a>>,
}

/// A validated replacement edit.
pub(crate) struct PreparedEdit<'a> {
    pub(crate) edit: &'a TextEdit,
    pub(crate) source: &'a SourceFile,
    pub(crate) start: LineColumn,
    pub(crate) end: LineColumn,
}

/// Counts grouped by diagnostic severity.
#[derive(Clone, Copy, Default)]
pub(crate) struct Summary {
    pub(crate) errors: usize,
    pub(crate) warnings: usize,
    pub(crate) infos: usize,
    pub(crate) hints: usize,
    pub(crate) total: usize,
}

impl Summary {
    fn include(&mut self, severity: Severity) {
        self.total += 1;
        match severity {
            Severity::Error => self.errors += 1,
            Severity::Warning => self.warnings += 1,
            Severity::Info => self.infos += 1,
            Severity::Hint => self.hints += 1,
        }
    }
}

impl<'a> PreparedInput<'a> {
    pub(crate) fn new(input: RenderInput<'a>) -> Result<Self, RenderError> {
        let mut diagnostics = input
            .diagnostics
            .iter()
            .enumerate()
            .map(|(index, diagnostic)| prepare_diagnostic(input.sources, index, diagnostic))
            .collect::<Result<Vec<_>, _>>()?;
        diagnostics.sort_by(compare_diagnostics);

        let mut files = input.sources.files().collect::<Vec<_>>();
        files.sort_by_key(|source| source.id());

        let mut summary = Summary::default();
        for diagnostic in &diagnostics {
            summary.include(diagnostic.diagnostic.severity);
        }

        Ok(Self {
            files,
            diagnostics,
            summary,
        })
    }
}

fn prepare_diagnostic<'a>(
    sources: &'a SourceStore,
    diagnostic_index: usize,
    diagnostic: &'a Diagnostic,
) -> Result<PreparedDiagnostic<'a>, RenderError> {
    let primary = prepare_label(
        sources,
        diagnostic_index,
        SpanKind::Primary,
        &diagnostic.primary,
    )?;
    let secondary = diagnostic
        .secondary
        .iter()
        .enumerate()
        .map(|(index, label)| {
            prepare_label(sources, diagnostic_index, SpanKind::Secondary(index), label)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let fix = diagnostic
        .fix
        .as_ref()
        .map(|fix| prepare_fix(sources, diagnostic_index, fix))
        .transpose()?;

    Ok(PreparedDiagnostic {
        diagnostic,
        primary,
        secondary,
        fix,
    })
}

fn prepare_label<'a>(
    sources: &'a SourceStore,
    diagnostic_index: usize,
    kind: SpanKind,
    label: &'a Label,
) -> Result<PreparedLabel<'a>, RenderError> {
    let (source, start, end) =
        validate_span(sources, diagnostic_index, kind, label.file, label.span)?;
    Ok(PreparedLabel {
        source,
        span: label.span,
        start,
        end,
        note: &label.note,
    })
}

fn prepare_fix<'a>(
    sources: &'a SourceStore,
    diagnostic_index: usize,
    fix: &'a Fix,
) -> Result<PreparedFix<'a>, RenderError> {
    let edits = fix
        .edits
        .iter()
        .enumerate()
        .map(|(index, edit)| prepare_edit(sources, diagnostic_index, index, edit))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(PreparedFix { fix, edits })
}

fn prepare_edit<'a>(
    sources: &'a SourceStore,
    diagnostic_index: usize,
    index: usize,
    edit: &'a TextEdit,
) -> Result<PreparedEdit<'a>, RenderError> {
    let (source, start, end) = validate_span(
        sources,
        diagnostic_index,
        SpanKind::FixEdit(index),
        edit.file,
        edit.span,
    )?;
    Ok(PreparedEdit {
        edit,
        source,
        start,
        end,
    })
}

fn validate_span(
    sources: &SourceStore,
    diagnostic_index: usize,
    kind: SpanKind,
    file: FileId,
    span: TextRange,
) -> Result<(&SourceFile, LineColumn, LineColumn), RenderError> {
    let source = sources.get(file).ok_or_else(|| RenderError::UnknownFile {
        diagnostic_index,
        kind: kind.clone(),
        file,
    })?;
    let start = span.start();
    let end = span.end();
    let invalid = || RenderError::InvalidSpan {
        diagnostic_index,
        kind: kind.clone(),
        file,
        start: u32::from(start),
        end: u32::from(end),
    };
    if start > end {
        return Err(invalid());
    }
    let start_location = source.line_column(start).ok_or_else(invalid)?;
    let end_location = source.line_column(end).ok_or_else(invalid)?;
    Ok((source, start_location, end_location))
}

fn compare_diagnostics(left: &PreparedDiagnostic<'_>, right: &PreparedDiagnostic<'_>) -> Ordering {
    compare_diagnostic(left.diagnostic, right.diagnostic)
}

fn compare_diagnostic(left: &Diagnostic, right: &Diagnostic) -> Ordering {
    compare_label(&left.primary, &right.primary)
        .then_with(|| left.code.cmp(&right.code))
        .then_with(|| left.severity.cmp(&right.severity))
        .then_with(|| left.message.cmp(&right.message))
        .then_with(|| compare_labels(&left.secondary, &right.secondary))
        .then_with(|| compare_fixes(left.fix.as_ref(), right.fix.as_ref()))
        .then_with(|| compare_verdicts(left.verdict.as_ref(), right.verdict.as_ref()))
        .then_with(|| left.reason.cmp(&right.reason))
        .then_with(|| left.url.cmp(&right.url))
}

fn compare_labels(left: &[Label], right: &[Label]) -> Ordering {
    left.iter()
        .zip(right)
        .map(|(left, right)| compare_label(left, right))
        .find(|order| *order != Ordering::Equal)
        .unwrap_or_else(|| left.len().cmp(&right.len()))
}

fn compare_label(left: &Label, right: &Label) -> Ordering {
    left.file
        .cmp(&right.file)
        .then_with(|| compare_ranges(left.span, right.span))
        .then_with(|| left.note.cmp(&right.note))
}

fn compare_ranges(left: TextRange, right: TextRange) -> Ordering {
    left.start()
        .cmp(&right.start())
        .then_with(|| left.end().cmp(&right.end()))
}

fn compare_fixes(left: Option<&Fix>, right: Option<&Fix>) -> Ordering {
    match (left, right) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Less,
        (Some(_), None) => Ordering::Greater,
        (Some(left), Some(right)) => left
            .title
            .cmp(&right.title)
            .then_with(|| {
                applicability_rank(left.applicability).cmp(&applicability_rank(right.applicability))
            })
            .then_with(|| provenance_rank(left.provenance).cmp(&provenance_rank(right.provenance)))
            .then_with(|| compare_edits(&left.edits, &right.edits)),
    }
}

const fn applicability_rank(applicability: Applicability) -> u8 {
    match applicability {
        Applicability::MachineApplicable => 0,
        Applicability::Suggested => 1,
        Applicability::Unsafe => 2,
    }
}

const fn provenance_rank(provenance: FixProvenance) -> u8 {
    match provenance {
        FixProvenance::SyntaxOnly => 0,
        FixProvenance::ModelDependent => 1,
        FixProvenance::SingleArityProven => 2,
    }
}

fn compare_edits(left: &[TextEdit], right: &[TextEdit]) -> Ordering {
    left.iter()
        .zip(right)
        .map(|(left, right)| {
            left.file
                .cmp(&right.file)
                .then_with(|| compare_ranges(left.span, right.span))
                .then_with(|| left.new_text.cmp(&right.new_text))
        })
        .find(|order| *order != Ordering::Equal)
        .unwrap_or_else(|| left.len().cmp(&right.len()))
}

fn compare_verdicts(left: Option<&Verdict>, right: Option<&Verdict>) -> Ordering {
    match (left, right) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Less,
        (Some(_), None) => Ordering::Greater,
        (Some(left), Some(right)) => compare_verdict(left, right),
    }
}

fn compare_verdict(left: &Verdict, right: &Verdict) -> Ordering {
    match (left, right) {
        (Verdict::Equivalent, Verdict::Equivalent) | (Verdict::Indecisive, Verdict::Indecisive) => {
            Ordering::Equal
        }
        (Verdict::Equivalent, _) => Ordering::Less,
        (_, Verdict::Equivalent) => Ordering::Greater,
        (Verdict::NotEquivalent { witness: left }, Verdict::NotEquivalent { witness: right }) => {
            left.cmp(right)
        }
        (Verdict::NotEquivalent { .. }, Verdict::Indecisive) => Ordering::Less,
        (Verdict::Indecisive, Verdict::NotEquivalent { .. }) => Ordering::Greater,
    }
}
