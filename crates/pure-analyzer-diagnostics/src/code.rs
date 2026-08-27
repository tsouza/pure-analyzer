//! Stable diagnostic identifiers and their semantic families.

use std::{fmt, str::FromStr};

/// Every diagnostic identifier currently defined by the analyzer design.
///
/// The serialized form is always `PUR` followed by four decimal digits. New
/// analyzer rules extend this enum in the same change that implements the rule,
/// keeping misspelled or unregistered identifiers from compiling.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub enum DiagCode {
    /// `PUR0101`: an island reaches end-of-input before its matching terminator.
    #[serde(rename = "PUR0101")]
    UnterminatedIsland,
    /// `PUR0102`: the lexer encounters an unrecognized token.
    #[serde(rename = "PUR0102")]
    BadToken,
    /// `PUR1200`: source tokens do not form a complete supported declaration or expression.
    #[serde(rename = "PUR1200")]
    MalformedSyntax,
    /// `PUR1201`: a parenthesized value tuple is admitted only for targeted validation.
    #[serde(rename = "PUR1201")]
    ParenthesizedTuple,
    /// `PUR1202`: a bracket index is not a string or integer literal.
    #[serde(rename = "PUR1202")]
    IllegalBracketIndex,
    /// `PUR1204`: milestoning parentheses have an invalid surface shape.
    #[serde(rename = "PUR1204")]
    MalformedMilestoningArguments,
    /// `PUR1210`: a join kind is outside the supported closed enum.
    #[serde(rename = "PUR1210")]
    UnknownJoinKind,
    /// `PUR2001`: navigation supplies the wrong number of milestoning dates.
    #[serde(rename = "PUR2001")]
    WrongMilestoningArity,
    /// `PUR2002`: a closed-world source class has no such property.
    #[serde(rename = "PUR2002")]
    UnknownProperty,
    /// `PUR2003`: statically known multiplicity and usage cardinality disagree.
    #[serde(rename = "PUR2003")]
    CardinalityMisuse,
    /// `PUR2100`: the model loader synthesized a generated qualified property.
    #[serde(rename = "PUR2100")]
    DerivedQualifiedProperty,
    /// `PUR2101`: local inference cannot determine the navigation source.
    #[serde(rename = "PUR2101")]
    UnknownSource,
    /// `PUR3001`: an equivalence or difference verdict.
    #[serde(rename = "PUR3001")]
    EquivalenceVerdict,
    /// `PUR9000`: later model input replaces an earlier definition.
    #[serde(rename = "PUR9000")]
    ModelMergeConflict,
    /// `PUR9001`: a command requires a model but none was supplied.
    #[serde(rename = "PUR9001")]
    ModelRequired,
}

/// The complete diagnostic registry in stable numeric order.
pub const ALL_DIAG_CODES: &[DiagCode] = &[
    DiagCode::UnterminatedIsland,
    DiagCode::BadToken,
    DiagCode::MalformedSyntax,
    DiagCode::ParenthesizedTuple,
    DiagCode::IllegalBracketIndex,
    DiagCode::MalformedMilestoningArguments,
    DiagCode::UnknownJoinKind,
    DiagCode::WrongMilestoningArity,
    DiagCode::UnknownProperty,
    DiagCode::CardinalityMisuse,
    DiagCode::DerivedQualifiedProperty,
    DiagCode::UnknownSource,
    DiagCode::EquivalenceVerdict,
    DiagCode::ModelMergeConflict,
    DiagCode::ModelRequired,
];

impl DiagCode {
    /// The stable wire identifier.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UnterminatedIsland => "PUR0101",
            Self::BadToken => "PUR0102",
            Self::MalformedSyntax => "PUR1200",
            Self::ParenthesizedTuple => "PUR1201",
            Self::IllegalBracketIndex => "PUR1202",
            Self::MalformedMilestoningArguments => "PUR1204",
            Self::UnknownJoinKind => "PUR1210",
            Self::WrongMilestoningArity => "PUR2001",
            Self::UnknownProperty => "PUR2002",
            Self::CardinalityMisuse => "PUR2003",
            Self::DerivedQualifiedProperty => "PUR2100",
            Self::UnknownSource => "PUR2101",
            Self::EquivalenceVerdict => "PUR3001",
            Self::ModelMergeConflict => "PUR9000",
            Self::ModelRequired => "PUR9001",
        }
    }

    /// The namespace family encoded by the first decimal digit.
    #[must_use]
    pub const fn family(self) -> DiagFamily {
        match self {
            Self::UnterminatedIsland | Self::BadToken => DiagFamily::Lexer,
            Self::MalformedSyntax
            | Self::ParenthesizedTuple
            | Self::IllegalBracketIndex
            | Self::MalformedMilestoningArguments
            | Self::UnknownJoinKind => DiagFamily::Parser,
            Self::WrongMilestoningArity
            | Self::UnknownProperty
            | Self::CardinalityMisuse
            | Self::DerivedQualifiedProperty
            | Self::UnknownSource => DiagFamily::Lint,
            Self::EquivalenceVerdict => DiagFamily::Equivalence,
            Self::ModelMergeConflict | Self::ModelRequired => DiagFamily::Tool,
        }
    }
}

impl fmt::Display for DiagCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for DiagCode {
    type Err = UnknownDiagCode;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        ALL_DIAG_CODES
            .iter()
            .copied()
            .find(|code| code.as_str() == value)
            .ok_or_else(|| UnknownDiagCode {
                value: value.to_owned(),
            })
    }
}

/// The subsystem family encoded by a [`DiagCode`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DiagFamily {
    /// Lexer and island diagnostics (`PUR0xxx`).
    Lexer,
    /// Parser and grammar-validation diagnostics (`PUR1xxx`).
    Parser,
    /// Resolution and lint diagnostics (`PUR2xxx`).
    Lint,
    /// Equivalence and difference verdicts (`PUR3xxx`).
    Equivalence,
    /// Tool, configuration, and model diagnostics (`PUR9xxx`).
    Tool,
}

/// An unregistered diagnostic identifier supplied by a user-facing boundary.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown diagnostic code {value:?}")]
pub struct UnknownDiagCode {
    value: String,
}

impl UnknownDiagCode {
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
    fn every_code_has_a_unique_four_digit_identifier() {
        let identifiers: BTreeSet<_> = ALL_DIAG_CODES.iter().map(|code| code.as_str()).collect();
        assert_eq!(identifiers.len(), ALL_DIAG_CODES.len());
        for identifier in identifiers {
            assert_eq!(identifier.len(), 7);
            assert!(identifier.starts_with("PUR"));
            assert!(identifier[3..].bytes().all(|byte| byte.is_ascii_digit()));
        }
    }

    #[test]
    fn registry_order_identifiers_and_families_are_locked() {
        let expected = [
            (DiagCode::UnterminatedIsland, "PUR0101", DiagFamily::Lexer),
            (DiagCode::BadToken, "PUR0102", DiagFamily::Lexer),
            (DiagCode::MalformedSyntax, "PUR1200", DiagFamily::Parser),
            (DiagCode::ParenthesizedTuple, "PUR1201", DiagFamily::Parser),
            (DiagCode::IllegalBracketIndex, "PUR1202", DiagFamily::Parser),
            (
                DiagCode::MalformedMilestoningArguments,
                "PUR1204",
                DiagFamily::Parser,
            ),
            (DiagCode::UnknownJoinKind, "PUR1210", DiagFamily::Parser),
            (DiagCode::WrongMilestoningArity, "PUR2001", DiagFamily::Lint),
            (DiagCode::UnknownProperty, "PUR2002", DiagFamily::Lint),
            (DiagCode::CardinalityMisuse, "PUR2003", DiagFamily::Lint),
            (
                DiagCode::DerivedQualifiedProperty,
                "PUR2100",
                DiagFamily::Lint,
            ),
            (DiagCode::UnknownSource, "PUR2101", DiagFamily::Lint),
            (
                DiagCode::EquivalenceVerdict,
                "PUR3001",
                DiagFamily::Equivalence,
            ),
            (DiagCode::ModelMergeConflict, "PUR9000", DiagFamily::Tool),
            (DiagCode::ModelRequired, "PUR9001", DiagFamily::Tool),
        ];
        assert_eq!(ALL_DIAG_CODES.len(), expected.len());
        for (&code, &(variant, identifier, family)) in ALL_DIAG_CODES.iter().zip(&expected) {
            assert_eq!(code, variant);
            assert_eq!(code.as_str(), identifier);
            assert_eq!(code.family(), family);
        }
    }

    #[test]
    fn display_parse_and_json_round_trip_the_complete_registry() {
        for &expected in ALL_DIAG_CODES {
            let rendered = expected.to_string();
            assert_eq!(rendered.parse::<DiagCode>(), Ok(expected));
            let json = serde_json::to_string(&expected).expect("serialize code");
            let parsed: DiagCode = serde_json::from_str(&json).expect("deserialize code");
            assert_eq!(parsed, expected);
            assert_eq!(json, format!("\"{rendered}\""));
        }
    }

    #[test]
    fn parsing_is_exact_and_rejects_legacy_tool_codes() {
        for value in ["", "pur2001", "PUR200", "PUR900", "PUR901", "PUR9999"] {
            let error = value
                .parse::<DiagCode>()
                .expect_err("unknown code must fail");
            assert_eq!(error.value(), value);
            assert_eq!(
                error.to_string(),
                format!("unknown diagnostic code {value:?}")
            );
        }
    }

    #[test]
    fn numeric_prefix_matches_the_declared_family() {
        for &code in ALL_DIAG_CODES {
            let expected_digit = match code.family() {
                DiagFamily::Lexer => b'0',
                DiagFamily::Parser => b'1',
                DiagFamily::Lint => b'2',
                DiagFamily::Equivalence => b'3',
                DiagFamily::Tool => b'9',
            };
            assert_eq!(code.as_str().as_bytes()[3], expected_digit);
        }
    }
}
