//! Hermetic validation of frozen Legend semantic witnesses for guarded rewrites.

use std::collections::BTreeSet;

use serde_json::{Map, Value};

const CORPUS_VERSION: &str = "4.113.0";
const CORPUS_ROOT: &str = "legend-4.113.0";
const METADATA: &str = include_str!("../corpus/legend-4.113.0/metadata.json");
const CASES: &str = include_str!("../corpus/legend-4.113.0/cases.jsonl");
const SCHEMA_VERSION: u64 = 1;
const EQUAL: &str = "equal";
const DIFFERENT: &str = "different";
const INDECISIVE: &str = "indecisive";
const CANONICAL_FAMILIES: &[&str] = &["row-order", "bag-semantics", "three-valued-logic"];

fn object<'a>(value: &'a Value, path: &str) -> &'a Map<String, Value> {
    value
        .as_object()
        .unwrap_or_else(|| panic!("{path}: expected a JSON object"))
}

fn non_empty_string<'a>(object: &'a Map<String, Value>, field: &str, path: &str) -> &'a str {
    object
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| panic!("{path}: {field} must be a non-empty string"))
}

fn assert_exact_fields(object: &Map<String, Value>, expected: &[&str], path: &str) {
    let actual = object.keys().map(String::as_str).collect::<BTreeSet<_>>();
    let expected = expected.iter().copied().collect::<BTreeSet<_>>();
    assert_eq!(actual, expected, "{path}: unexpected corpus fields");
}

fn is_exact_version_pin(version: &str) -> bool {
    version.split('.').count() == 3
        && version
            .split('.')
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
}

fn assert_api_endpoint(object: &Map<String, Value>, field: &str) {
    let endpoint = non_empty_string(object, field, CORPUS_ROOT);
    assert!(
        endpoint
            .strip_prefix("/api/")
            .is_some_and(|path| !path.is_empty()),
        "{CORPUS_ROOT}: {field} must be an absolute API path"
    );
}

fn metadata() -> BTreeSet<String> {
    let value: Value = serde_json::from_str(METADATA)
        .unwrap_or_else(|error| panic!("{CORPUS_ROOT}: invalid metadata JSON: {error}"));
    let metadata = object(&value, CORPUS_ROOT);
    assert_exact_fields(
        metadata,
        &[
            "schema_version",
            "engine_version",
            "model_endpoint",
            "lambda_endpoint",
            "execution_endpoint",
            "required_families",
        ],
        CORPUS_ROOT,
    );
    assert_eq!(
        metadata.get("schema_version").and_then(Value::as_u64),
        Some(SCHEMA_VERSION),
        "{CORPUS_ROOT}: unsupported metadata schema version"
    );
    let version = non_empty_string(metadata, "engine_version", CORPUS_ROOT);
    assert!(
        is_exact_version_pin(version),
        "{CORPUS_ROOT}: engine version must be an exact x.y.z pin"
    );
    assert_eq!(
        version, CORPUS_VERSION,
        "{CORPUS_ROOT}: unexpected engine pin"
    );
    for field in ["model_endpoint", "lambda_endpoint", "execution_endpoint"] {
        assert_api_endpoint(metadata, field);
    }
    let families = metadata
        .get("required_families")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("{CORPUS_ROOT}: required_families must be an array"));
    assert!(
        !families.is_empty(),
        "{CORPUS_ROOT}: required_families must not be empty"
    );
    let families = families
        .iter()
        .map(|family| {
            family
                .as_str()
                .filter(|family| !family.trim().is_empty())
                .unwrap_or_else(|| {
                    panic!("{CORPUS_ROOT}: required_families must contain non-empty strings")
                })
                .to_owned()
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(
        families.len(),
        metadata
            .get("required_families")
            .and_then(Value::as_array)
            .map_or(0, Vec::len),
        "{CORPUS_ROOT}: required_families must not contain duplicates"
    );
    assert_eq!(
        families,
        CANONICAL_FAMILIES
            .iter()
            .map(|family| (*family).to_owned())
            .collect(),
        "{CORPUS_ROOT}: required_families must exactly list the canonical semantic classes"
    );
    families
}

fn decisive_side<'a>(value: &'a Value, path: &str) -> &'a Value {
    let side = object(value, path);
    assert_exact_fields(side, &["lambda", "result"], path);
    non_empty_string(side, "lambda", path);
    side.get("result")
        .unwrap_or_else(|| panic!("{path}: decisive evidence must include a result"))
}

fn probe_side(value: &Value, path: &str) {
    let side = object(value, path);
    assert_exact_fields(side, &["lambda"], path);
    non_empty_string(side, "lambda", path);
}

fn assert_case(
    value: &Value,
    path: &str,
    required_families: &BTreeSet<String>,
    ids: &mut BTreeSet<String>,
    families: &mut BTreeSet<String>,
    outcomes: &mut BTreeSet<String>,
) {
    let case = object(value, path);
    let outcome = non_empty_string(case, "outcome", path);
    match outcome {
        EQUAL | DIFFERENT => assert_exact_fields(
            case,
            &[
                "id",
                "family",
                "candidate",
                "model",
                "left",
                "right",
                "outcome",
            ],
            path,
        ),
        INDECISIVE => assert_exact_fields(
            case,
            &[
                "id",
                "family",
                "candidate",
                "model",
                "probe",
                "outcome",
                "reason",
            ],
            path,
        ),
        other => panic!("{path}: unsupported outcome {other:?}"),
    }

    let id = non_empty_string(case, "id", path);
    assert!(
        ids.insert(id.to_owned()),
        "{path}: duplicate case id {id:?}"
    );
    let family = non_empty_string(case, "family", path);
    assert!(
        required_families.contains(family),
        "{path}: case {id} uses unregistered semantic family {family:?}"
    );
    families.insert(family.to_owned());
    outcomes.insert(outcome.to_owned());
    for field in ["candidate", "model"] {
        non_empty_string(case, field, path);
    }

    match outcome {
        EQUAL | DIFFERENT => {
            let left = decisive_side(
                case.get("left")
                    .unwrap_or_else(|| panic!("{path}: decisive case lacks left evidence")),
                &format!("{path}:left"),
            );
            let right = decisive_side(
                case.get("right")
                    .unwrap_or_else(|| panic!("{path}: decisive case lacks right evidence")),
                &format!("{path}:right"),
            );
            if outcome == EQUAL {
                assert_eq!(
                    left, right,
                    "{path}: an equal case must store identical frozen results"
                );
            } else {
                assert_ne!(
                    left, right,
                    "{path}: a different case must store distinct frozen results"
                );
            }
        }
        INDECISIVE => {
            non_empty_string(case, "reason", path);
            let probe = object(
                case.get("probe")
                    .unwrap_or_else(|| panic!("{path}: indecisive case lacks its probe")),
                &format!("{path}:probe"),
            );
            assert_exact_fields(probe, &["left", "right"], &format!("{path}:probe"));
            probe_side(
                probe
                    .get("left")
                    .unwrap_or_else(|| panic!("{path}:probe lacks left source")),
                &format!("{path}:probe:left"),
            );
            probe_side(
                probe
                    .get("right")
                    .unwrap_or_else(|| panic!("{path}:probe lacks right source")),
                &format!("{path}:probe:right"),
            );
        }
        other => panic!("{path}: unsupported outcome {other:?}"),
    }
}

#[test]
fn frozen_legend_semantic_witnesses_preserve_their_declared_relationships() {
    let required_families = metadata();
    let mut ids = BTreeSet::new();
    let mut families = BTreeSet::new();
    let mut outcomes = BTreeSet::new();
    let mut case_count = 0;

    for (index, line) in CASES.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let path = format!("cases.jsonl:{}", index + 1);
        let value: Value = serde_json::from_str(line)
            .unwrap_or_else(|error| panic!("{path}: invalid case JSON: {error}"));
        assert_case(
            &value,
            &path,
            &required_families,
            &mut ids,
            &mut families,
            &mut outcomes,
        );
        case_count += 1;
    }

    assert!(
        case_count > 0,
        "cases.jsonl must contain semantic witnesses"
    );
    assert_eq!(
        families, required_families,
        "{CORPUS_ROOT}: every canonical semantic family needs at least one witness"
    );
    for outcome in [EQUAL, DIFFERENT, INDECISIVE] {
        assert!(
            outcomes.contains(outcome),
            "{CORPUS_ROOT}: corpus must retain at least one {outcome} outcome"
        );
    }
}
