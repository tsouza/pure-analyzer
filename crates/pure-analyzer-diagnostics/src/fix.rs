//! Structured fixes: a [`Fix`] is a set of [`TextEdit`]s, not a rendered
//! string, so front ends can map it to their own edit representation.

use text_size::TextRange;

use crate::FileId;

/// How safe a [`Fix`] is to apply automatically.
///
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum Applicability {
    /// Safe to apply without review; `--fix` applies these.
    MachineApplicable,
    /// Shown to the user (CLI text, LSP code action) but never auto-applied.
    Suggested,
    /// Known to be capable of changing behavior; shown only, heavily flagged.
    Unsafe,
}

/// Evidence used to decide whether a fix may be applied automatically.
///
/// Model-dependent v0.1 diagnostics must remain suggestions unless their
/// producer has established the narrow single-arity proof represented by
/// [`Self::SingleArityProven`].  [`crate::FixPlan`] verifies this invariant
/// before selecting a machine-applicable fix.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum FixProvenance {
    /// The edit is justified solely by syntax/local source facts.
    SyntaxOnly,
    /// The edit depends on model facts but has no automatic-application proof.
    ModelDependent,
    /// A model-dependent arity correction has the documented unique proof.
    SingleArityProven,
}

impl FixProvenance {
    /// Return whether this provenance permits `MachineApplicable` selection.
    #[must_use]
    pub const fn permits_machine_application(self) -> bool {
        matches!(self, Self::SyntaxOnly | Self::SingleArityProven)
    }
}

/// One textual replacement: replace `span` with `new_text`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TextEdit {
    /// The source file that owns `span`.
    pub file: FileId,
    /// The byte range to replace.
    pub span: TextRange,
    /// The replacement text.
    pub new_text: String,
}

impl PartialOrd for TextEdit {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for TextEdit {
    /// Order by `(file, span start, span end, new_text)`.
    ///
    /// `TextRange` has no `Ord` impl of its own (its only ordering method,
    /// `ordering`, treats overlapping ranges as equal, which is not a total
    /// order), so this compares its `start()`/`end()` directly — the same
    /// total, deterministic-only ordering [`crate::Diagnostic`]'s span field
    /// needs. See that type's `Ord` impl for why this exists.
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (
            self.file,
            self.span.start(),
            self.span.end(),
            &self.new_text,
        )
            .cmp(&(
                other.file,
                other.span.start(),
                other.span.end(),
                &other.new_text,
            ))
    }
}

/// A structured, applicable-or-not fix for a [`crate::Diagnostic`].
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub struct Fix {
    /// A short human-readable title (e.g. "insert `%latest` argument").
    pub title: String,
    /// How safe this fix is to apply automatically.
    pub applicability: Applicability,
    /// The evidence supporting the applicability classification.
    pub provenance: FixProvenance,
    /// The edits that make up this fix, applied together.
    pub edits: Vec<TextEdit>,
}

impl Fix {
    /// Construct a model-dependent fix that must remain a suggestion.
    #[must_use]
    pub fn model_dependent(title: impl Into<String>, edits: Vec<TextEdit>) -> Self {
        Self {
            title: title.into(),
            applicability: Applicability::Suggested,
            provenance: FixProvenance::ModelDependent,
            edits,
        }
    }

    /// Construct a machine-applicable fix with a proven single arity.
    #[must_use]
    pub fn single_arity_proven(title: impl Into<String>, edits: Vec<TextEdit>) -> Self {
        Self {
            title: title.into(),
            applicability: Applicability::MachineApplicable,
            provenance: FixProvenance::SingleArityProven,
            edits,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_ord_agrees_with_ord_and_orders_by_span() {
        // `TextEdit`'s `PartialOrd` delegates to its manual `Ord`; a mutant
        // that replaces it with an always-`None` stub leaves `Ord::cmp`
        // untouched but breaks `<`/`<=` (which go through `PartialOrd`) and
        // any direct `partial_cmp` caller.
        let earlier = TextEdit {
            file: FileId::new(0),
            span: TextRange::new(0.into(), 1.into()),
            new_text: "a".to_owned(),
        };
        let later = TextEdit {
            file: FileId::new(0),
            span: TextRange::new(1.into(), 2.into()),
            new_text: "b".to_owned(),
        };
        assert_eq!(earlier.partial_cmp(&later), Some(earlier.cmp(&later)));
        assert!(earlier < later);
    }

    #[test]
    fn serializes_and_deserializes_round_trip() {
        let fix = Fix {
            title: "insert %latest argument".to_owned(),
            applicability: Applicability::Suggested,
            provenance: FixProvenance::ModelDependent,
            edits: vec![TextEdit {
                file: FileId::new(7),
                span: TextRange::new(4.into(), 4.into()),
                new_text: "%latest".to_owned(),
            }],
        };
        let json = serde_json::to_string(&fix).expect("serialize");
        let back: Fix = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(fix, back);
    }
}
