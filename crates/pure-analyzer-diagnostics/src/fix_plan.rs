//! Validation, deterministic selection, and all-or-nothing in-memory
//! application of structured diagnostic fixes.
//!
//! This is deliberately a front-end-neutral layer.  It never writes paths:
//! callers provide the exact source snapshots they analysed, receive complete
//! replacement buffers, and may commit those buffers using the transactional
//! primitive appropriate to their environment (CLI, LSP workspace edit, or
//! editor buffer).  Consequently a rejected fix or a stale source can never
//! leave a subset of files modified.

use std::collections::BTreeMap;

use thiserror::Error;

use crate::{Applicability, Diagnostic, FileId, FixProvenance, TextEdit, TextRange};

/// A named source snapshot participating in a [`FixPlan`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedFile {
    /// The analysis-run-local file identity used by diagnostic labels.
    pub file: FileId,
    /// The exact UTF-8 source analysed before the fixes were planned.
    pub source: String,
}

/// One changed source buffer produced by a dry run of a [`FixPlan`].
///
/// Front ends can render this as a diff, feed it into an LSP workspace edit,
/// or atomically persist all of its `after` buffers.  The core deliberately
/// does not choose a path-writing transaction mechanism.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedChange {
    /// The file whose bytes would change.
    pub file: FileId,
    /// The exact analysed source snapshot.
    pub before: String,
    /// The complete replacement source buffer.
    pub after: String,
}

impl PlannedFile {
    /// Construct a source snapshot for fix planning.
    #[must_use]
    pub fn new(file: FileId, source: impl Into<String>) -> Self {
        Self {
            file,
            source: source.into(),
        }
    }
}

/// A validated, deterministic collection of machine-applicable edits.
///
/// A plan owns the source snapshot it was validated against.  [`Self::apply`]
/// refuses different input, closing the stale-source window between analysis
/// and application without performing any I/O itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixPlan {
    files: BTreeMap<FileId, PlannedFile>,
    edits: BTreeMap<FileId, Vec<TextEdit>>,
}

/// Why a [`FixPlan`] could not be built or applied.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum FixPlanError {
    /// The same `FileId` was supplied more than once with different ownership.
    #[error("source snapshots contain duplicate file {file}")]
    DuplicateFile {
        /// The ambiguous file identity.
        file: FileId,
    },
    /// A selected automatic fix has no edits to apply.
    #[error("machine-applicable fix `{title}` has no edits")]
    EmptyFix {
        /// The empty fix title.
        title: String,
    },
    /// An automatic applicability label is not justified by its provenance.
    #[error("fix `{title}` cannot be machine-applicable with {provenance:?} provenance")]
    ApplicabilityPolicy {
        /// The fix title.
        title: String,
        /// The insufficient evidence attached to the fix.
        provenance: FixProvenance,
    },
    /// A diagnostic fix refers to a file that was not supplied to the plan.
    #[error("fix `{title}` refers to unknown file {file}")]
    UnknownFile {
        /// The fix title.
        title: String,
        /// The missing file identity.
        file: FileId,
    },
    /// A fix's edits were not supplied in strictly increasing source order.
    #[error("fix `{title}` has unordered edits in file {file}")]
    UnorderedEdits {
        /// The fix title.
        title: String,
        /// The affected file identity.
        file: FileId,
    },
    /// An edit lies outside the source snapshot.
    #[error("fix `{title}` has an out-of-bounds edit in file {file}: {span:?}")]
    OutOfBounds {
        /// The fix title.
        title: String,
        /// The affected file identity.
        file: FileId,
        /// The invalid range.
        span: TextRange,
    },
    /// An edit splits a UTF-8 code point.
    #[error("fix `{title}` has a non-UTF-8-boundary edit in file {file}: {span:?}")]
    InvalidUtf8Boundary {
        /// The fix title.
        title: String,
        /// The affected file identity.
        file: FileId,
        /// The invalid range.
        span: TextRange,
    },
    /// Two edits cannot both be applied without ambiguity.
    #[error("conflicting fixes `{first}` and `{second}` in file {file}")]
    Conflict {
        /// The affected file identity.
        file: FileId,
        /// Deterministically first conflicting fix title.
        first: String,
        /// Deterministically second conflicting fix title.
        second: String,
    },
    /// The source supplied at application time differs from the analysed snapshot.
    #[error("source for file {file} changed after fixes were planned")]
    StaleSource {
        /// The changed file identity.
        file: FileId,
    },
    /// Application received an input source that was not part of the plan.
    #[error("application received an unplanned source file {file}")]
    UnexpectedSource {
        /// The unexpected file identity.
        file: FileId,
    },
}

impl FixPlan {
    /// Select and validate all machine-applicable diagnostic fixes.
    ///
    /// Suggested and unsafe fixes are intentionally omitted.  This is the
    /// v0.1 applicability policy for model-dependent diagnostics: such fixes
    /// remain visible to clients but cannot be applied by this automatic path.
    /// Any invalid or competing fix rejects the complete plan before a caller
    /// receives changed bytes.
    ///
    /// # Errors
    ///
    /// Returns an error for duplicate files, invalid edit ranges or ordering,
    /// or an overlap between selected fixes.
    pub fn build(
        files: impl IntoIterator<Item = PlannedFile>,
        diagnostics: impl IntoIterator<Item = Diagnostic>,
    ) -> Result<Self, FixPlanError> {
        let mut files_by_id = BTreeMap::new();
        for file in files {
            if files_by_id.insert(file.file, file.clone()).is_some() {
                return Err(FixPlanError::DuplicateFile { file: file.file });
            }
        }

        let mut candidates = Vec::new();
        for diagnostic in diagnostics {
            let Some(fix) = diagnostic.fix else {
                continue;
            };
            if fix.applicability != Applicability::MachineApplicable {
                continue;
            }
            if !fix.provenance.permits_machine_application() {
                return Err(FixPlanError::ApplicabilityPolicy {
                    title: fix.title,
                    provenance: fix.provenance,
                });
            }
            if fix.edits.is_empty() {
                return Err(FixPlanError::EmptyFix { title: fix.title });
            }
            validate_fix(&fix.title, &files_by_id, &fix.edits)?;
            candidates.push(Candidate {
                title: fix.title,
                edits: fix.edits,
            });
        }

        // This key makes both selection and the reported pair deterministic,
        // irrespective of producer/pass ordering.
        candidates.sort_by(|left, right| first_key(left).cmp(&first_key(right)));

        let mut edits = BTreeMap::<FileId, Vec<TextEdit>>::new();
        let mut accepted = BTreeMap::<FileId, Vec<AcceptedEdit>>::new();
        for candidate in &candidates {
            for edit in &candidate.edits {
                let accepted_in_file = accepted.entry(edit.file).or_default();
                if let Some(prior) = accepted_in_file
                    .iter()
                    .find(|prior| edits_overlap(&prior.edit, edit))
                {
                    return Err(FixPlanError::Conflict {
                        file: edit.file,
                        first: prior.title.clone(),
                        second: candidate.title.clone(),
                    });
                }
                accepted_in_file.push(AcceptedEdit {
                    title: candidate.title.clone(),
                    edit: edit.clone(),
                });
                edits.entry(edit.file).or_default().push(edit.clone());
            }
        }

        for file_edits in edits.values_mut() {
            file_edits.sort_by_key(|edit| (edit.span.start(), edit.span.end()));
        }
        Ok(Self {
            files: files_by_id,
            edits,
        })
    }

    /// Return whether this plan contains no machine-applicable edits.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.edits.is_empty()
    }

    /// Apply the plan to exact source snapshots, yielding all output buffers.
    ///
    /// The returned map includes every supplied file, unchanged where no edit
    /// was selected.  Validation occurs before any output is produced.
    ///
    /// # Errors
    ///
    /// Returns [`FixPlanError::StaleSource`] if an input differs from its
    /// analysed source snapshot.
    pub fn apply(
        &self,
        sources: &BTreeMap<FileId, String>,
    ) -> Result<BTreeMap<FileId, String>, FixPlanError> {
        Ok(self
            .transformed_changes(sources)?
            .into_iter()
            .map(|change| (change.file, change.after))
            .collect())
    }

    /// Produce changed buffers without persisting them.
    ///
    /// This is the front-end-neutral dry-run/diff interface.  Entries are in
    /// stable `FileId` order and omit files whose resulting bytes are exactly
    /// unchanged.
    ///
    /// # Errors
    ///
    /// Returns [`FixPlanError::StaleSource`] if an input differs from its
    /// analysed source snapshot.
    pub fn preview(
        &self,
        sources: &BTreeMap<FileId, String>,
    ) -> Result<Vec<PlannedChange>, FixPlanError> {
        Ok(self
            .transformed_changes(sources)?
            .into_iter()
            .filter(|change| change.before != change.after)
            .collect())
    }

    fn transformed_changes(
        &self,
        sources: &BTreeMap<FileId, String>,
    ) -> Result<Vec<PlannedChange>, FixPlanError> {
        self.validate_sources(sources)?;
        Ok(self
            .files
            .iter()
            .map(|(file, snapshot)| {
                let mut after = snapshot.source.clone();
                if let Some(edits) = self.edits.get(file) {
                    for edit in edits.iter().rev() {
                        after.replace_range(
                            to_usize(edit.span.start())..to_usize(edit.span.end()),
                            &edit.new_text,
                        );
                    }
                }
                PlannedChange {
                    file: *file,
                    before: snapshot.source.clone(),
                    after,
                }
            })
            .collect())
    }

    fn validate_sources(&self, sources: &BTreeMap<FileId, String>) -> Result<(), FixPlanError> {
        if let Some(file) = sources.keys().find(|file| !self.files.contains_key(file)) {
            return Err(FixPlanError::UnexpectedSource { file: *file });
        }
        for (file, snapshot) in &self.files {
            if sources.get(file) != Some(&snapshot.source) {
                return Err(FixPlanError::StaleSource { file: *file });
            }
        }
        Ok(())
    }

    /// Return whether this plan would alter any exact supplied source.
    ///
    /// A CLI `--check` mode can use this without writing.  A valid but
    /// no-op replacement returns `false`.
    ///
    /// # Errors
    ///
    /// Returns [`FixPlanError::StaleSource`] if an input differs from its
    /// analysed source snapshot.
    pub fn check(&self, sources: &BTreeMap<FileId, String>) -> Result<bool, FixPlanError> {
        Ok(!self.preview(sources)?.is_empty())
    }
}

#[derive(Debug)]
struct Candidate {
    title: String,
    edits: Vec<TextEdit>,
}

#[derive(Debug)]
struct AcceptedEdit {
    title: String,
    edit: TextEdit,
}

fn validate_fix(
    title: &str,
    files: &BTreeMap<FileId, PlannedFile>,
    edits: &[TextEdit],
) -> Result<(), FixPlanError> {
    let mut by_file = BTreeMap::<FileId, Vec<&TextEdit>>::new();
    for edit in edits {
        let file = edit.file;
        let Some(snapshot) = files.get(&file) else {
            return Err(FixPlanError::UnknownFile {
                title: title.to_owned(),
                file,
            });
        };
        let source = &snapshot.source;
        let start = to_usize(edit.span.start());
        let end = to_usize(edit.span.end());
        if start > source.len() || end > source.len() {
            return Err(FixPlanError::OutOfBounds {
                title: title.to_owned(),
                file,
                span: edit.span,
            });
        }
        if !source.is_char_boundary(start) || !source.is_char_boundary(end) {
            return Err(FixPlanError::InvalidUtf8Boundary {
                title: title.to_owned(),
                file,
                span: edit.span,
            });
        }
        by_file.entry(file).or_default().push(edit);
    }

    for (file, file_edits) in by_file {
        let mut previous: Option<&TextEdit> = None;
        for edit in file_edits {
            if let Some(prior) = previous
                && (edit.span.start() < prior.span.end()
                    || (edit.span == prior.span && edit.span.is_empty()))
            {
                return Err(FixPlanError::UnorderedEdits {
                    title: title.to_owned(),
                    file,
                });
            }
            previous = Some(edit);
        }
    }
    Ok(())
}

fn edits_overlap(left: &TextEdit, right: &TextEdit) -> bool {
    let left_start = left.span.start();
    let left_end = left.span.end();
    let right_start = right.span.start();
    let right_end = right.span.end();
    left_start < right_end && right_start < left_end
        || (left.span == right.span && left.span.is_empty())
}

fn first_key(candidate: &Candidate) -> Option<(FileId, crate::TextSize, crate::TextSize, &str)> {
    candidate
        .edits
        .iter()
        .min_by_key(|edit| (edit.file, edit.span.start(), edit.span.end()))
        .map(|first| {
            (
                first.file,
                first.span.start(),
                first.span.end(),
                candidate.title.as_str(),
            )
        })
}

fn to_usize(size: crate::TextSize) -> usize {
    usize::from(size)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DiagCode, Label, Severity};

    fn diagnostic(
        file: FileId,
        title: &str,
        applicability: Applicability,
        edits: Vec<TextEdit>,
    ) -> Diagnostic {
        Diagnostic::builder(
            DiagCode::BadToken,
            Severity::Warning,
            "fixture",
            Label::new(file, TextRange::default()),
        )
        .fix(crate::Fix {
            title: title.to_owned(),
            applicability,
            provenance: FixProvenance::SyntaxOnly,
            edits,
        })
        .build()
    }

    fn diagnostic_with_provenance(
        file: FileId,
        title: &str,
        applicability: Applicability,
        provenance: FixProvenance,
        edits: Vec<TextEdit>,
    ) -> Diagnostic {
        Diagnostic::builder(
            DiagCode::BadToken,
            Severity::Warning,
            "fixture",
            Label::new(file, TextRange::default()),
        )
        .fix(crate::Fix {
            title: title.to_owned(),
            applicability,
            provenance,
            edits,
        })
        .build()
    }

    fn edit(start: u32, end: u32, text: &str) -> TextEdit {
        edit_in(FileId::new(1), start, end, text)
    }

    fn edit_in(file: FileId, start: u32, end: u32, text: &str) -> TextEdit {
        TextEdit {
            file,
            span: TextRange::new(start.into(), end.into()),
            new_text: text.to_owned(),
        }
    }

    fn sources() -> (Vec<PlannedFile>, BTreeMap<FileId, String>) {
        let files = vec![
            PlannedFile::new(FileId::new(1), "alpha"),
            PlannedFile::new(FileId::new(2), "bravo"),
        ];
        let source_map = files
            .iter()
            .map(|file| (file.file, file.source.clone()))
            .collect();
        (files, source_map)
    }

    #[test]
    fn applies_multiple_files_atomically_and_preserves_unedited_bytes() {
        let (files, source_map) = sources();
        let plan = FixPlan::build(
            files,
            [diagnostic(
                FileId::new(1),
                "rename both files",
                Applicability::MachineApplicable,
                vec![edit(1, 4, "L"), edit_in(FileId::new(2), 1, 4, "AR")],
            )],
        )
        .expect("valid plan");
        let output = plan.apply(&source_map).expect("exact snapshot");
        assert_eq!(output[&FileId::new(1)], "aLa");
        assert_eq!(output[&FileId::new(2)], "bARo");
        assert_eq!(
            plan.preview(&source_map).expect("dry run"),
            vec![
                PlannedChange {
                    file: FileId::new(1),
                    before: "alpha".to_owned(),
                    after: "aLa".to_owned(),
                },
                PlannedChange {
                    file: FileId::new(2),
                    before: "bravo".to_owned(),
                    after: "bARo".to_owned(),
                },
            ]
        );
        assert!(plan.check(&source_map).expect("would change"));
    }

    #[test]
    fn suggested_fixes_are_never_selected_for_automatic_application() {
        let (files, source_map) = sources();
        let plan = FixPlan::build(
            files,
            [diagnostic(
                FileId::new(1),
                "suggestion",
                Applicability::Suggested,
                vec![edit(0, 1, "A")],
            )],
        )
        .expect("suggestion is displayed but not selected");
        assert!(plan.is_empty());
        assert_eq!(plan.apply(&source_map).expect("unchanged"), source_map);
        assert!(!plan.check(&source_map).expect("no machine fix"));
    }

    #[test]
    fn enforces_machine_applicability_provenance_and_rejects_empty_fixes() {
        let (files, _) = sources();
        let err = FixPlan::build(
            files.clone(),
            [diagnostic_with_provenance(
                FileId::new(1),
                "unproven model change",
                Applicability::MachineApplicable,
                FixProvenance::ModelDependent,
                vec![edit(0, 1, "A")],
            )],
        )
        .expect_err("model-dependent fixes need the single-arity proof");
        assert!(matches!(err, FixPlanError::ApplicabilityPolicy { .. }));

        let plan = FixPlan::build(
            files.clone(),
            [diagnostic_with_provenance(
                FileId::new(1),
                "proven arity",
                Applicability::MachineApplicable,
                FixProvenance::SingleArityProven,
                vec![edit(0, 1, "A")],
            )],
        )
        .expect("documented single-arity proof permits automatic application");
        assert!(!plan.is_empty());

        let err = FixPlan::build(
            files.clone(),
            [diagnostic(
                FileId::new(1),
                "empty",
                Applicability::MachineApplicable,
                Vec::new(),
            )],
        )
        .expect_err("empty automatic fix is invalid");
        assert!(matches!(err, FixPlanError::EmptyFix { .. }));
    }

    #[test]
    fn validates_per_edit_file_ownership_and_exact_application_file_set() {
        let (files, mut source_map) = sources();
        let err = FixPlan::build(
            files.clone(),
            [diagnostic(
                FileId::new(1),
                "unknown",
                Applicability::MachineApplicable,
                vec![edit_in(FileId::new(9), 0, 0, "new")],
            )],
        )
        .expect_err("every text edit owns a supplied file");
        assert!(matches!(err, FixPlanError::UnknownFile { file, .. } if file == FileId::new(9)));

        let err = FixPlan::build(
            vec![
                PlannedFile::new(FileId::new(1), "alpha"),
                PlannedFile::new(FileId::new(1), "different"),
            ],
            Vec::<Diagnostic>::new(),
        )
        .expect_err("same FileId cannot have two source owners");
        assert!(matches!(err, FixPlanError::DuplicateFile { .. }));

        let plan = FixPlan::build(
            files,
            [diagnostic(
                FileId::new(1),
                "replace",
                Applicability::MachineApplicable,
                vec![edit(0, 1, "A")],
            )],
        )
        .expect("valid plan");
        source_map.insert(FileId::new(9), "unplanned".to_owned());
        assert_eq!(
            plan.apply(&source_map),
            Err(FixPlanError::UnexpectedSource {
                file: FileId::new(9)
            })
        );
    }

    #[test]
    fn rejects_bad_ranges_and_utf8_boundaries_before_application() {
        let (files, _) = sources();
        let err = FixPlan::build(
            files,
            [diagnostic(
                FileId::new(1),
                "bad",
                Applicability::MachineApplicable,
                vec![edit(0, 9, "")],
            )],
        )
        .expect_err("range exceeds source");
        assert!(matches!(err, FixPlanError::OutOfBounds { .. }));

        let files = vec![PlannedFile::new(FileId::new(3), "é")];
        let err = FixPlan::build(
            files,
            [diagnostic(
                FileId::new(3),
                "utf8",
                Applicability::MachineApplicable,
                vec![edit_in(FileId::new(3), 1, 2, "")],
            )],
        )
        .expect_err("byte one splits é");
        assert!(matches!(err, FixPlanError::InvalidUtf8Boundary { .. }));

        let files = vec![PlannedFile::new(FileId::new(3), "éclair")];
        let source_map = BTreeMap::from([(FileId::new(3), "éclair".to_owned())]);
        let plan = FixPlan::build(
            files,
            [diagnostic(
                FileId::new(3),
                "preserve unicode",
                Applicability::MachineApplicable,
                vec![edit_in(FileId::new(3), 2, 3, "C")],
            )],
        )
        .expect("byte range follows the complete é codepoint");
        assert_eq!(
            plan.apply(&source_map).expect("valid unicode edit")[&FileId::new(3)],
            "éClair"
        );
    }

    #[test]
    fn accepts_insertions_at_eof_and_replacements_ending_at_eof() {
        let (files, source_map) = sources();
        let plan = FixPlan::build(
            files,
            [diagnostic(
                FileId::new(1),
                "finish source",
                Applicability::MachineApplicable,
                vec![edit(4, 5, "A"), edit(5, 5, "!")],
            )],
        )
        .expect("an edit may end at EOF and an insertion may start at EOF");

        assert_eq!(
            plan.apply(&source_map).expect("apply exact source")[&FileId::new(1)],
            "alphA!"
        );
    }

    #[test]
    fn accepts_adjacent_edits_and_distinct_insertions_within_one_fix() {
        let (files, source_map) = sources();
        let adjacent = FixPlan::build(
            files.clone(),
            [diagnostic(
                FileId::new(1),
                "adjacent replacements",
                Applicability::MachineApplicable,
                vec![edit(0, 1, "A"), edit(1, 2, "L")],
            )],
        )
        .expect("adjacent replacement ranges do not overlap");
        assert_eq!(
            adjacent
                .apply(&source_map)
                .expect("apply adjacent replacements")[&FileId::new(1)],
            "ALpha"
        );

        let insertions = FixPlan::build(
            files,
            [diagnostic(
                FileId::new(1),
                "separate insertions",
                Applicability::MachineApplicable,
                vec![edit(1, 1, "-"), edit(4, 4, "+")],
            )],
        )
        .expect("distinct zero-width edits in one fix do not overlap");
        assert_eq!(
            insertions.apply(&source_map).expect("apply insertions")[&FileId::new(1)],
            "a-lph+a"
        );
    }

    #[test]
    fn accepts_adjacent_and_distinct_zero_width_fix_candidates() {
        let (files, source_map) = sources();
        let adjacent = FixPlan::build(
            files.clone(),
            [
                diagnostic(
                    FileId::new(1),
                    "later replacement",
                    Applicability::MachineApplicable,
                    vec![edit(1, 2, "L")],
                ),
                diagnostic(
                    FileId::new(1),
                    "earlier replacement",
                    Applicability::MachineApplicable,
                    vec![edit(0, 1, "A")],
                ),
            ],
        )
        .expect("adjacent fixes do not overlap");
        assert_eq!(
            adjacent.apply(&source_map).expect("apply adjacent fixes")[&FileId::new(1)],
            "ALpha"
        );

        let insertions = FixPlan::build(
            files,
            [
                diagnostic(
                    FileId::new(1),
                    "later insertion",
                    Applicability::MachineApplicable,
                    vec![edit(4, 4, "+")],
                ),
                diagnostic(
                    FileId::new(1),
                    "earlier insertion",
                    Applicability::MachineApplicable,
                    vec![edit(1, 1, "-")],
                ),
            ],
        )
        .expect("distinct zero-width fixes do not overlap");
        assert_eq!(
            insertions
                .apply(&source_map)
                .expect("apply distinct insertions")[&FileId::new(1)],
            "a-lph+a"
        );
    }

    #[test]
    fn accepts_adjacent_edits_when_an_earlier_candidate_has_multiple_edits() {
        let (files, source_map) = sources();
        let plan = FixPlan::build(
            files,
            [
                diagnostic(
                    FileId::new(1),
                    "split outer edits",
                    Applicability::MachineApplicable,
                    vec![edit(0, 1, "A"), edit(4, 5, "A")],
                ),
                diagnostic(
                    FileId::new(1),
                    "middle edit",
                    Applicability::MachineApplicable,
                    vec![edit(1, 4, "MID")],
                ),
            ],
        )
        .expect("adjacent edits in separate candidates do not overlap");

        assert_eq!(
            plan.apply(&source_map).expect("apply adjacent edits")[&FileId::new(1)],
            "AMIDA"
        );
    }

    #[test]
    fn reports_overlapping_fix_conflicts_in_source_order() {
        let (files, _) = sources();
        let err = FixPlan::build(
            files,
            [
                diagnostic(
                    FileId::new(1),
                    "later source span",
                    Applicability::MachineApplicable,
                    vec![edit(2, 4, "X")],
                ),
                diagnostic(
                    FileId::new(1),
                    "earlier source span",
                    Applicability::MachineApplicable,
                    vec![edit(1, 3, "Y")],
                ),
            ],
        )
        .expect_err("overlapping fixes conflict");

        assert_eq!(
            err,
            FixPlanError::Conflict {
                file: FileId::new(1),
                first: "earlier source span".to_owned(),
                second: "later source span".to_owned(),
            }
        );
    }

    #[test]
    fn rejects_ordering_and_cross_fix_conflicts_deterministically() {
        let (files, _) = sources();
        let err = FixPlan::build(
            files.clone(),
            [diagnostic(
                FileId::new(1),
                "unordered",
                Applicability::MachineApplicable,
                vec![edit(3, 4, ""), edit(1, 2, "")],
            )],
        )
        .expect_err("edits must be source ordered");
        assert!(matches!(err, FixPlanError::UnorderedEdits { .. }));

        let err = FixPlan::build(
            files.clone(),
            [
                diagnostic(
                    FileId::new(1),
                    "zeta",
                    Applicability::MachineApplicable,
                    vec![edit(1, 4, "")],
                ),
                diagnostic(
                    FileId::new(1),
                    "alpha",
                    Applicability::MachineApplicable,
                    vec![edit(2, 5, "")],
                ),
            ],
        )
        .expect_err("overlapping diagnostics conflict");
        assert_eq!(
            err,
            FixPlanError::Conflict {
                file: FileId::new(1),
                first: "zeta".to_owned(),
                second: "alpha".to_owned(),
            }
        );

        let err = FixPlan::build(
            files.clone(),
            [diagnostic(
                FileId::new(1),
                "interleaved",
                Applicability::MachineApplicable,
                vec![
                    edit_in(FileId::new(1), 3, 4, ""),
                    edit_in(FileId::new(2), 1, 2, ""),
                    edit_in(FileId::new(1), 1, 2, ""),
                ],
            )],
        )
        .expect_err("ordering is checked independently for every owned file");
        assert!(matches!(err, FixPlanError::UnorderedEdits { .. }));
    }

    #[test]
    fn checks_conflicts_against_every_earlier_multi_edit_fix() {
        let (files, _) = sources();
        let err = FixPlan::build(
            files,
            [
                diagnostic(
                    FileId::new(1),
                    "split",
                    Applicability::MachineApplicable,
                    vec![edit(0, 1, ""), edit(3, 5, "")],
                ),
                diagnostic(
                    FileId::new(1),
                    "between",
                    Applicability::MachineApplicable,
                    vec![edit(1, 2, "")],
                ),
                diagnostic(
                    FileId::new(1),
                    "late-conflict",
                    Applicability::MachineApplicable,
                    vec![edit(4, 5, "")],
                ),
            ],
        )
        .expect_err("later edit conflicts with the earlier split fix");
        assert_eq!(
            err,
            FixPlanError::Conflict {
                file: FileId::new(1),
                first: "split".to_owned(),
                second: "late-conflict".to_owned(),
            }
        );
    }

    #[test]
    fn rejects_stale_input_without_returning_a_partial_result() {
        let (files, mut source_map) = sources();
        let plan = FixPlan::build(
            files,
            [diagnostic(
                FileId::new(1),
                "replace",
                Applicability::MachineApplicable,
                vec![edit(0, 1, "A")],
            )],
        )
        .expect("valid plan");
        source_map.insert(FileId::new(2), "changed".to_owned());
        assert_eq!(
            plan.apply(&source_map),
            Err(FixPlanError::StaleSource {
                file: FileId::new(2)
            })
        );
    }

    #[test]
    fn applying_a_plan_twice_to_its_output_is_rejected_as_stale() {
        let (files, source_map) = sources();
        let plan = FixPlan::build(
            files,
            [diagnostic(
                FileId::new(1),
                "replace",
                Applicability::MachineApplicable,
                vec![edit(0, 1, "A")],
            )],
        )
        .expect("valid plan");
        let output = plan.apply(&source_map).expect("first apply");
        assert!(matches!(
            plan.apply(&output),
            Err(FixPlanError::StaleSource { .. })
        ));
    }
}
