//! `eq`/`diff` verdicts and the `Indecisive` reason-code taxonomy.

use std::{fmt, str::FromStr};

use serde::ser::{SerializeStruct, Serializer};

/// The outcome of `eq`/`diff`: sound, incomplete, three-valued.
///
/// This type has no fourth
/// variant that could be mistaken for a commitment, and every producer must
/// map uncertainty to [`Verdict::Indecisive`] rather than guessing.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "verdict", rename_all = "snake_case")]
pub enum Verdict {
    /// Proven equal by canonical-normal-form match.
    Equivalent,
    /// Proven distinct by a concrete, model-legal witness.
    NotEquivalent {
        /// A rendered, paste-and-run-in-the-engine Pure literal exhibiting
        /// the divergence.
        witness: String,
    },
    /// Neither proof succeeded. The owning [`crate::Diagnostic`]'s `reason`
    /// field carries why.
    Indecisive,
}

/// Whether an inconclusive result is a deliberate semantic limit or tractable backlog.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasonBucket {
    /// Out of scope without a fundamentally different semantic technique.
    Fundamental,
    /// A limitation that prevents a committed verdict for the current input.
    Recoverable,
}

/// A stable explanation for an inconclusive verdict or downgraded finding.
///
/// Each variant owns exactly one identifier, bucket, and explanation. Callers
/// cannot construct misspelled reasons or reclassify a fundamental limit as
/// recoverable (or vice versa).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ReasonCode {
    /// Window and OLAP-frame equivalence is outside the sound core.
    IndWindow,
    /// Pareto and top-per-group equivalence depends on unmodeled tie semantics.
    IndPareto,
    /// Multi-step fiscal accumulation equivalence is outside the sound core.
    IndMultistepFiscal,
    /// Division and ratio equivalence is outside the sound core.
    IndDivisionRatio,
    /// Bitemporal as-of equivalence is outside the sound core.
    IndMilestoningAsof,
    /// Output depends on an order that the model cannot prove total.
    IndOrderUnderdetermined,
    /// A predicate falls outside the sound interpreted whitelist.
    IndOpaquePredicate,
    /// The two queries read different named sources.
    IndDifferentSources,
    /// The normalizer lacks a known sound rewrite.
    IndMissingRewrite,
    /// A relational operator lacks a sound semantic model.
    IndUnmodeledOp,
    /// Witness evaluation encounters an uninterpreted function.
    IndOpaqueFunctionInWitness,
    /// Available model facts cannot prove the required schema property.
    IndUnresolvedSchema,
    /// Deterministic witness enumeration exhausted its configured budget.
    IndWitnessBudgetExhausted,
    /// Predicate normalization cannot reach a proven canonical form.
    IndPredicateNormalFormGap,
    /// An input or deep-parsed island did not parse.
    IndUnparseable,
    /// Model coverage is insufficient for a hard conclusion.
    ModelIncomplete,
    /// A relation row's column types are unavailable.
    RelationRowTypeUnknown,
}

/// The complete reason registry in stable taxonomy order.
pub const ALL_REASON_CODES: &[ReasonCode] = &[
    ReasonCode::IndWindow,
    ReasonCode::IndPareto,
    ReasonCode::IndMultistepFiscal,
    ReasonCode::IndDivisionRatio,
    ReasonCode::IndMilestoningAsof,
    ReasonCode::IndOrderUnderdetermined,
    ReasonCode::IndOpaquePredicate,
    ReasonCode::IndDifferentSources,
    ReasonCode::IndMissingRewrite,
    ReasonCode::IndUnmodeledOp,
    ReasonCode::IndOpaqueFunctionInWitness,
    ReasonCode::IndUnresolvedSchema,
    ReasonCode::IndWitnessBudgetExhausted,
    ReasonCode::IndPredicateNormalFormGap,
    ReasonCode::IndUnparseable,
    ReasonCode::ModelIncomplete,
    ReasonCode::RelationRowTypeUnknown,
];

impl ReasonCode {
    /// The stable wire identifier.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::IndWindow => "IND_WINDOW",
            Self::IndPareto => "IND_PARETO",
            Self::IndMultistepFiscal => "IND_MULTISTEP_FISCAL",
            Self::IndDivisionRatio => "IND_DIVISION_RATIO",
            Self::IndMilestoningAsof => "IND_MILESTONING_ASOF",
            Self::IndOrderUnderdetermined => "IND_ORDER_UNDERDETERMINED",
            Self::IndOpaquePredicate => "IND_OPAQUE_PREDICATE",
            Self::IndDifferentSources => "IND_DIFFERENT_SOURCES",
            Self::IndMissingRewrite => "IND_MISSING_REWRITE",
            Self::IndUnmodeledOp => "IND_UNMODELED_OP",
            Self::IndOpaqueFunctionInWitness => "IND_OPAQUE_FUNCTION_IN_WITNESS",
            Self::IndUnresolvedSchema => "IND_UNRESOLVED_SCHEMA",
            Self::IndWitnessBudgetExhausted => "IND_WITNESS_BUDGET_EXHAUSTED",
            Self::IndPredicateNormalFormGap => "IND_PREDICATE_NORMAL_FORM_GAP",
            Self::IndUnparseable => "IND_UNPARSEABLE",
            Self::ModelIncomplete => "MODEL_INCOMPLETE",
            Self::RelationRowTypeUnknown => "RELATION_ROW_TYPE_UNKNOWN",
        }
    }

    /// The immutable taxonomy bucket.
    #[must_use]
    pub const fn bucket(self) -> ReasonBucket {
        match self {
            Self::IndWindow
            | Self::IndPareto
            | Self::IndMultistepFiscal
            | Self::IndDivisionRatio
            | Self::IndMilestoningAsof
            | Self::IndOrderUnderdetermined
            | Self::IndOpaquePredicate
            | Self::IndDifferentSources => ReasonBucket::Fundamental,
            Self::IndMissingRewrite
            | Self::IndUnmodeledOp
            | Self::IndOpaqueFunctionInWitness
            | Self::IndUnresolvedSchema
            | Self::IndWitnessBudgetExhausted
            | Self::IndPredicateNormalFormGap
            | Self::IndUnparseable
            | Self::ModelIncomplete
            | Self::RelationRowTypeUnknown => ReasonBucket::Recoverable,
        }
    }

    /// A one-line explanation suitable for structured and `explain` output.
    #[must_use]
    pub const fn blurb(self) -> &'static str {
        match self {
            Self::IndWindow => "window/OLAP frame equivalence is outside the sound core",
            Self::IndPareto => "pareto/top-per-group equivalence needs modeled tie semantics",
            Self::IndMultistepFiscal => {
                "multi-step fiscal accumulation equivalence is outside the sound core"
            }
            Self::IndDivisionRatio => "division and ratio equivalence is outside the sound core",
            Self::IndMilestoningAsof => "bitemporal as-of equivalence is outside the sound core",
            Self::IndOrderUnderdetermined => "the available facts do not prove a total order",
            Self::IndOpaquePredicate => "the predicate is outside the sound interpreted whitelist",
            Self::IndDifferentSources => "the queries read different named sources",
            Self::IndMissingRewrite => "the normalizer lacks a sound rewrite for this difference",
            Self::IndUnmodeledOp => "a relational operator has no sound semantic model",
            Self::IndOpaqueFunctionInWitness => {
                "witness evaluation encountered an uninterpreted function"
            }
            Self::IndUnresolvedSchema => "the available model facts do not resolve the schema",
            Self::IndWitnessBudgetExhausted => {
                "deterministic witness enumeration exhausted its budget"
            }
            Self::IndPredicateNormalFormGap => {
                "the predicate did not reach a proven canonical form"
            }
            Self::IndUnparseable => "an input or deep-parsed island did not parse",
            Self::ModelIncomplete => "model coverage is insufficient for a hard conclusion",
            Self::RelationRowTypeUnknown => "the relation row's column types are unavailable",
        }
    }

    /// Structured, renderer-neutral explain content for this reason.
    #[must_use]
    pub fn explanation(self) -> &'static crate::ExplainContent {
        crate::explain::reason_explanation(self)
    }
}

impl fmt::Display for ReasonCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.id())
    }
}

impl FromStr for ReasonCode {
    type Err = UnknownReasonCode;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        ALL_REASON_CODES
            .iter()
            .copied()
            .find(|reason| reason.id() == value)
            .ok_or_else(|| UnknownReasonCode {
                value: value.to_owned(),
            })
    }
}

impl serde::Serialize for ReasonCode {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("ReasonCode", 3)?;
        state.serialize_field("id", self.id())?;
        state.serialize_field("bucket", &self.bucket())?;
        state.serialize_field("blurb", self.blurb())?;
        state.end()
    }
}

/// An unregistered reason identifier supplied by a user-facing boundary.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown reason code {value:?}")]
pub struct UnknownReasonCode {
    value: String,
}

impl UnknownReasonCode {
    /// The rejected identifier.
    #[must_use]
    pub fn value(&self) -> &str {
        &self.value
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    #[test]
    fn not_equivalent_carries_its_witness() {
        let verdict = Verdict::NotEquivalent {
            witness: "#>{db.T}#->filter(x|$x.a == 1)".to_owned(),
        };
        assert!(
            matches!(verdict, Verdict::NotEquivalent { witness } if witness.contains("filter"))
        );
    }

    #[test]
    fn verdict_serializes_with_a_tag() {
        let json = serde_json::to_string(&Verdict::Equivalent).expect("serialize");
        assert!(json.contains("\"verdict\":\"equivalent\""));
    }

    #[test]
    fn every_reason_has_a_unique_exact_identifier() {
        let identifiers: BTreeSet<_> = ALL_REASON_CODES.iter().map(|reason| reason.id()).collect();
        assert_eq!(identifiers.len(), ALL_REASON_CODES.len());
        for &reason in ALL_REASON_CODES {
            assert!(!reason.blurb().is_empty());
        }
    }

    #[test]
    fn reason_display_and_parse_round_trip_the_complete_registry() {
        for &reason in ALL_REASON_CODES {
            let rendered = reason.to_string();
            assert_eq!(rendered, reason.id());
            assert_eq!(rendered.parse::<ReasonCode>(), Ok(reason));
        }
    }

    #[test]
    fn registry_identifiers_and_buckets_are_locked() {
        let expected = [
            (
                ReasonCode::IndWindow,
                "IND_WINDOW",
                ReasonBucket::Fundamental,
            ),
            (
                ReasonCode::IndPareto,
                "IND_PARETO",
                ReasonBucket::Fundamental,
            ),
            (
                ReasonCode::IndMultistepFiscal,
                "IND_MULTISTEP_FISCAL",
                ReasonBucket::Fundamental,
            ),
            (
                ReasonCode::IndDivisionRatio,
                "IND_DIVISION_RATIO",
                ReasonBucket::Fundamental,
            ),
            (
                ReasonCode::IndMilestoningAsof,
                "IND_MILESTONING_ASOF",
                ReasonBucket::Fundamental,
            ),
            (
                ReasonCode::IndOrderUnderdetermined,
                "IND_ORDER_UNDERDETERMINED",
                ReasonBucket::Fundamental,
            ),
            (
                ReasonCode::IndOpaquePredicate,
                "IND_OPAQUE_PREDICATE",
                ReasonBucket::Fundamental,
            ),
            (
                ReasonCode::IndDifferentSources,
                "IND_DIFFERENT_SOURCES",
                ReasonBucket::Fundamental,
            ),
            (
                ReasonCode::IndMissingRewrite,
                "IND_MISSING_REWRITE",
                ReasonBucket::Recoverable,
            ),
            (
                ReasonCode::IndUnmodeledOp,
                "IND_UNMODELED_OP",
                ReasonBucket::Recoverable,
            ),
            (
                ReasonCode::IndOpaqueFunctionInWitness,
                "IND_OPAQUE_FUNCTION_IN_WITNESS",
                ReasonBucket::Recoverable,
            ),
            (
                ReasonCode::IndUnresolvedSchema,
                "IND_UNRESOLVED_SCHEMA",
                ReasonBucket::Recoverable,
            ),
            (
                ReasonCode::IndWitnessBudgetExhausted,
                "IND_WITNESS_BUDGET_EXHAUSTED",
                ReasonBucket::Recoverable,
            ),
            (
                ReasonCode::IndPredicateNormalFormGap,
                "IND_PREDICATE_NORMAL_FORM_GAP",
                ReasonBucket::Recoverable,
            ),
            (
                ReasonCode::IndUnparseable,
                "IND_UNPARSEABLE",
                ReasonBucket::Recoverable,
            ),
            (
                ReasonCode::ModelIncomplete,
                "MODEL_INCOMPLETE",
                ReasonBucket::Recoverable,
            ),
            (
                ReasonCode::RelationRowTypeUnknown,
                "RELATION_ROW_TYPE_UNKNOWN",
                ReasonBucket::Recoverable,
            ),
        ];
        assert_eq!(ALL_REASON_CODES.len(), expected.len());
        for (&reason, &(variant, identifier, bucket)) in ALL_REASON_CODES.iter().zip(&expected) {
            assert_eq!(reason, variant);
            assert_eq!(reason.id(), identifier);
            assert_eq!(reason.bucket(), bucket);
        }
    }

    #[test]
    fn reason_serialization_preserves_the_object_shape() {
        let value = serde_json::to_value(ReasonCode::IndWindow).expect("serialize reason");
        assert_eq!(value["id"], "IND_WINDOW");
        assert_eq!(value["bucket"], "fundamental");
        assert_eq!(
            value["blurb"],
            "window/OLAP frame equivalence is outside the sound core"
        );
    }

    #[test]
    fn fundamental_and_recoverable_rosters_are_locked() {
        let fundamental = ALL_REASON_CODES
            .iter()
            .filter(|reason| reason.bucket() == ReasonBucket::Fundamental)
            .count();
        let recoverable = ALL_REASON_CODES.len() - fundamental;
        assert_eq!((fundamental, recoverable), (8, 9));
    }

    #[test]
    fn reason_parsing_is_exact() {
        for value in ["", "ind_window", "RelationRowTypeUnknown", "IND_UNKNOWN"] {
            let error = value
                .parse::<ReasonCode>()
                .expect_err("unknown reason must fail");
            assert_eq!(error.value(), value);
            assert_eq!(error.to_string(), format!("unknown reason code {value:?}"));
        }
    }
}
