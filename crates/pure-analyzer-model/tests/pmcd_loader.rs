//! Contract tests for PMCD ingestion and graph normalization.

#![allow(clippy::disallowed_methods)]

use std::path::PathBuf;

use pure_analyzer_model::{
    MODEL_MERGE_CONFLICT, ModelError, ModelErrorKind, PmcdDocument, Provenance, QpKind, Temporal,
    load_pmcd_documents, load_pmcd_files,
};
use serde_json::{Value, json};

const COMPLETE: &str = include_str!("fixtures/complete.pmcd.json");

fn load_values(values: &[(&str, Value)]) -> Result<pure_analyzer_model::ModelGraph, ModelError> {
    let encoded = values
        .iter()
        .map(|(_, value)| value.to_string())
        .collect::<Vec<_>>();
    let documents = values
        .iter()
        .zip(encoded.iter())
        .map(|((label, _), contents)| PmcdDocument::new(label, contents))
        .collect::<Vec<_>>();
    load_pmcd_documents(&documents)
}

fn document(elements: Vec<Value>) -> Value {
    json!({"_type": "data", "elements": elements})
}

fn class(package: &str, name: &str, properties: Vec<Value>) -> Value {
    json!({
        "_type": "class",
        "package": package,
        "name": name,
        "superTypes": [],
        "stereotypes": [],
        "properties": properties,
        "qualifiedProperties": []
    })
}

fn property(name: &str, target: &str, lower: u32, upper: Option<u32>) -> Value {
    json!({
        "name": name,
        "genericType": {"rawType": target},
        "multiplicity": {"lowerBound": lower, "upperBound": upper}
    })
}

fn association(package: &str, name: &str, ends: Vec<Value>) -> Value {
    json!({
        "_type": "association",
        "package": package,
        "name": name,
        "stereotypes": [],
        "properties": ends
    })
}

fn expect_element_error(error: ModelError) -> ModelErrorKind {
    match error {
        ModelError::InvalidElement { kind, .. } => *kind,
        other => panic!("expected element error, got {other:?}"),
    }
}

#[test]
fn complete_fixture_loads_all_class_facts_deterministically() {
    let graph = load_pmcd_documents(&[PmcdDocument::new("complete", COMPLETE)]).expect("load");
    let paths = graph
        .classes()
        .keys()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    assert_eq!(
        paths,
        [
            "model::Entity",
            "model::Product",
            "model::Quote",
            "model::Trade"
        ]
    );
    assert_eq!(graph.class_id("model::Entity").expect("id").index(), 0);
    assert_eq!(
        graph
            .class_by_id(graph.class_id("model::Trade").expect("id"))
            .expect("class")
            .path()
            .as_str(),
        "model::Trade"
    );

    let trade = graph.class("model::Trade").expect("trade");
    assert_eq!(trade.supertypes()[0].as_str(), "model::Entity");
    assert_eq!(trade.temporal(), Some(Temporal::Bitemporal));
    assert_eq!(trade.provenance(), Provenance::Pmcd);
    assert!(!trade.coverage_gap());
    assert_eq!(trade.source().index(), 0);
    assert_eq!(graph.sources()[0].label(), "complete");
    assert!(graph.diagnostics().is_empty());
}

#[test]
fn qualified_properties_preserve_signature_and_classification() {
    let graph = load_pmcd_documents(&[PmcdDocument::new("complete", COMPLETE)]).expect("load");
    let product = graph.class("model::Product").expect("product");
    let properties = product.qualified_properties();
    assert_eq!(properties["priceAfter"].kind(), QpKind::UserQualified);
    assert_eq!(
        properties["priceAfter"].signature().expect("signature")[0]
            .raw_type()
            .as_str(),
        "StrictDate"
    );
    assert_eq!(properties["quotes"].kind(), QpKind::MilestonedPoint);
    assert_eq!(properties["quotesAllVersions"].kind(), QpKind::AllVersions);
    assert_eq!(
        properties["quotesAllVersionsInRange"].kind(),
        QpKind::AllVersionsInRange
    );
    assert_eq!(properties["quotesEdge"].kind(), QpKind::EdgePoint);
    assert!(properties["quotes"].signature().is_none());
    assert!(properties["quotesEdge"].multiplicity().is_unbounded());
}

#[test]
fn association_ends_are_materialized_on_the_opposite_classes() {
    let graph = load_pmcd_documents(&[PmcdDocument::new("complete", COMPLETE)]).expect("load");
    let association = &graph.associations()[0];
    assert_eq!(association.path().as_str(), "model::Trade_Product");
    assert_eq!(association.temporal(), Some(Temporal::ProcessingTemporal));
    assert_eq!(association.end_a().owner().as_str(), "model::Product");
    assert_eq!(association.end_a().property().name().as_str(), "trades");
    assert_eq!(association.end_b().owner().as_str(), "model::Trade");
    assert_eq!(association.end_b().property().name().as_str(), "product");

    let trades = &graph.class("model::Product").expect("product").properties()["trades"];
    assert_eq!(trades.target().raw_type().as_str(), "model::Trade");
    assert!(trades.from_assoc());
    assert_eq!(
        trades.association().expect("association").as_str(),
        "model::Trade_Product"
    );
    let trade = graph.class("model::Trade").expect("trade");
    assert_eq!(
        trade.properties()["product"].target().raw_type().as_str(),
        "model::Product"
    );
    assert_eq!(
        trade.qualified_properties()["product"].kind(),
        QpKind::MilestonedPoint
    );
}

#[test]
fn file_loading_uses_the_same_ingestion_path() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/complete.pmcd.json");
    let graph = load_pmcd_files(&[path]).expect("load fixture");
    assert_eq!(graph.classes().len(), 4);
    assert_eq!(graph.associations().len(), 1);
}

#[test]
fn simplified_design_shape_and_unknown_elements_are_supported() {
    let pmcd = json!({
        "elements": [
            {"_type": "futureProtocolKind", "anything": [false, 42]},
            class("demo", "Base", vec![]),
            {
                "_type": "class",
                "package": "demo",
                "name": "Child",
                "superTypes": ["demo::Base"],
                "stereotypes": [],
                "properties": [property("value", "String", 0, Some(1))],
                "qualifiedProperties": []
            }
        ]
    });
    let graph = load_values(&[("simple", pmcd)]).expect("load");
    assert_eq!(
        graph.class("demo::Child").expect("child").supertypes()[0].as_str(),
        "demo::Base"
    );
}

#[test]
fn generic_type_arguments_are_retained_recursively() {
    let generic_property = json!({
        "name": "lookup",
        "genericType": {
            "rawType": "Map",
            "typeArguments": [
                {"rawType": "String"},
                {"rawType": "List", "typeArguments": [{"rawType": "model::Trade"}]}
            ]
        },
        "multiplicity": {"lowerBound": 1, "upperBound": 1}
    });
    let graph = load_values(&[(
        "generic",
        document(vec![class("model", "Holder", vec![generic_property])]),
    )])
    .expect("load");
    let target = graph.class("model::Holder").expect("holder").properties()["lookup"].target();
    assert_eq!(target.raw_type().as_str(), "Map");
    assert_eq!(target.type_arguments()[0].raw_type().as_str(), "String");
    assert_eq!(target.type_arguments()[1].raw_type().as_str(), "List");
    assert_eq!(
        target.type_arguments()[1].type_arguments()[0]
            .raw_type()
            .as_str(),
        "model::Trade"
    );
}

#[test]
fn an_empty_pmcd_is_a_valid_deterministic_graph() {
    let graph = load_values(&[("empty", document(vec![]))]).expect("load");
    assert!(graph.classes().is_empty());
    assert!(graph.associations().is_empty());
    assert!(graph.diagnostics().is_empty());
    assert_eq!(graph.sources().len(), 1);
}

#[test]
fn later_sources_win_and_emit_one_shared_diagnostic_per_collision() {
    let first = document(vec![class(
        "demo",
        "Trade",
        vec![property("value", "Integer", 1, Some(1))],
    )]);
    let second = document(vec![class(
        "demo",
        "Trade",
        vec![property("value", "String", 0, Some(1))],
    )]);
    let graph = load_values(&[("first", first), ("second", second)]).expect("merge");
    let class = graph.class("demo::Trade").expect("class");
    assert_eq!(
        class.properties()["value"].target().raw_type().as_str(),
        "String"
    );
    assert_eq!(class.source().index(), 1);
    assert_eq!(graph.diagnostics().len(), 1);
    let diagnostic = &graph.diagnostics()[0];
    assert_eq!(diagnostic.code, MODEL_MERGE_CONFLICT);
    assert_eq!(diagnostic.code.as_str(), "PUR9000");
    assert_eq!(diagnostic.primary.file.index(), 1);
    assert_eq!(diagnostic.secondary[0].file.index(), 0);
    assert!(diagnostic.message.contains("first"));
    assert!(diagnostic.message.contains("second"));
}

#[test]
fn association_can_reference_classes_from_another_source() {
    let classes = document(vec![
        class("demo", "Left", vec![]),
        class("demo", "Right", vec![]),
    ]);
    let associations = document(vec![association(
        "demo",
        "Link",
        vec![
            property("lefts", "demo::Left", 0, None),
            property("right", "demo::Right", 1, Some(1)),
        ],
    )]);
    let graph = load_values(&[("classes", classes), ("links", associations)]).expect("merge");
    assert!(
        graph
            .class("demo::Left")
            .expect("left")
            .properties()
            .contains_key("right")
    );
    assert!(
        graph
            .class("demo::Right")
            .expect("right")
            .properties()
            .contains_key("lefts")
    );
}

#[test]
fn malformed_document_envelopes_fail_closed() {
    let syntax = load_pmcd_documents(&[PmcdDocument::new("bad", "{")]).expect_err("syntax");
    assert!(matches!(syntax, ModelError::Json { .. }));

    for json in [
        "[]",
        r#"{"_type":"data"}"#,
        r#"{"_type":"wrong","elements":[]}"#,
        r#"{"_type":7,"elements":[]}"#,
    ] {
        let error = load_pmcd_documents(&[PmcdDocument::new("bad", json)]).expect_err("envelope");
        assert!(matches!(error, ModelError::InvalidDocument { .. }));
    }
}

#[test]
fn every_element_requires_a_string_discriminator() {
    for element in [json!(null), json!({}), json!({"_type": false})] {
        let error = load_values(&[("bad", document(vec![element]))]).expect_err("element");
        assert!(matches!(
            expect_element_error(error),
            ModelErrorKind::InvalidRecord(_)
        ));
    }
}

#[test]
fn malformed_relevant_records_are_not_treated_as_extensions() {
    let missing_name = json!({
        "_type": "class",
        "package": "demo",
        "properties": []
    });
    let error = load_values(&[("bad", document(vec![missing_name]))]).expect_err("missing field");
    assert!(matches!(
        expect_element_error(error),
        ModelErrorKind::InvalidRecord(_)
    ));
}

#[test]
fn protocol_omissions_and_short_temporal_profile_are_supported() {
    let compact = document(vec![
        json!({
            "_type": "class",
            "package": "demo",
            "name": "Left",
            "taggedValues": []
        }),
        json!({
            "_type": "class",
            "package": "demo",
            "name": "Right",
            "stereotypes": [{"profile": "temporal", "value": "bitemporal"}]
        }),
        json!({
            "_type": "association",
            "package": "demo",
            "name": "Link",
            "properties": [
                property("lefts", "demo::Left", 0, None),
                property("right", "demo::Right", 1, Some(1))
            ],
            "taggedValues": []
        }),
    ]);
    let graph = load_values(&[("compact-protocol", compact)]).expect("load compact protocol");
    assert_eq!(graph.classes().len(), 2);
    assert_eq!(graph.associations().len(), 1);
    assert_eq!(
        graph.class("demo::Right").expect("right").temporal(),
        Some(Temporal::Bitemporal)
    );
}

#[test]
fn duplicate_packageable_members_and_properties_are_rejected() {
    let duplicate_class = class("demo", "X", vec![]);
    let error = load_values(&[(
        "bad",
        document(vec![duplicate_class.clone(), duplicate_class]),
    )])
    .expect_err("duplicate element");
    assert!(matches!(
        expect_element_error(error),
        ModelErrorKind::DuplicateElement { .. }
    ));

    let duplicate_property = class(
        "demo",
        "X",
        vec![
            property("x", "String", 1, Some(1)),
            property("x", "Integer", 1, Some(1)),
        ],
    );
    let error = load_values(&[("bad", document(vec![duplicate_property]))])
        .expect_err("duplicate property");
    assert!(matches!(
        expect_element_error(error),
        ModelErrorKind::DuplicateProperty { .. }
    ));

    let qualified = json!({
        "name": "derive",
        "returnGenericType": {"rawType": "String"},
        "returnMultiplicity": {"lowerBound": 1, "upperBound": 1},
        "stereotypes": [],
        "parameters": []
    });
    let mut duplicate_qualified = class("demo", "X", vec![]);
    duplicate_qualified["qualifiedProperties"] = json!([qualified.clone(), qualified]);
    let error = load_values(&[("bad", document(vec![duplicate_qualified]))])
        .expect_err("duplicate qualified property");
    assert!(matches!(
        expect_element_error(error),
        ModelErrorKind::DuplicateQualifiedProperty { .. }
    ));
}

#[test]
fn invalid_multiplicity_and_temporal_stereotypes_are_rejected() {
    let invalid_mult = class("demo", "X", vec![property("x", "String", 2, Some(1))]);
    let error = load_values(&[("bad", document(vec![invalid_mult]))]).expect_err("multiplicity");
    assert!(matches!(
        expect_element_error(error),
        ModelErrorKind::InvalidMultiplicity(_)
    ));

    let mut unknown_temporal = class("demo", "X", vec![]);
    unknown_temporal["stereotypes"] = json!([{
        "profile": "meta::pure::profiles::temporal",
        "value": "futuretemporal"
    }]);
    let error =
        load_values(&[("bad", document(vec![unknown_temporal]))]).expect_err("unknown temporal");
    assert!(matches!(
        expect_element_error(error),
        ModelErrorKind::UnknownTemporalStereotype { .. }
    ));

    let mut multiple_temporal = class("demo", "X", vec![]);
    multiple_temporal["stereotypes"] = json!([
        {
            "profile": "meta::pure::profiles::temporal",
            "value": "businesstemporal"
        },
        {
            "profile": "meta::pure::profiles::temporal",
            "value": "processingtemporal"
        }
    ]);
    let error = load_values(&[("bad", document(vec![multiple_temporal]))])
        .expect_err("multiple temporal stereotypes");
    assert!(matches!(
        expect_element_error(error),
        ModelErrorKind::MultipleTemporalStereotypes { .. }
    ));
}

#[test]
fn associations_require_two_known_unoccupied_ends() {
    let one_end = association("demo", "Broken", vec![property("xs", "demo::X", 0, None)]);
    let error = load_values(&[("bad", document(vec![one_end]))]).expect_err("arity");
    assert!(matches!(
        expect_element_error(error),
        ModelErrorKind::AssociationArity { actual: 1, .. }
    ));

    let missing_owner = document(vec![
        class("demo", "X", vec![]),
        association(
            "demo",
            "Broken",
            vec![
                property("xs", "demo::X", 0, None),
                property("missing", "demo::Missing", 1, Some(1)),
            ],
        ),
    ]);
    let error = load_values(&[("bad", missing_owner)]).expect_err("owner");
    assert!(matches!(
        error,
        ModelError::InvalidMergedGraph {
            kind: ModelErrorKind::MissingAssociationOwner { .. },
            ..
        }
    ));

    let conflict = document(vec![
        class("demo", "X", vec![property("y", "demo::Y", 1, Some(1))]),
        class("demo", "Y", vec![]),
        association(
            "demo",
            "Broken",
            vec![
                property("xs", "demo::X", 0, None),
                property("y", "demo::Y", 1, Some(1)),
            ],
        ),
    ]);
    let error = load_values(&[("bad", conflict)]).expect_err("property conflict");
    assert!(matches!(
        error,
        ModelError::InvalidMergedGraph {
            kind: ModelErrorKind::AssociationPropertyConflict { .. },
            ..
        }
    ));
}

#[test]
fn missing_file_is_a_typed_read_error() {
    let missing = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/absent.pmcd.json");
    let error = load_pmcd_files(&[missing]).expect_err("missing file");
    assert!(matches!(error, ModelError::Read { .. }));
}
