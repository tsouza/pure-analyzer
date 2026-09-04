//! The [`Diagnostic`] type itself, plus [`Label`], [`Severity`], and a small
//! builder so pass authors don't hand-write nine-field struct literals.

use text_size::TextRange;

use crate::code::DiagCode;
use crate::file::FileId;
use crate::fix::Fix;

/// How serious a [`Diagnostic`] is, independent of which pass produced it.
///
/// The CLI computes exit-code and `--deny`/`--warn` policy from this field;
/// this crate assigns no exit-code meaning to it.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// A finding that fails the run outright (subject to `--deny`/`--strict`).
    Error,
    /// A finding that is surfaced but does not fail the run by default.
    Warning,
    /// Informational: e.g. a derived qualified property, a synthesized fact.
    Info,
    /// An editor-only annotation, never rendered by the CLI's default format.
    Hint,
}

/// One labeled span within a [`Diagnostic`]: a byte range in a specific file,
/// plus a short note explaining what that span shows.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Label {
    // NOTE: `TextRange`'s (De)Serialize impls come from text-size's `serde`
    // feature (enabled workspace-wide); no `&'static str` fields here, so
    // this type — unlike `Diagnostic` below — can safely round-trip.
    /// The file the span is in.
    pub file: FileId,
    /// The byte-offset range within `file`.
    pub span: TextRange,
    /// A short note rendered alongside the span (e.g. "target class declared
    /// here").
    pub note: String,
}

impl PartialOrd for Label {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Label {
    /// Order by `(file, span start, span end, note)`.
    ///
    /// `TextRange` has no `Ord` impl (see [`crate::fix::TextEdit`]'s `Ord`
    /// impl for why), so this compares `span.start()`/`span.end()` directly.
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (self.file, self.span.start(), self.span.end(), &self.note).cmp(&(
            other.file,
            other.span.start(),
            other.span.end(),
            &other.note,
        ))
    }
}

impl Label {
    /// Construct a `Label` with an empty note.
    #[must_use]
    pub fn new(file: FileId, span: TextRange) -> Self {
        Self {
            file,
            span,
            note: String::new(),
        }
    }

    /// Construct a `Label` and set its note in one call.
    #[must_use]
    pub fn with_note(file: FileId, span: TextRange, note: impl Into<String>) -> Self {
        Self {
            file,
            span,
            note: note.into(),
        }
    }
}

/// A finding produced by a pass: the parser's syntax errors, `lint`'s
/// milestoning-arity findings.
///
/// Findings flow one way, from passes to renderers, so this type is
/// intentionally serialization-only. Its identifiers are closed enums rather
/// than caller-provided strings.
///
/// `PartialOrd`/`Ord` order by every field in declaration order. This is a
/// deterministic total order, not a human-facing sort: `code` orders by this
/// enum's declared variant order, not [`DiagCode::as_str`]'s `PUR`-number
/// text (a caller that wants the human-facing order, e.g. to present
/// findings grouped by code, should sort on that explicitly rather than rely
/// on this impl). Its purpose is to give callers like
/// `pure-analyzer-analysis`'s pass engine a cheap, total tie-breaker for two
/// diagnostics that share every field a human-facing sort key already
/// covers, without round-tripping through a serialization format.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
pub struct Diagnostic {
    /// Stable registered identifier, e.g. [`DiagCode::WrongMilestoningArity`].
    pub code: DiagCode,
    /// How serious this finding is.
    pub severity: Severity,
    /// The human-readable finding message.
    pub message: String,
    /// The primary span this diagnostic points at.
    pub primary: Label,
    /// Additional spans (e.g. the target class's stereotype declaration).
    pub secondary: Vec<Label>,
    /// A structured fix, if one is available.
    pub fix: Option<Fix>,
    /// A doc link into `docs/reason-codes/<code>`, if one exists.
    pub url: Option<String>,
}

impl Diagnostic {
    /// Start building a `Diagnostic` with the required fields (`code`,
    /// `severity`, `message`, `primary`); every other field defaults to
    /// empty/`None` and can be set via the builder's `with_*` methods.
    #[must_use]
    pub fn builder(
        code: DiagCode,
        severity: Severity,
        message: impl Into<String>,
        primary: Label,
    ) -> DiagnosticBuilder {
        DiagnosticBuilder::new(code, severity, message, primary)
    }
}

/// Builder for [`Diagnostic`]. Construct via [`Diagnostic::builder`].
#[derive(Debug, Clone)]
pub struct DiagnosticBuilder {
    inner: Diagnostic,
}

impl DiagnosticBuilder {
    fn new(code: DiagCode, severity: Severity, message: impl Into<String>, primary: Label) -> Self {
        Self {
            inner: Diagnostic {
                code,
                severity,
                message: message.into(),
                primary,
                secondary: Vec::new(),
                fix: None,
                url: None,
            },
        }
    }

    /// Append a secondary label.
    #[must_use]
    pub fn secondary(mut self, label: Label) -> Self {
        self.inner.secondary.push(label);
        self
    }

    /// Attach a structured fix.
    #[must_use]
    pub fn fix(mut self, fix: Fix) -> Self {
        self.inner.fix = Some(fix);
        self
    }

    /// Attach a doc-link URL.
    #[must_use]
    pub fn url(mut self, url: impl Into<String>) -> Self {
        self.inner.url = Some(url.into());
        self
    }

    /// Finish building the `Diagnostic`.
    #[must_use]
    pub fn build(self) -> Diagnostic {
        self.inner
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn label() -> Label {
        Label::new(FileId::new(0), TextRange::new(0.into(), 4.into()))
    }

    #[test]
    fn label_partial_ord_agrees_with_ord_and_orders_by_span() {
        // `Label`'s `PartialOrd` delegates to its manual `Ord`; a mutant
        // that replaces it with an always-`None` stub leaves `Ord::cmp`
        // untouched but breaks `<`/`<=` (which go through `PartialOrd`) and
        // any direct `partial_cmp` caller.
        let earlier = Label::new(FileId::new(0), TextRange::new(0.into(), 1.into()));
        let later = Label::new(FileId::new(0), TextRange::new(1.into(), 2.into()));
        assert_eq!(earlier.partial_cmp(&later), Some(earlier.cmp(&later)));
        assert!(earlier < later);
    }

    #[test]
    fn builder_defaults_optional_fields_to_empty() {
        let d = Diagnostic::builder(DiagCode::BadToken, Severity::Error, "boom", label()).build();
        assert_eq!(d.code, DiagCode::BadToken);
        assert_eq!(d.severity, Severity::Error);
        assert!(d.secondary.is_empty());
        assert!(d.fix.is_none());
        assert!(d.url.is_none());
    }

    #[test]
    fn builder_chains_optional_fields() {
        let secondary = Label::with_note(
            FileId::new(0),
            TextRange::new(5.into(), 9.into()),
            "declared here",
        );
        let d = Diagnostic::builder(
            DiagCode::WrongMilestoningArity,
            Severity::Warning,
            "wrong arity",
            label(),
        )
        .secondary(secondary.clone())
        .url("https://example.invalid/PUR2001")
        .build();
        assert_eq!(d.secondary, vec![secondary]);
        assert_eq!(d.url.as_deref(), Some("https://example.invalid/PUR2001"));
    }

    #[test]
    fn serializes_to_the_expected_json_shape() {
        let d = Diagnostic::builder(
            DiagCode::WrongMilestoningArity,
            Severity::Error,
            "wrong arity",
            label(),
        )
        .build();
        let value: serde_json::Value = serde_json::to_value(&d).expect("serialize");
        assert_eq!(value["code"], "PUR2001");
        assert_eq!(value["severity"], "error");
        assert_eq!(value["message"], "wrong arity");
        assert_eq!(value["primary"]["file"], 0);
    }

    #[test]
    fn severity_orders_error_first() {
        assert!(Severity::Error < Severity::Warning);
        assert!(Severity::Warning < Severity::Info);
        assert!(Severity::Info < Severity::Hint);
    }
}
