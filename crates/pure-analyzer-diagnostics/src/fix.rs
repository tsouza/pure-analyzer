//! Structured fixes: a [`Fix`] is a set of [`TextEdit`]s, not a rendered
//! string, so front ends can map it to their own edit representation.

use text_size::TextRange;

/// How safe a [`Fix`] is to apply automatically.
///
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Applicability {
    /// Safe to apply without review; `--fix` applies these.
    MachineApplicable,
    /// Shown to the user (CLI text, LSP code action) but never auto-applied.
    Suggested,
    /// Known to be capable of changing behavior; shown only, heavily flagged.
    Unsafe,
}

/// One textual replacement: replace `span` with `new_text`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TextEdit {
    /// The byte range to replace.
    pub span: TextRange,
    /// The replacement text.
    pub new_text: String,
}

/// A structured, applicable-or-not fix for a [`crate::Diagnostic`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Fix {
    /// A short human-readable title (e.g. "insert `%latest` argument").
    pub title: String,
    /// How safe this fix is to apply automatically.
    pub applicability: Applicability,
    /// The edits that make up this fix, applied together.
    pub edits: Vec<TextEdit>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_and_deserializes_round_trip() {
        let fix = Fix {
            title: "insert %latest argument".to_owned(),
            applicability: Applicability::Suggested,
            edits: vec![TextEdit {
                span: TextRange::new(4.into(), 4.into()),
                new_text: "%latest".to_owned(),
            }],
        };
        let json = serde_json::to_string(&fix).expect("serialize");
        let back: Fix = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(fix, back);
    }
}
