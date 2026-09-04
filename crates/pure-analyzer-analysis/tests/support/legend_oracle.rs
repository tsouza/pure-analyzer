//! Shared bounded-oracle JSONL parsing for the analyzer's frozen Legend
//! corpora (issue #245).
//!
//! Originally private to `comparison_corpus.rs`; factored out so
//! `canonical_corpus.rs` can pin canonical-emission fixtures against the same
//! four bounded, independently executable oracle shapes without duplicating
//! the JSON-field validation or the oracle-to-lowered-query structural check.
//! A bounded oracle is deliberately not a claim that Legend executed the
//! fixture's own `source` — see each corpus test's module doc for exactly
//! what the tie-back does and does not prove.

use std::collections::BTreeSet;

use pure_analyzer_analysis::{RelationOperator, RelationalQuery, ScalarLiteral, ScalarOperator};
use serde_json::{Map, Value};

/// A deliberately bounded executable observation derived from one M3 shape.
#[derive(Debug)]
pub enum Oracle {
    /// A bare class-extent scan, observed as an integer-identifier list.
    Scan {
        /// The frozen scan's observed identifiers.
        values: Vec<i64>,
    },
    /// A `->filter(x| true)` pass-through, observed as an unchanged list.
    FilterTrue {
        /// The frozen filter's observed identifiers.
        values: Vec<i64>,
    },
    /// A `->project(~[...])` output column order, observed as a name list.
    OrderedColumns {
        /// The frozen output column names, in declared order.
        columns: Vec<String>,
    },
    /// A `->filter(x| $x == <value>)` literal equality, observed as a
    /// single-element list.
    LiteralFilter {
        /// The candidate values the filter selects among.
        values: Vec<String>,
        /// The literal the filter selects.
        value: String,
    },
}

impl Oracle {
    /// Render the exact bounded Pure lambda this oracle claims to observe.
    #[must_use]
    pub fn lambda(&self) -> String {
        match self {
            Self::Scan { values } => format!("|[{}]", integer_values(values)),
            Self::FilterTrue { values } => {
                format!("|[{}]->filter(x: Integer[1]|true)", integer_values(values))
            }
            Self::OrderedColumns { columns } => {
                format!("|[{}]", string_values(columns))
            }
            Self::LiteralFilter { values, value } => format!(
                "|[{}]->filter(x: String[1]|$x == {})",
                string_values(values),
                pure_string(value)
            ),
        }
    }
}

fn integer_values(value: &[i64]) -> String {
    value
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

fn pure_string(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn string_values(values: &[String]) -> String {
    values
        .iter()
        .map(|value| pure_string(value))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Read a required JSON object field, panicking with `path` context.
pub fn object<'source>(value: &'source Value, path: &str) -> &'source Map<String, Value> {
    value
        .as_object()
        .unwrap_or_else(|| panic!("{path}: expected a JSON object"))
}

/// Read a required non-empty string field, panicking with `path` context.
pub fn non_empty_string<'source>(
    object: &'source Map<String, Value>,
    field: &str,
    path: &str,
) -> &'source str {
    object
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| panic!("{path}: {field} must be a non-empty string"))
}

/// Read a required field of any JSON type, panicking with `path` context.
pub fn required_value<'source>(
    object: &'source Map<String, Value>,
    field: &str,
    path: &str,
) -> &'source Value {
    object
        .get(field)
        .unwrap_or_else(|| panic!("{path}: missing {field}"))
}

/// Reject a corpus object carrying fields other than exactly `expected`.
pub fn assert_exact_fields(object: &Map<String, Value>, expected: &[&str], path: &str) {
    let actual = object.keys().map(String::as_str).collect::<BTreeSet<_>>();
    let expected = expected.iter().copied().collect::<BTreeSet<_>>();
    assert_eq!(actual, expected, "{path}: unexpected corpus fields");
}

fn parse_integer_values(value: &Value, path: &str) -> Vec<i64> {
    let values = value
        .as_array()
        .filter(|values| !values.is_empty())
        .unwrap_or_else(|| panic!("{path}: values must be a non-empty integer list"));
    values
        .iter()
        .enumerate()
        .map(|(index, value)| {
            value
                .as_i64()
                .unwrap_or_else(|| panic!("{path}:{index}: expected an integer"))
        })
        .collect()
}

fn parse_string_values(value: &Value, path: &str) -> Vec<String> {
    let values = value
        .as_array()
        .filter(|values| !values.is_empty())
        .unwrap_or_else(|| panic!("{path}: values must be a non-empty string list"));
    values
        .iter()
        .enumerate()
        .map(|(index, value)| {
            value
                .as_str()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| panic!("{path}:{index}: expected a non-empty string"))
                .to_owned()
        })
        .collect()
}

/// Parse and validate one bounded oracle object.
pub fn parse_oracle(value: &Value, path: &str) -> Oracle {
    let oracle = object(value, path);
    match non_empty_string(oracle, "kind", path) {
        "scan" => {
            assert_exact_fields(oracle, &["kind", "values"], path);
            Oracle::Scan {
                values: parse_integer_values(required_value(oracle, "values", path), path),
            }
        }
        "filter_true" => {
            assert_exact_fields(oracle, &["kind", "values"], path);
            Oracle::FilterTrue {
                values: parse_integer_values(required_value(oracle, "values", path), path),
            }
        }
        "ordered_columns" => {
            assert_exact_fields(oracle, &["kind", "columns"], path);
            let columns = parse_string_values(required_value(oracle, "columns", path), path);
            assert_eq!(
                columns.len(),
                columns.iter().collect::<BTreeSet<_>>().len(),
                "{path}: ordered column names must be unique"
            );
            Oracle::OrderedColumns { columns }
        }
        "literal_filter" => {
            assert_exact_fields(oracle, &["kind", "values", "value"], path);
            let values = parse_string_values(required_value(oracle, "values", path), path);
            let value = non_empty_string(oracle, "value", path).to_owned();
            assert!(
                values.contains(&value),
                "{path}: literal filter value must be one of its input values"
            );
            Oracle::LiteralFilter { values, value }
        }
        kind => panic!("{path}: unsupported bounded oracle {kind:?}"),
    }
}

fn is_navigation(operator: &ScalarOperator) -> bool {
    matches!(operator, ScalarOperator::Navigation { .. })
}

fn is_string_literal(operator: &ScalarOperator, value: &str) -> bool {
    matches!(
        operator,
        ScalarOperator::Literal(ScalarLiteral::String(candidate)) if candidate == value
    )
}

fn is_matching_navigation_literal_equality(operator: &ScalarOperator, value: &str) -> bool {
    let ScalarOperator::Equal { left, right } = operator else {
        return false;
    };
    (is_navigation(left.operator()) && is_string_literal(right.operator(), value))
        || (is_string_literal(left.operator(), value) && is_navigation(right.operator()))
}

/// Tie a bounded oracle back to the real lowered query's structural shape
/// (its root operator kind, and for `ordered_columns`, its actual output
/// column order). This is a hermetic, engine-free check; it does not by
/// itself prove the oracle's frozen result describes `query` — only that
/// `query`'s shape is consistent with the oracle's own claimed construct.
pub fn assert_oracle_matches_query(oracle: &Oracle, query: &RelationalQuery, context: &str) {
    match oracle {
        Oracle::Scan { .. } => assert!(
            matches!(query.root().operator(), RelationOperator::Scan(_)),
            "{context}\nscan oracle must correspond to a lowered scan: {:#?}",
            query.root().operator(),
        ),
        Oracle::FilterTrue { .. } => assert!(
            matches!(
                query.root().operator(),
                RelationOperator::Filter { predicate, .. }
                    if matches!(
                        predicate.operator(),
                        ScalarOperator::Literal(ScalarLiteral::Boolean(true))
                    )
            ),
            "{context}\ntrue-filter oracle must correspond to a lowered true filter: {:#?}",
            query.root().operator(),
        ),
        Oracle::OrderedColumns { columns } => {
            assert!(
                matches!(query.root().operator(), RelationOperator::Project { .. }),
                "{context}\nordered-columns oracle must correspond to a lowered project: {:#?}",
                query.root().operator(),
            );
            let actual = query
                .output()
                .columns()
                .iter()
                .map(|column| column.name().as_str())
                .collect::<Vec<_>>();
            let expected = columns.iter().map(String::as_str).collect::<Vec<_>>();
            assert_eq!(
                actual, expected,
                "{context}\nordered-columns oracle must preserve lowered output aliases and order"
            );
        }
        Oracle::LiteralFilter { value, .. } => assert!(
            matches!(
                query.root().operator(),
                RelationOperator::Filter { predicate, .. }
                    if is_matching_navigation_literal_equality(predicate.operator(), value)
            ),
            "{context}\nliteral-filter oracle must correspond to a lowered navigation equality literal: {:#?}",
            query.root().operator(),
        ),
    }
}
