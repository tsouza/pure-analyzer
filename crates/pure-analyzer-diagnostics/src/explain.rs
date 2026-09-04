//! Renderer-neutral explain content for diagnostic and reason identifiers.

use crate::{DiagCode, ReasonCode};

/// The documentation index for registered explain content.
pub const EXPLAIN_INDEX_URL: &str =
    concat!(env!("CARGO_PKG_REPOSITORY"), "/tree/main/docs/explain");

#[cfg(test)]
const EXPLAIN_DOCUMENTATION_URL_ROOT: &str =
    concat!(env!("CARGO_PKG_REPOSITORY"), "/blob/main/docs/explain");

/// Whether explain content describes a diagnostic finding or a conservative reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExplainKind {
    /// A registered `PUR<nnnn>` diagnostic finding.
    Diagnostic,
    /// A registered reason attached to an inconclusive or downgraded outcome.
    Reason,
}

impl ExplainKind {
    /// The stable lowercase value used in structured output.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Diagnostic => "diagnostic",
            Self::Reason => "reason",
        }
    }
}

/// The stable semantic classification of [`ExplainContent`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExplainClassification {
    /// Lexer and island diagnostics.
    Lexer,
    /// Parser and grammar-validation diagnostics.
    Parser,
    /// Resolution and lint diagnostics.
    Lint,
    /// Tool, configuration, and model diagnostics.
    Tool,
    /// A soundness boundary, not an input error.
    Fundamental,
    /// A conservative implementation limitation, not an input error.
    Recoverable,
}

impl ExplainClassification {
    /// The stable lowercase value used in structured output.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Lexer => "lexer",
            Self::Parser => "parser",
            Self::Lint => "lint",
            Self::Tool => "tool",
            Self::Fundamental => "fundamental",
            Self::Recoverable => "recoverable",
        }
    }
}

/// Structured, renderer-neutral content for one registered identifier.
///
/// Front ends can choose their presentation while preserving the same stable
/// identifier, classification, user-facing explanation, and documentation URL.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct ExplainContent {
    /// The exact registered diagnostic or reason identifier.
    pub identifier: &'static str,
    /// Whether this content describes a diagnostic finding or a reason.
    pub kind: ExplainKind,
    /// The diagnostic family or reason-limit classification.
    pub classification: ExplainClassification,
    /// What the identifier reports.
    pub meaning: &'static str,
    /// What this identifier does not establish.
    pub limit: &'static str,
    /// The most useful next action.
    pub remedy: &'static str,
    /// The stable documentation URL for this identifier.
    pub documentation_url: &'static str,
}

/// An identifier that is absent from both closed explain registries.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "unknown explain identifier {value:?}; expected an exact registered diagnostic or reason identifier; see {EXPLAIN_INDEX_URL}"
)]
pub struct UnknownExplainIdentifier {
    value: String,
}

impl UnknownExplainIdentifier {
    /// The rejected identifier.
    #[must_use]
    pub fn value(&self) -> &str {
        &self.value
    }
}

macro_rules! explain_content {
    ($name:ident, $identifier:literal, $kind:ident, $classification:ident, $meaning:literal, $limit:literal, $remedy:literal) => {
        static $name: ExplainContent = ExplainContent {
            identifier: $identifier,
            kind: ExplainKind::$kind,
            classification: ExplainClassification::$classification,
            meaning: $meaning,
            limit: $limit,
            remedy: $remedy,
            documentation_url: concat!(
                env!("CARGO_PKG_REPOSITORY"),
                "/blob/main/docs/explain/",
                $identifier,
                ".md"
            ),
        };
    };
}

explain_content!(
    PUR0101,
    "PUR0101",
    Diagnostic,
    Lexer,
    "An island reaches end-of-input before its matching terminator.",
    "This establishes only delimiter balance; it does not validate the island's inner semantics.",
    "Close the island with its matching terminator, then validate the inner text."
);
explain_content!(
    PUR0102,
    "PUR0102",
    Diagnostic,
    Lexer,
    "The lexer encountered input that is not a token in the supported Pure surface.",
    "This establishes lexical validity only; later syntax and semantic checks are separate.",
    "Remove or replace the unrecognized character sequence."
);
explain_content!(
    PUR1200,
    "PUR1200",
    Diagnostic,
    Parser,
    "Source tokens do not form a complete supported declaration or expression.",
    "This reports the parsed surface shape and does not establish model-dependent semantics.",
    "Repair the reported syntax, then rerun analysis with the relevant model."
);
explain_content!(
    PUR1201,
    "PUR1201",
    Diagnostic,
    Parser,
    "A parenthesized value tuple is admitted for targeted validation but is not accepted in that position.",
    "This targets the tuple shape itself and does not validate every surrounding expression.",
    "Use a supported collection or call form for the intended values."
);
explain_content!(
    PUR1202,
    "PUR1202",
    Diagnostic,
    Parser,
    "A bracket index is neither a string literal nor an integer literal.",
    "This checks bracket-index shape and does not prove that a referenced column exists.",
    "Use a literal index such as `$row['name']` or `$row[0]`."
);
explain_content!(
    PUR1204,
    "PUR1204",
    Diagnostic,
    Parser,
    "Milestoning parentheses have an invalid surface shape.",
    "This does not decide the model-dependent number of valid dates; PUR2001 performs that check.",
    "Use a syntactically valid argument list, then run lint with the relevant model."
);
explain_content!(
    PUR1210,
    "PUR1210",
    Diagnostic,
    Parser,
    "A relation join kind is outside the supported closed set.",
    "This checks join-kind vocabulary, not whether the full join expression is semantically valid.",
    "Replace the value with a supported join kind."
);
explain_content!(
    PUR2001,
    "PUR2001",
    Diagnostic,
    Lint,
    "A navigation supplies a number of milestoning dates that disagrees with a known target stereotype.",
    "An incomplete model must produce a conservative result rather than an error-level claim.",
    "Load the relevant model and provide the date count required by the target class."
);
explain_content!(
    PUR2002,
    "PUR2002",
    Diagnostic,
    Lint,
    "A closed-world source class has no property with the requested name.",
    "An open or incomplete model cannot justify an error-level unknown-property conclusion.",
    "Correct the property name or load the model that defines it."
);
explain_content!(
    PUR2101,
    "PUR2101",
    Diagnostic,
    Lint,
    "Local inference cannot determine the source of a navigation.",
    "This is a conservative lack of proof, not evidence that the navigation is invalid.",
    "Make the source type available through a model or a simpler local binding."
);
explain_content!(
    PUR9000,
    "PUR9000",
    Diagnostic,
    Tool,
    "A later model input replaces an earlier definition with the same identity.",
    "This reports input composition; it does not determine whether either definition is semantically correct.",
    "Reconcile duplicate definitions or provide one authoritative model input."
);
explain_content!(
    PUR9002,
    "PUR9002",
    Diagnostic,
    Tool,
    "One model source declares the same fact more than once.",
    "This reports duplicate source input and does not decide which declaration was intended.",
    "Keep one authoritative declaration for the duplicated fact."
);
explain_content!(
    PUR9003,
    "PUR9003",
    Diagnostic,
    Tool,
    "A Pure association cannot be materialized without ambiguity.",
    "The analyzer stays conservative rather than inventing association ends or ownership.",
    "Make the association ends and their ownership unambiguous in the model."
);

explain_content!(
    IND_WINDOW,
    "IND_WINDOW",
    Reason,
    Fundamental,
    "Window and OLAP-frame equivalence is outside the sound core.",
    "This fundamental boundary does not make either query erroneous.",
    "Compare the result with domain-specific evidence."
);
explain_content!(
    IND_PARETO,
    "IND_PARETO",
    Reason,
    Fundamental,
    "Pareto and top-per-group equivalence depends on unmodeled tie semantics.",
    "This fundamental boundary does not make either query erroneous.",
    "Make tie semantics explicit or validate the comparison with domain-specific evidence."
);
explain_content!(
    IND_MULTISTEP_FISCAL,
    "IND_MULTISTEP_FISCAL",
    Reason,
    Fundamental,
    "Multi-step fiscal accumulation equivalence is outside the sound core.",
    "This fundamental boundary does not make either query erroneous.",
    "Validate the fiscal transformation with domain-specific evidence."
);
explain_content!(
    IND_DIVISION_RATIO,
    "IND_DIVISION_RATIO",
    Reason,
    Fundamental,
    "Division and ratio equivalence is outside the sound core.",
    "This fundamental boundary does not make either query erroneous.",
    "Avoid the rewrite for a definitive core verdict or validate it separately."
);
explain_content!(
    IND_MILESTONING_ASOF,
    "IND_MILESTONING_ASOF",
    Reason,
    Fundamental,
    "Bitemporal as-of equivalence is outside the sound core.",
    "This fundamental boundary does not make either query erroneous.",
    "Validate the temporal law with domain-specific evidence."
);
explain_content!(
    IND_ORDER_UNDERDETERMINED,
    "IND_ORDER_UNDERDETERMINED",
    Reason,
    Fundamental,
    "The available facts do not prove a total order for an order-sensitive operation.",
    "This fundamental boundary does not make either query erroneous.",
    "Add a deterministic tie-breaker or validate behavior under the relevant ordering contract."
);
explain_content!(
    IND_OPAQUE_PREDICATE,
    "IND_OPAQUE_PREDICATE",
    Reason,
    Fundamental,
    "A predicate falls outside the sound interpreted whitelist.",
    "This fundamental boundary does not make either query erroneous.",
    "Express the predicate with modeled operations or validate the comparison separately."
);
explain_content!(
    IND_DIFFERENT_SOURCES,
    "IND_DIFFERENT_SOURCES",
    Reason,
    Fundamental,
    "The two queries read different named sources.",
    "This fundamental boundary does not make either query erroneous.",
    "Compare queries over a shared source or provide a source-equivalence argument."
);
explain_content!(
    IND_MISSING_REWRITE,
    "IND_MISSING_REWRITE",
    Reason,
    Recoverable,
    "The normalizer lacks a known sound rewrite for the observed structural difference.",
    "This recoverable limitation does not make either query erroneous.",
    "Use a simpler shared form or investigate a sound normalization rule."
);
explain_content!(
    IND_UNMODELED_OP,
    "IND_UNMODELED_OP",
    Reason,
    Recoverable,
    "A relational operator has no sound semantic model in the analyzer.",
    "This recoverable limitation does not make either query erroneous.",
    "Avoid the operator for a core comparison or establish its semantics separately."
);
explain_content!(
    IND_OPAQUE_FUNCTION_IN_WITNESS,
    "IND_OPAQUE_FUNCTION_IN_WITNESS",
    Reason,
    Recoverable,
    "Witness evaluation encountered a function whose semantics are not interpreted.",
    "This recoverable limitation does not make either query erroneous.",
    "Use modeled functions or validate the candidate witness outside the analyzer."
);
explain_content!(
    IND_UNRESOLVED_SCHEMA,
    "IND_UNRESOLVED_SCHEMA",
    Reason,
    Recoverable,
    "The available model facts do not resolve a schema property required for a hard conclusion.",
    "This recoverable limitation does not make either query erroneous.",
    "Load the model that supplies the missing schema facts."
);
explain_content!(
    IND_WITNESS_BUDGET_EXHAUSTED,
    "IND_WITNESS_BUDGET_EXHAUSTED",
    Reason,
    Recoverable,
    "Deterministic witness enumeration exhausted its configured budget without a proof.",
    "This recoverable limitation does not make either query erroneous.",
    "Use a larger supported budget or investigate the comparison with additional evidence."
);
explain_content!(
    IND_PREDICATE_NORMAL_FORM_GAP,
    "IND_PREDICATE_NORMAL_FORM_GAP",
    Reason,
    Recoverable,
    "Predicate normalization cannot reach a proven canonical form.",
    "This recoverable limitation does not make either query erroneous.",
    "Use an equivalent supported predicate form or investigate a sound normalization rule."
);
explain_content!(
    IND_UNPARSEABLE,
    "IND_UNPARSEABLE",
    Reason,
    Recoverable,
    "An input or deep-parsed island did not parse far enough for a hard conclusion.",
    "This recoverable limitation does not turn an unknown result into a proof.",
    "Repair the parse error and run the comparison again."
);
explain_content!(
    MODEL_INCOMPLETE,
    "MODEL_INCOMPLETE",
    Reason,
    Recoverable,
    "Model coverage is insufficient for a hard conclusion.",
    "This recoverable limitation does not make the query erroneous.",
    "Load a more complete model or provide the missing source and property facts."
);
explain_content!(
    RELATION_ROW_TYPE_UNKNOWN,
    "RELATION_ROW_TYPE_UNKNOWN",
    Reason,
    Recoverable,
    "A relation row's column types are unavailable.",
    "This recoverable limitation does not make the relation expression erroneous.",
    "Load the relevant store schema or make the row type explicit."
);

/// Look up explain content by an exact registered diagnostic or reason identifier.
///
/// # Errors
///
/// Returns [`UnknownExplainIdentifier`] when `identifier` is not an exact,
/// case-sensitive member of either registry.
pub fn lookup_explanation(
    identifier: &str,
) -> Result<&'static ExplainContent, UnknownExplainIdentifier> {
    if let Ok(code) = identifier.parse::<DiagCode>() {
        return Ok(code.explanation());
    }
    if let Ok(reason) = identifier.parse::<ReasonCode>() {
        return Ok(reason.explanation());
    }
    Err(UnknownExplainIdentifier {
        value: identifier.to_owned(),
    })
}

/// Return explain content for one registered diagnostic code.
#[must_use]
pub(crate) fn diagnostic_explanation(code: DiagCode) -> &'static ExplainContent {
    match code {
        DiagCode::UnterminatedIsland => &PUR0101,
        DiagCode::BadToken => &PUR0102,
        DiagCode::MalformedSyntax => &PUR1200,
        DiagCode::ParenthesizedTuple => &PUR1201,
        DiagCode::IllegalBracketIndex => &PUR1202,
        DiagCode::MalformedMilestoningArguments => &PUR1204,
        DiagCode::UnknownJoinKind => &PUR1210,
        DiagCode::WrongMilestoningArity => &PUR2001,
        DiagCode::UnknownProperty => &PUR2002,
        DiagCode::UnknownSource => &PUR2101,
        DiagCode::ModelMergeConflict => &PUR9000,
        DiagCode::DuplicateModelDeclaration => &PUR9002,
        DiagCode::UnresolvedModelAssociation => &PUR9003,
    }
}

/// Return explain content for one registered reason code.
#[must_use]
pub(crate) fn reason_explanation(reason: ReasonCode) -> &'static ExplainContent {
    match reason {
        ReasonCode::IndWindow => &IND_WINDOW,
        ReasonCode::IndPareto => &IND_PARETO,
        ReasonCode::IndMultistepFiscal => &IND_MULTISTEP_FISCAL,
        ReasonCode::IndDivisionRatio => &IND_DIVISION_RATIO,
        ReasonCode::IndMilestoningAsof => &IND_MILESTONING_ASOF,
        ReasonCode::IndOrderUnderdetermined => &IND_ORDER_UNDERDETERMINED,
        ReasonCode::IndOpaquePredicate => &IND_OPAQUE_PREDICATE,
        ReasonCode::IndDifferentSources => &IND_DIFFERENT_SOURCES,
        ReasonCode::IndMissingRewrite => &IND_MISSING_REWRITE,
        ReasonCode::IndUnmodeledOp => &IND_UNMODELED_OP,
        ReasonCode::IndOpaqueFunctionInWitness => &IND_OPAQUE_FUNCTION_IN_WITNESS,
        ReasonCode::IndUnresolvedSchema => &IND_UNRESOLVED_SCHEMA,
        ReasonCode::IndWitnessBudgetExhausted => &IND_WITNESS_BUDGET_EXHAUSTED,
        ReasonCode::IndPredicateNormalFormGap => &IND_PREDICATE_NORMAL_FORM_GAP,
        ReasonCode::IndUnparseable => &IND_UNPARSEABLE,
        ReasonCode::ModelIncomplete => &MODEL_INCOMPLETE,
        ReasonCode::RelationRowTypeUnknown => &RELATION_ROW_TYPE_UNKNOWN,
    }
}

#[cfg(test)]
mod tests {
    use crate::{ALL_DIAG_CODES, ALL_REASON_CODES, DiagFamily, ReasonBucket};

    use super::*;

    fn diagnostic_classification(family: DiagFamily) -> ExplainClassification {
        match family {
            DiagFamily::Lexer => ExplainClassification::Lexer,
            DiagFamily::Parser => ExplainClassification::Parser,
            DiagFamily::Lint => ExplainClassification::Lint,
            DiagFamily::Tool => ExplainClassification::Tool,
        }
    }

    fn reason_classification(bucket: ReasonBucket) -> ExplainClassification {
        match bucket {
            ReasonBucket::Fundamental => ExplainClassification::Fundamental,
            ReasonBucket::Recoverable => ExplainClassification::Recoverable,
        }
    }

    #[test]
    fn every_registered_identifier_has_complete_structured_content() {
        for &code in ALL_DIAG_CODES {
            let content = code.explanation();
            assert_eq!(content.identifier, code.as_str());
            assert_eq!(content.kind, ExplainKind::Diagnostic);
            assert_eq!(
                content.classification,
                diagnostic_classification(code.family())
            );
            assert_complete(content);
        }
        for &reason in ALL_REASON_CODES {
            let content = reason.explanation();
            assert_eq!(content.identifier, reason.id());
            assert_eq!(content.kind, ExplainKind::Reason);
            assert_eq!(
                content.classification,
                reason_classification(reason.bucket())
            );
            assert_complete(content);
        }
    }

    fn assert_complete(content: &ExplainContent) {
        assert!(
            !content.meaning.is_empty(),
            "{} has no meaning",
            content.identifier
        );
        assert!(
            !content.limit.is_empty(),
            "{} has no limit",
            content.identifier
        );
        assert!(
            !content.remedy.is_empty(),
            "{} has no remedy",
            content.identifier
        );
        assert_eq!(
            content.documentation_url,
            format!("{EXPLAIN_DOCUMENTATION_URL_ROOT}/{}.md", content.identifier)
        );
    }

    #[test]
    fn lookup_is_exact_for_every_registered_identifier() {
        for &code in ALL_DIAG_CODES {
            assert_eq!(lookup_explanation(code.as_str()), Ok(code.explanation()));
        }
        for &reason in ALL_REASON_CODES {
            assert_eq!(lookup_explanation(reason.id()), Ok(reason.explanation()));
        }
    }

    #[test]
    fn lookup_rejects_unknown_identifiers_with_a_typed_stable_error() {
        for identifier in ["", "pur2001", "PUR9999", "ind_window", "IND_UNKNOWN"] {
            let error = lookup_explanation(identifier).expect_err("unknown identifier must fail");
            assert_eq!(error.value(), identifier);
            assert_eq!(
                error.to_string(),
                format!(
                    "unknown explain identifier {identifier:?}; expected an exact registered diagnostic or reason identifier; see {EXPLAIN_INDEX_URL}"
                )
            );
        }
    }

    #[test]
    fn explain_kind_as_str_is_exact_for_every_variant() {
        // Each arm asserted individually (not round-tripped through a shared
        // fixture) so a mutant that swaps or replaces one arm's literal fails
        // here even though the other arm's literal happens to stay correct.
        assert_eq!(ExplainKind::Diagnostic.as_str(), "diagnostic");
        assert_eq!(ExplainKind::Reason.as_str(), "reason");
    }

    #[test]
    fn explain_classification_as_str_is_exact_for_every_variant() {
        assert_eq!(ExplainClassification::Lexer.as_str(), "lexer");
        assert_eq!(ExplainClassification::Parser.as_str(), "parser");
        assert_eq!(ExplainClassification::Lint.as_str(), "lint");
        assert_eq!(ExplainClassification::Tool.as_str(), "tool");
        assert_eq!(ExplainClassification::Fundamental.as_str(), "fundamental");
        assert_eq!(ExplainClassification::Recoverable.as_str(), "recoverable");
    }

    #[test]
    fn serialized_content_has_the_renderer_neutral_contract() {
        let content = DiagCode::WrongMilestoningArity.explanation();
        let value = serde_json::to_value(content).expect("serialize explain content");
        assert_eq!(value["identifier"], "PUR2001");
        assert_eq!(value["kind"], "diagnostic");
        assert_eq!(value["classification"], "lint");
        assert_eq!(
            value["documentation_url"],
            format!("{EXPLAIN_DOCUMENTATION_URL_ROOT}/PUR2001.md")
        );
        for field in ["meaning", "limit", "remedy"] {
            assert!(value[field].as_str().is_some_and(|text| !text.is_empty()));
        }
    }
}
