//! Hermetic replay of frozen Legend grammar-parser verdicts.

use std::collections::BTreeSet;

use pure_analyzer_diagnostics::FileId;
use pure_analyzer_parser::parse_query;
use serde::Deserialize;

const CORPUS_VERSION: &str = "4.113.0";
const CORPUS_ROOT: &str = "legend-4.113.0";
const ACCEPT_CORPUS: &str = include_str!("../corpus/legend-4.113.0/accept.jsonl");
const REJECT_CORPUS: &str = include_str!("../corpus/legend-4.113.0/reject.jsonl");
const METADATA: &str = include_str!("../corpus/legend-4.113.0/metadata.json");
const FIXTURE_FILE_ID: u32 = 27;
const PARSE_OK: &str = "parse_ok";
const PARSE_FAIL: &str = "parse_fail";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CorpusMetadata {
    schema_version: u8,
    engine_version: String,
    grammar_endpoint: String,
    required_families: Vec<String>,
    provenance: String,
    update_policy: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Fixture {
    id: String,
    query: String,
    legend: String,
    endpoint: String,
    family: String,
    provenance: String,
}

fn metadata() -> CorpusMetadata {
    serde_json::from_str(METADATA).expect("parser differential metadata must be valid JSON")
}

fn fixtures(text: &str, path: &str) -> Vec<Fixture> {
    text.lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty())
        .map(|(index, line)| {
            serde_json::from_str(line)
                .unwrap_or_else(|error| panic!("{path}:{}: invalid fixture: {error}", index + 1))
        })
        .collect()
}

fn is_exact_version_pin(version: &str) -> bool {
    version.split('.').count() == 3
        && version
            .split('.')
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
}

fn assert_metadata(metadata: &CorpusMetadata) {
    assert_eq!(metadata.schema_version, 1);
    assert!(
        is_exact_version_pin(&metadata.engine_version),
        "{CORPUS_ROOT}: engine version must be an exact x.y.z pin"
    );
    assert_eq!(metadata.engine_version, CORPUS_VERSION);
    assert!(
        metadata
            .grammar_endpoint
            .strip_prefix("/api/")
            .is_some_and(|path| !path.is_empty()),
        "{CORPUS_ROOT}: grammar endpoint must be an absolute API path"
    );
    assert!(
        !metadata.required_families.is_empty(),
        "{CORPUS_ROOT}: required_families must not be empty"
    );
    let mut required_families = BTreeSet::new();
    for family in &metadata.required_families {
        assert!(
            !family.trim().is_empty(),
            "{CORPUS_ROOT}: required_families must not contain empty entries"
        );
        assert!(
            required_families.insert(family),
            "{CORPUS_ROOT}: duplicate required grammar family {family:?}"
        );
    }
    assert!(!metadata.provenance.trim().is_empty());
    assert!(!metadata.update_policy.trim().is_empty());
}

fn assert_fixture_schema(
    metadata: &CorpusMetadata,
    path: &str,
    expected_verdict: &str,
    fixture: &Fixture,
    ids: &mut BTreeSet<String>,
    families: &mut BTreeSet<String>,
) {
    assert!(
        !fixture.id.trim().is_empty(),
        "{path}: fixture id must not be empty"
    );
    assert!(
        !fixture.query.trim().is_empty(),
        "{path}: fixture {} has an empty query",
        fixture.id
    );
    assert!(
        ids.insert(fixture.id.clone()),
        "{path}: duplicate fixture id {:?}",
        fixture.id
    );
    assert_eq!(
        fixture.legend, expected_verdict,
        "{path}: fixture {} is in the wrong verdict corpus",
        fixture.id
    );
    assert!(
        !fixture.endpoint.trim().is_empty(),
        "{path}: fixture {} lacks an endpoint",
        fixture.id
    );
    assert_eq!(
        fixture.endpoint, metadata.grammar_endpoint,
        "{path}: fixture {} diverges from the pinned grammar endpoint",
        fixture.id
    );
    assert!(
        !fixture.provenance.trim().is_empty(),
        "{path}: fixture {} lacks provenance",
        fixture.id
    );
    assert!(
        !fixture.family.trim().is_empty(),
        "{path}: fixture {} lacks a grammar family",
        fixture.id
    );
    assert!(
        metadata.required_families.contains(&fixture.family),
        "{path}: fixture {} uses an unregistered grammar family {:?}",
        fixture.id,
        fixture.family
    );
    families.insert(fixture.family.clone());
}

fn assert_fixture(
    metadata: &CorpusMetadata,
    path: &str,
    expected_verdict: &str,
    fixture: Fixture,
    ids: &mut BTreeSet<String>,
    families: &mut BTreeSet<String>,
) {
    assert_fixture_schema(metadata, path, expected_verdict, &fixture, ids, families);

    let parsed =
        parse_query(&fixture.query, FileId::new(FIXTURE_FILE_ID)).unwrap_or_else(|error| {
            panic!(
                "{path}: fixture {} could not construct a lossless tree: {error}",
                fixture.id
            )
        });
    assert_eq!(
        parsed.green.text(),
        fixture.query,
        "{path}: fixture {} was not preserved losslessly",
        fixture.id
    );

    let local_verdict = if parsed.diagnostics.is_empty() {
        PARSE_OK
    } else {
        PARSE_FAIL
    };
    assert_eq!(
        local_verdict,
        fixture.legend,
        "Legend parser differential mismatch\ncase: {}\nengine: {} ({})\nlocal: {}\nsource:\n{}\ndiagnostics: {:#?}",
        fixture.id,
        fixture.legend,
        metadata.engine_version,
        local_verdict,
        fixture.query,
        parsed.diagnostics,
    );
}

fn assert_corpus(
    metadata: &CorpusMetadata,
    path: &str,
    text: &str,
    expected_verdict: &str,
    ids: &mut BTreeSet<String>,
    families: &mut BTreeSet<String>,
) {
    let fixtures = fixtures(text, path);
    assert!(
        !fixtures.is_empty(),
        "{path} must contain at least one fixture"
    );

    for fixture in fixtures {
        assert_fixture(metadata, path, expected_verdict, fixture, ids, families);
    }
}

fn assert_required_families(metadata: &CorpusMetadata, families: &BTreeSet<String>) {
    for family in &metadata.required_families {
        assert!(
            families.contains(family),
            "{CORPUS_ROOT}: required grammar family {family:?} has no fixture"
        );
    }
}

fn test_metadata() -> CorpusMetadata {
    CorpusMetadata {
        schema_version: 1,
        engine_version: CORPUS_VERSION.to_owned(),
        grammar_endpoint: "/api/pure/v1/grammar/grammarToJson/lambda".to_owned(),
        required_families: vec!["registered".to_owned()],
        provenance: "test provenance".to_owned(),
        update_policy: "test update policy".to_owned(),
    }
}

fn test_fixture() -> Fixture {
    Fixture {
        id: "test-case".to_owned(),
        query: "model::Person.all()".to_owned(),
        legend: PARSE_OK.to_owned(),
        endpoint: "/api/pure/v1/grammar/grammarToJson/lambda".to_owned(),
        family: "registered".to_owned(),
        provenance: "test provenance".to_owned(),
    }
}

#[test]
fn frozen_legend_grammar_verdicts_match_the_local_parser() {
    let metadata = metadata();
    assert_metadata(&metadata);

    let mut ids = BTreeSet::new();
    let mut families = BTreeSet::new();
    for (path, text, expected_verdict) in [
        ("accept.jsonl", ACCEPT_CORPUS, PARSE_OK),
        ("reject.jsonl", REJECT_CORPUS, PARSE_FAIL),
    ] {
        assert_corpus(
            &metadata,
            path,
            text,
            expected_verdict,
            &mut ids,
            &mut families,
        );
    }

    assert_required_families(&metadata, &families);
}

#[test]
#[should_panic(expected = "duplicate required grammar family")]
fn corpus_metadata_rejects_duplicate_required_families() {
    let mut metadata = test_metadata();
    metadata.required_families.push("registered".to_owned());

    assert_metadata(&metadata);
}

#[test]
#[should_panic(expected = "exact x.y.z pin")]
fn corpus_metadata_rejects_nonexact_engine_versions() {
    let mut metadata = test_metadata();
    metadata.engine_version = "4.113.0-dev".to_owned();

    assert_metadata(&metadata);
}

#[test]
#[should_panic(expected = "empty query")]
fn corpus_fixture_rejects_empty_queries() {
    let metadata = test_metadata();
    let mut fixture = test_fixture();
    fixture.query.clear();

    assert_fixture_schema(
        &metadata,
        "accept.jsonl",
        PARSE_OK,
        &fixture,
        &mut BTreeSet::new(),
        &mut BTreeSet::new(),
    );
}

#[test]
#[should_panic(expected = "fixture id must not be empty")]
fn corpus_fixture_rejects_empty_ids() {
    let metadata = test_metadata();
    let mut fixture = test_fixture();
    fixture.id.clear();

    assert_fixture_schema(
        &metadata,
        "accept.jsonl",
        PARSE_OK,
        &fixture,
        &mut BTreeSet::new(),
        &mut BTreeSet::new(),
    );
}

#[test]
#[should_panic(expected = "unregistered grammar family")]
fn corpus_fixture_rejects_unregistered_families() {
    let metadata = test_metadata();
    let mut fixture = test_fixture();
    fixture.family = "unregistered".to_owned();

    assert_fixture_schema(
        &metadata,
        "accept.jsonl",
        PARSE_OK,
        &fixture,
        &mut BTreeSet::new(),
        &mut BTreeSet::new(),
    );
}
