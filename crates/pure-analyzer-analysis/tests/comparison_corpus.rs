//! Hermetic replay of M4a outcomes, cross-checked against a bounded Legend
//! smoke oracle for each pair's declared column shape.
//!
//! The M4a `equivalent`/`not_equivalent`/`indecisive` verdicts themselves are
//! proven entirely by `compare_relational_queries` over the real lowered
//! `left`/`right` queries — that half needs no engine at all. What the frozen
//! `Evidence::result` fields add is a much narrower thing: confirmation that
//! each side's bounded [`Oracle`] (a closed, independently executable
//! expression Legend actually ran, pinned by
//! `scripts/analysis-comparison-corpus.mjs --refresh`) really does observe
//! what its `kind` claims, and `assert_oracle_matches_query` ties that oracle
//! back to the real lowered query's structural shape (its operator kind, and
//! for `ordered_columns`, its actual output column order). The frozen
//! `result`s are never a live oracle *of the `left`/`right` queries
//! themselves* — Legend cannot execute `test::Row.all()->project(~[...])`
//! from this corpus at all without a mapping the corpus does not provide
//! (confirmed live against the pinned 4.113.0 engine: `Row.all()` fails
//! compilation with "Error mapping not found for class Row"). So for
//! `ordered-project-schema-is-not-equivalent`, `assert_ne!(left.result,
//! right.result)` only proves the two frozen literal-list observations
//! `|['name', 'email']` and `|['email', 'name']` differ, which is true by
//! construction; it does not by itself prove anything about the `project`
//! queries. The link back to those queries is entirely
//! `assert_oracle_matches_query`'s job (a hermetic, engine-free structural
//! check against this crate's own lowering), not the engine's.

use std::collections::BTreeSet;

use pure_analyzer_analysis::{
    AnalysisInput, ComparisonOutcome, IrOrigin, OutputSchemaField, RelationOperator,
    RelationalOutcome, RelationalQuery, ScalarLiteral, ScalarOperator, compare_relational_queries,
    lower_m3_query,
};
use pure_analyzer_diagnostics::{FileId, ReasonCode};
use pure_analyzer_model::{ModelGraph, PureDocument, load_pure_documents};
use pure_analyzer_parser::parse_query;
use serde_json::{Map, Value};

const CORPUS_PATH: &str = "legend-4.113.0/comparison.jsonl";
const CASES: &str = include_str!("../corpus/legend-4.113.0/comparison.jsonl");
const EQUIVALENT: &str = "equivalent";
const NOT_EQUIVALENT: &str = "not_equivalent";
const INDECISIVE: &str = "indecisive";
const LEFT_FILE: u32 = 241;
const RIGHT_FILE: u32 = 242;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Outcome {
    Equivalent,
    NotEquivalent,
    Indecisive,
}

#[derive(Debug)]
struct Evidence<'source> {
    source: &'source str,
    oracle: Oracle,
    result: Option<&'source Value>,
}

/// A deliberately bounded executable observation derived from one M3 shape.
///
/// The Legend engine cannot execute the relation source used by the analyzer
/// corpus directly, so this records only a closed, independently executable
/// observation. `assert_oracle_matches_query` prevents it becoming unrelated
/// decorative evidence.
#[derive(Debug)]
enum Oracle {
    Scan { values: Vec<i64> },
    FilterTrue { values: Vec<i64> },
    OrderedColumns { columns: Vec<String> },
    LiteralFilter { values: Vec<String>, value: String },
}

impl Oracle {
    fn lambda(&self) -> String {
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

#[derive(Debug)]
struct ExpectedDifference {
    index: usize,
    field: OutputSchemaField,
}

#[derive(Debug)]
struct ComparisonCase<'source> {
    id: &'source str,
    model: &'source str,
    left: Evidence<'source>,
    right: Evidence<'source>,
    outcome: Outcome,
    difference: Option<ExpectedDifference>,
}

fn object<'source>(value: &'source Value, path: &str) -> &'source Map<String, Value> {
    value
        .as_object()
        .unwrap_or_else(|| panic!("{path}: expected a JSON object"))
}

fn non_empty_string<'source>(
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

fn required_value<'source>(
    object: &'source Map<String, Value>,
    field: &str,
    path: &str,
) -> &'source Value {
    object
        .get(field)
        .unwrap_or_else(|| panic!("{path}: missing {field}"))
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

fn assert_exact_fields(object: &Map<String, Value>, expected: &[&str], path: &str) {
    let actual = object.keys().map(String::as_str).collect::<BTreeSet<_>>();
    let expected = expected.iter().copied().collect::<BTreeSet<_>>();
    assert_eq!(actual, expected, "{path}: unexpected corpus fields");
}

fn parse_outcome(value: &str, path: &str) -> Outcome {
    match value {
        EQUIVALENT => Outcome::Equivalent,
        NOT_EQUIVALENT => Outcome::NotEquivalent,
        INDECISIVE => Outcome::Indecisive,
        other => panic!("{path}: unsupported outcome {other:?}"),
    }
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

fn parse_oracle(value: &Value, path: &str) -> Oracle {
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

fn parse_evidence<'source>(value: &'source Value, path: &str, decisive: bool) -> Evidence<'source> {
    let evidence = object(value, path);
    let expected = if decisive {
        &["source", "oracle", "lambda", "result"][..]
    } else {
        &["source", "oracle", "lambda"][..]
    };
    assert_exact_fields(evidence, expected, path);
    let source = non_empty_string(evidence, "source", path);
    let oracle = parse_oracle(
        required_value(evidence, "oracle", path),
        &format!("{path}:oracle"),
    );
    let lambda = non_empty_string(evidence, "lambda", path);
    assert_eq!(
        lambda,
        oracle.lambda(),
        "{path}: lambda must exactly render its bounded oracle"
    );
    let result = decisive.then(|| required_value(evidence, "result", path));
    Evidence {
        source,
        oracle,
        result,
    }
}

fn parse_difference(value: &Value, path: &str) -> ExpectedDifference {
    let difference = object(value, path);
    assert_exact_fields(difference, &["kind", "index", "field"], path);
    assert_eq!(
        non_empty_string(difference, "kind", path),
        "output_column",
        "{path}: only output_column is a committed M4a refutation"
    );
    let index = required_value(difference, "index", path)
        .as_u64()
        .and_then(|index| usize::try_from(index).ok())
        .unwrap_or_else(|| panic!("{path}: index must be a non-negative usize"));
    let field = match non_empty_string(difference, "field", path) {
        "name" => OutputSchemaField::Name,
        "type" => OutputSchemaField::Type,
        "multiplicity" => OutputSchemaField::Multiplicity,
        "nullability" => OutputSchemaField::Nullability,
        field => panic!("{path}: unsupported output-column field {field:?}"),
    };
    ExpectedDifference { index, field }
}

fn parse_case<'source>(value: &'source Value, path: &str) -> ComparisonCase<'source> {
    let case = object(value, path);
    let outcome = parse_outcome(non_empty_string(case, "outcome", path), path);
    match outcome {
        Outcome::Equivalent => {
            assert_exact_fields(case, &["id", "model", "left", "right", "outcome"], path)
        }
        Outcome::NotEquivalent => assert_exact_fields(
            case,
            &["id", "model", "left", "right", "outcome", "difference"],
            path,
        ),
        Outcome::Indecisive => assert_exact_fields(
            case,
            &["id", "model", "left", "right", "outcome", "reason"],
            path,
        ),
    }
    let id = non_empty_string(case, "id", path);
    let model = non_empty_string(case, "model", path);
    let decisive = outcome != Outcome::Indecisive;
    let left = parse_evidence(
        required_value(case, "left", path),
        &format!("{path}:left"),
        decisive,
    );
    let right = parse_evidence(
        required_value(case, "right", path),
        &format!("{path}:right"),
        decisive,
    );
    let difference = match outcome {
        Outcome::NotEquivalent => Some(parse_difference(
            required_value(case, "difference", path),
            &format!("{path}:difference"),
        )),
        Outcome::Equivalent | Outcome::Indecisive => None,
    };
    if outcome == Outcome::Indecisive {
        assert_eq!(
            non_empty_string(case, "reason", path),
            ReasonCode::IndMissingRewrite.id(),
            "{path}: this corpus records the M4a missing-rewrite boundary"
        );
    }
    ComparisonCase {
        id,
        model,
        left,
        right,
        outcome,
        difference,
    }
}

fn comparison_context(case: &ComparisonCase<'_>, side: &str, source: &str) -> String {
    format!(
        "{CORPUS_PATH}:{id}:{side}\nmodel:\n{model}\nsource:\n{source}",
        id = case.id,
        model = case.model,
    )
}

fn case_context(case: &ComparisonCase<'_>) -> String {
    format!(
        "{CORPUS_PATH}:{id}\nmodel:\n{model}\nleft source:\n{left}\nright source:\n{right}",
        id = case.id,
        model = case.model,
        left = case.left.source,
        right = case.right.source,
    )
}

fn assert_origin_has_query_and_model_provenance(
    context: &str,
    origin: &IrOrigin,
    allowed_files: &[FileId],
    model: &ModelGraph,
) {
    assert!(
        allowed_files.contains(&origin.source().file()),
        "{context}\ncomparison origin refers to unexpected query file {}",
        origin.source().file()
    );
    let model_source = model
        .sources()
        .first()
        .unwrap_or_else(|| panic!("{context}\nfixture model has no recorded source"));
    assert!(
        origin
            .model_origins()
            .iter()
            .any(|model_origin| model_origin.definition().source() == model_source.id()),
        "{context}\ncomparison origin lost model provenance from {}",
        model_source.label(),
    );
}

fn assert_oracle_matches_query(oracle: &Oracle, query: &RelationalQuery, context: &str) {
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

fn is_matching_navigation_literal_equality(operator: &ScalarOperator, value: &str) -> bool {
    let ScalarOperator::Equal { left, right } = operator else {
        return false;
    };
    (is_navigation(left.operator()) && is_string_literal(right.operator(), value))
        || (is_string_literal(left.operator(), value) && is_navigation(right.operator()))
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

fn load_model(case: &ComparisonCase<'_>) -> ModelGraph {
    let label = format!("comparison-corpus-{}.pure", case.id);
    load_pure_documents(&[PureDocument::new(&label, case.model)]).unwrap_or_else(|error| {
        panic!(
            "{CORPUS_PATH}:{}: model {label:?} must load:\n{}\n{error:#}",
            case.id, case.model
        )
    })
}

fn lower_side(
    case: &ComparisonCase<'_>,
    side: &str,
    evidence: &Evidence<'_>,
    model: &ModelGraph,
    file: FileId,
) -> Box<pure_analyzer_analysis::RelationalQuery> {
    let context = comparison_context(case, side, evidence.source);
    let parsed = parse_query(evidence.source, file)
        .unwrap_or_else(|error| panic!("{context}\nquery must parse: {error}"));
    assert!(
        parsed.diagnostics.is_empty(),
        "{context}\nquery parser diagnostics: {:#?}",
        parsed.diagnostics
    );
    let outcome = lower_m3_query(AnalysisInput::new(
        file,
        evidence.source,
        &parsed.green,
        &parsed.diagnostics,
        Some(model),
    ));
    let RelationalOutcome::Supported(query) = outcome else {
        panic!("{context}\nquery must lower to the supported M3 subset: {outcome:#?}");
    };
    assert_origin_has_query_and_model_provenance(&context, query.root().origin(), &[file], model);
    assert_oracle_matches_query(&evidence.oracle, &query, &context);
    query
}

fn assert_equivalent_outcome(
    case: &ComparisonCase<'_>,
    comparison: &ComparisonOutcome,
    context: &str,
) {
    assert_eq!(
        case.left.result, case.right.result,
        "{context}\nfrozen Legend results contradict an equivalent commitment"
    );
    assert_eq!(
        comparison,
        &ComparisonOutcome::Equivalent,
        "{context}\nM4a outcome contradicts the frozen equivalent evidence"
    );
}

fn assert_not_equivalent_outcome(
    case: &ComparisonCase<'_>,
    model: &ModelGraph,
    comparison: &ComparisonOutcome,
    context: &str,
) {
    assert_ne!(
        case.left.result, case.right.result,
        "{context}\nfrozen Legend results contradict a structural refutation"
    );
    let Some(expected) = &case.difference else {
        panic!("{context}\nstructural refutation lacks an expected difference");
    };
    let ComparisonOutcome::NotEquivalent(difference) = comparison else {
        panic!(
            "{context}\nM4a outcome contradicts the frozen structural refutation: {comparison:#?}"
        );
    };
    assert!(
        matches!(
            difference.kind(),
            pure_analyzer_analysis::StructuralDifferenceKind::OutputColumn { index, field }
                if *index == expected.index && *field == expected.field
        ),
        "{context}\ncommitted structural difference drifted: {difference:#?}"
    );
    let query_files = [FileId::new(LEFT_FILE), FileId::new(RIGHT_FILE)];
    assert_origin_has_query_and_model_provenance(
        context,
        difference.primary_origin(),
        &query_files,
        model,
    );
    assert_origin_has_query_and_model_provenance(
        context,
        difference.secondary_origin(),
        &query_files,
        model,
    );
    assert_eq!(
        BTreeSet::from([
            difference.primary_origin().source().file(),
            difference.secondary_origin().source().file(),
        ]),
        BTreeSet::from(query_files),
        "{context}\nstructural-difference origins must retain both query files"
    );
}

fn assert_indecisive_outcome(
    case: &ComparisonCase<'_>,
    model: &ModelGraph,
    comparison: &ComparisonOutcome,
    context: &str,
) {
    assert!(
        case.left.result.is_none() && case.right.result.is_none(),
        "{context}\nan indecisive case must remain result-free"
    );
    let ComparisonOutcome::Indecisive(indecision) = comparison else {
        panic!("{context}\nM4a must not commit beyond missing-rewrite evidence: {comparison:#?}");
    };
    assert_eq!(
        indecision.reason(),
        ReasonCode::IndMissingRewrite,
        "{context}\nindecision reason drifted"
    );
    assert_origin_has_query_and_model_provenance(
        context,
        indecision.origin(),
        &[FileId::new(LEFT_FILE), FileId::new(RIGHT_FILE)],
        model,
    );
}

fn assert_comparison_outcome(
    case: &ComparisonCase<'_>,
    model: &ModelGraph,
    comparison: &ComparisonOutcome,
) {
    let context = case_context(case);
    match case.outcome {
        Outcome::Equivalent => assert_equivalent_outcome(case, comparison, &context),
        Outcome::NotEquivalent => assert_not_equivalent_outcome(case, model, comparison, &context),
        Outcome::Indecisive => assert_indecisive_outcome(case, model, comparison, &context),
    }
}

/// Replay every M4a comparison witness and check its bounded Legend oracle.
///
/// This proves the real M4a outcome (via `compare_relational_queries`) for
/// every witness, and separately confirms that witness's bounded [`Oracle`]
/// structurally matches its lowered query and that the oracle's own frozen
/// Legend observation is internally consistent with the witness's declared
/// outcome. It does not prove the frozen Legend results are an independent
/// oracle *for the `left`/`right` queries themselves* — see the module doc.
#[test]
fn frozen_witnesses_and_their_bounded_legend_oracles_agree() {
    let mut ids = BTreeSet::new();
    let mut outcomes = BTreeSet::new();
    let mut count = 0;

    for (index, line) in CASES.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let path = format!("{CORPUS_PATH}:{}", index + 1);
        let value: Value = serde_json::from_str(line)
            .unwrap_or_else(|error| panic!("{path}: invalid comparison JSON: {error}"));
        let case = parse_case(&value, &path);
        assert!(
            ids.insert(case.id.to_owned()),
            "{path}: duplicate case id {:?}",
            case.id
        );
        let model = load_model(&case);
        let left = lower_side(&case, "left", &case.left, &model, FileId::new(LEFT_FILE));
        let right = lower_side(&case, "right", &case.right, &model, FileId::new(RIGHT_FILE));
        let comparison = compare_relational_queries(&left, &right);
        assert_comparison_outcome(&case, &model, &comparison);
        outcomes.insert(case.outcome);
        count += 1;
    }

    assert!(count > 0, "{CORPUS_PATH} must contain comparison evidence");
    assert_eq!(
        outcomes,
        BTreeSet::from([
            Outcome::Equivalent,
            Outcome::NotEquivalent,
            Outcome::Indecisive,
        ]),
        "{CORPUS_PATH} must retain equivalent, structural-refutation, and indecisive cases"
    );
}
