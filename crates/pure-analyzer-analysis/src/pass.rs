//! Deterministic orchestration for independent analysis passes.

use std::collections::{BTreeMap, BTreeSet};

use pure_analyzer_diagnostics::{DiagCode, Diagnostic, FileId, Severity};
use pure_analyzer_model::ModelGraph;
use pure_analyzer_syntax::GreenNode;

/// Immutable input shared with one analysis pass.
#[derive(Debug, Clone, Copy)]
pub struct AnalysisInput<'source, 'model> {
    file: FileId,
    source: &'source str,
    tree: &'source GreenNode,
    parse_diagnostics: &'source [Diagnostic],
    model: Option<&'model ModelGraph>,
}

impl<'source, 'model> AnalysisInput<'source, 'model> {
    /// Construct input for one parsed source file and its optional model facts.
    #[must_use]
    pub const fn new(
        file: FileId,
        source: &'source str,
        tree: &'source GreenNode,
        parse_diagnostics: &'source [Diagnostic],
        model: Option<&'model ModelGraph>,
    ) -> Self {
        Self {
            file,
            source,
            tree,
            parse_diagnostics,
            model,
        }
    }

    /// Return the stable file identifier assigned by the front end.
    #[must_use]
    pub const fn file(self) -> FileId {
        self.file
    }

    /// Return the immutable lossless syntax tree for this source file.
    #[must_use]
    pub const fn tree(self) -> &'source GreenNode {
        self.tree
    }

    /// Return original source text for exact-span validation checks.
    #[must_use]
    pub const fn source(self) -> &'source str {
        self.source
    }

    /// Return lexer and parser recovery findings for this parsed file.
    #[must_use]
    pub const fn parse_diagnostics(self) -> &'source [Diagnostic] {
        self.parse_diagnostics
    }

    /// Return the loaded model when the caller supplied one.
    #[must_use]
    pub const fn model(self) -> Option<&'model ModelGraph> {
        self.model
    }

    /// Describe whether this invocation has model facts available.
    #[must_use]
    pub const fn model_availability(self) -> ModelAvailability {
        if self.model.is_some() {
            ModelAvailability::Available
        } else {
            ModelAvailability::Unavailable
        }
    }
}

/// Whether the caller provided model facts to an analysis invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelAvailability {
    /// Passes may resolve source against a [`ModelGraph`].
    Available,
    /// Only model-free passes may produce semantic findings.
    Unavailable,
}

/// One independently testable analyzer rule set.
///
/// Implementations must return only facts derived from `input`; the engine
/// canonicalizes their output before exposing it to a caller.
pub trait AnalysisPass: std::fmt::Debug + Send + Sync {
    /// Return the unique, stable machine-facing pass name.
    fn name(&self) -> &'static str;

    /// Produce findings for one parsed input.
    fn analyze(&self, input: AnalysisInput<'_, '_>) -> Vec<Diagnostic>;
}

/// Policy applied after all analysis passes have produced findings.
///
/// Suppression is evaluated before severity overrides. Findings below the
/// configured minimum severity are removed after overrides are applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FindingPolicy {
    suppressed: BTreeSet<DiagCode>,
    severity_overrides: BTreeMap<DiagCode, Severity>,
    minimum_severity: Severity,
}

impl Default for FindingPolicy {
    fn default() -> Self {
        Self {
            suppressed: BTreeSet::new(),
            severity_overrides: BTreeMap::new(),
            minimum_severity: Severity::Warning,
        }
    }
}

impl FindingPolicy {
    /// Construct the default policy, which retains warnings and errors.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Suppress every finding with `code`.
    #[must_use]
    pub fn suppress(mut self, code: DiagCode) -> Self {
        self.suppressed.insert(code);
        self
    }

    /// Override the presentation severity for a registered diagnostic code.
    #[must_use]
    pub fn with_severity(mut self, code: DiagCode, severity: Severity) -> Self {
        self.severity_overrides.insert(code, severity);
        self
    }

    /// Retain only findings at or above `minimum_severity`.
    #[must_use]
    pub const fn with_minimum_severity(mut self, minimum_severity: Severity) -> Self {
        self.minimum_severity = minimum_severity;
        self
    }

    fn apply(&self, mut diagnostic: Diagnostic) -> Option<Diagnostic> {
        if self.suppressed.contains(&diagnostic.code) {
            return None;
        }
        if let Some(severity) = self.severity_overrides.get(&diagnostic.code) {
            diagnostic.severity = *severity;
        }
        (severity_rank(diagnostic.severity) <= severity_rank(self.minimum_severity))
            .then_some(diagnostic)
    }
}

/// Canonical findings from one analysis-engine invocation.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AnalysisResult {
    diagnostics: Vec<Diagnostic>,
}

impl AnalysisResult {
    /// Return canonical, de-duplicated diagnostics in deterministic order.
    #[must_use]
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    /// Consume this result and return its canonical diagnostics.
    #[must_use]
    pub fn into_diagnostics(self) -> Vec<Diagnostic> {
        self.diagnostics
    }
}

/// Deterministic orchestrator for independent analysis passes.
#[derive(Debug)]
pub struct AnalysisEngine {
    passes: Vec<Box<dyn AnalysisPass>>,
    policy: FindingPolicy,
}

impl AnalysisEngine {
    /// Construct an engine and sort passes by their stable names.
    #[must_use]
    pub fn new(mut passes: Vec<Box<dyn AnalysisPass>>, policy: FindingPolicy) -> Self {
        passes.sort_by_key(|pass| pass.name());
        Self { passes, policy }
    }

    /// Return pass names in the deterministic execution order.
    pub fn pass_names(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.passes.iter().map(|pass| pass.name())
    }

    /// Run every pass and return policy-filtered canonical findings.
    #[must_use]
    pub fn analyze(&self, input: AnalysisInput<'_, '_>) -> AnalysisResult {
        let mut diagnostics = self
            .passes
            .iter()
            .flat_map(|pass| pass.analyze(input))
            .filter_map(|diagnostic| self.policy.apply(diagnostic))
            .collect::<Vec<_>>();
        diagnostics.sort_by(compare_diagnostics);
        diagnostics.dedup();
        AnalysisResult { diagnostics }
    }
}

fn compare_diagnostics(left: &Diagnostic, right: &Diagnostic) -> std::cmp::Ordering {
    (
        left.code.as_str(),
        severity_rank(left.severity),
        left.primary.file.index(),
        left.primary.span.start(),
        left.primary.span.end(),
        left.message.as_str(),
    )
        .cmp(&(
            right.code.as_str(),
            severity_rank(right.severity),
            right.primary.file.index(),
            right.primary.span.start(),
            right.primary.span.end(),
            right.message.as_str(),
        ))
        .then_with(|| canonical_diagnostic(left).cmp(&canonical_diagnostic(right)))
}

fn canonical_diagnostic(diagnostic: &Diagnostic) -> String {
    // `Diagnostic` is a closed, data-only serializable model: its JSON encoding
    // contains every field that participates in equality. This tie-breaker
    // prevents a pass's incidental emission order from leaking into output when
    // two distinct findings share the human-facing sort fields above.
    serde_json::to_string(diagnostic).unwrap_or_default()
}

const fn severity_rank(severity: Severity) -> u8 {
    match severity {
        Severity::Error => 0,
        Severity::Warning => 1,
        Severity::Info => 2,
        Severity::Hint => 3,
    }
}

#[cfg(test)]
mod tests {
    use pure_analyzer_diagnostics::{Diagnostic, Label, TextRange};
    use pure_analyzer_parser::parse_query;

    use super::*;

    #[derive(Debug)]
    struct StaticPass {
        name: &'static str,
        diagnostics: Vec<Diagnostic>,
    }

    impl AnalysisPass for StaticPass {
        fn name(&self) -> &'static str {
            self.name
        }

        fn analyze(&self, _input: AnalysisInput<'_, '_>) -> Vec<Diagnostic> {
            self.diagnostics.clone()
        }
    }

    fn diagnostic(code: DiagCode, severity: Severity, message: &str) -> Diagnostic {
        Diagnostic::builder(
            code,
            severity,
            message,
            Label::new(FileId::new(4), TextRange::new(2.into(), 5.into())),
        )
        .build()
    }

    fn input() -> AnalysisInput<'static, 'static> {
        let parse = Box::leak(Box::new(
            parse_query("Person.all()", FileId::new(4)).expect("parse fixture"),
        ));
        AnalysisInput::new(
            FileId::new(4),
            "Person.all()",
            &parse.green,
            &parse.diagnostics,
            None,
        )
    }

    #[test]
    fn input_exposes_the_exact_source_and_supplied_model() {
        let parse = parse_query("Person.all()", FileId::new(4)).expect("parse fixture");
        let model = ModelGraph::default();
        let input = AnalysisInput::new(
            FileId::new(4),
            "Person.all()",
            &parse.green,
            &parse.diagnostics,
            Some(&model),
        );

        assert_eq!(input.source(), "Person.all()");
        assert!(std::ptr::eq(input.model().expect("supplied model"), &model));
        assert_eq!(input.model_availability(), ModelAvailability::Available);
    }

    #[test]
    fn result_consumes_the_engine_findings() {
        let finding = diagnostic(DiagCode::UnknownProperty, Severity::Warning, "unknown");
        let engine = AnalysisEngine::new(
            vec![Box::new(StaticPass {
                name: "only",
                diagnostics: vec![finding.clone()],
            })],
            FindingPolicy::new(),
        );

        assert_eq!(engine.analyze(input()).into_diagnostics(), vec![finding]);
    }

    #[test]
    fn engine_sorts_passes_findings_and_deduplicates_equal_findings() {
        let duplicate = diagnostic(DiagCode::UnknownProperty, Severity::Error, "unknown");
        let engine = AnalysisEngine::new(
            vec![
                Box::new(StaticPass {
                    name: "zeta",
                    diagnostics: vec![duplicate.clone()],
                }),
                Box::new(StaticPass {
                    name: "alpha",
                    diagnostics: vec![
                        diagnostic(DiagCode::WrongMilestoningArity, Severity::Warning, "arity"),
                        duplicate,
                    ],
                }),
            ],
            FindingPolicy::new(),
        );

        assert_eq!(engine.pass_names().collect::<Vec<_>>(), ["alpha", "zeta"]);
        let result = engine.analyze(input());
        assert_eq!(result.diagnostics().len(), 2);
        assert_eq!(
            result.diagnostics()[0].code,
            DiagCode::WrongMilestoningArity
        );
        assert_eq!(result.diagnostics()[1].code, DiagCode::UnknownProperty);
    }

    #[test]
    fn engine_breaks_equal_primary_sort_keys_with_all_diagnostic_fields() {
        let first = Diagnostic::builder(
            DiagCode::UnknownProperty,
            Severity::Warning,
            "unknown",
            Label::new(FileId::new(4), TextRange::new(2.into(), 5.into())),
        )
        .secondary(Label::with_note(
            FileId::new(2),
            TextRange::new(1.into(), 3.into()),
            "a",
        ))
        .build();
        let second = Diagnostic::builder(
            DiagCode::UnknownProperty,
            Severity::Warning,
            "unknown",
            Label::new(FileId::new(4), TextRange::new(2.into(), 5.into())),
        )
        .secondary(Label::with_note(
            FileId::new(2),
            TextRange::new(1.into(), 3.into()),
            "b",
        ))
        .build();
        let engine = AnalysisEngine::new(
            vec![Box::new(StaticPass {
                name: "only",
                diagnostics: vec![second, first],
            })],
            FindingPolicy::new(),
        );

        let result = engine.analyze(input());
        assert_eq!(result.diagnostics().len(), 2);
        assert_eq!(result.diagnostics()[0].secondary[0].note, "a");
        assert_eq!(result.diagnostics()[1].secondary[0].note, "b");
    }

    #[test]
    fn policy_suppresses_before_overriding_and_applies_minimum_severity() {
        let engine = AnalysisEngine::new(
            vec![Box::new(StaticPass {
                name: "only",
                diagnostics: vec![
                    diagnostic(DiagCode::UnknownProperty, Severity::Error, "hidden"),
                    diagnostic(DiagCode::CardinalityMisuse, Severity::Hint, "promoted"),
                ],
            })],
            FindingPolicy::new()
                .suppress(DiagCode::UnknownProperty)
                .with_severity(DiagCode::CardinalityMisuse, Severity::Warning),
        );

        let result = engine.analyze(input());
        assert_eq!(result.diagnostics().len(), 1);
        assert_eq!(result.diagnostics()[0].code, DiagCode::CardinalityMisuse);
        assert_eq!(result.diagnostics()[0].severity, Severity::Warning);
    }

    #[test]
    fn policy_filters_below_the_minimum_without_filtering_errors() {
        let engine = AnalysisEngine::new(
            vec![Box::new(StaticPass {
                name: "only",
                diagnostics: vec![
                    diagnostic(DiagCode::UnknownProperty, Severity::Hint, "quiet"),
                    diagnostic(DiagCode::CardinalityMisuse, Severity::Error, "loud"),
                ],
            })],
            FindingPolicy::new().with_minimum_severity(Severity::Warning),
        );

        let result = engine.analyze(input());
        assert_eq!(result.diagnostics().len(), 1);
        assert_eq!(result.diagnostics()[0].code, DiagCode::CardinalityMisuse);
    }
}
