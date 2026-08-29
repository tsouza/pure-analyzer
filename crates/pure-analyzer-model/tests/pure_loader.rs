//! Contracts for resilient Pure Domain source ingestion.

#![allow(clippy::disallowed_methods)]

use pure_analyzer_diagnostics::{DiagCode, Severity, TextRange};
use pure_analyzer_model::{
    MODEL_MERGE_CONFLICT, ModelDocument, PmcdDocument, Provenance, PureDocument, QpKind, Temporal,
    load_model_documents, load_pure_documents, load_pure_files,
};
use serde_json::json;
use std::path::PathBuf;

fn pure(source: &str) -> pure_analyzer_model::ModelGraph {
    load_pure_documents(&[PureDocument::new("memory:model.pure", source)]).expect("load Pure")
}

fn exact_span(source: &str, declaration: &str) -> TextRange {
    let start = source.find(declaration).expect("declaration occurs once");
    let end = start + declaration.len();
    TextRange::new(
        u32::try_from(start).expect("source fits TextRange").into(),
        u32::try_from(end).expect("source fits TextRange").into(),
    )
}

fn empty_pmcd_class(name: &str) -> String {
    json!({
        "_type": "data",
        "elements": [{
            "_type": "class",
            "package": "demo",
            "name": name,
            "superTypes": [],
            "stereotypes": [],
            "properties": [],
            "qualifiedProperties": []
        }]
    })
    .to_string()
}

const COLLIDING_PURE_ASSOCIATION: &str = r#"
Association demo::APure
{
 others: demo::Left[*];
 shared: demo::Right[1];
}
"#;

const SECOND_COLLIDING_PURE_ASSOCIATION: &str = r#"
Association demo::BPure
{
 moreOthers: demo::Left[*];
 shared: demo::Right[1];
}
"#;

const REPLACING_PURE_ASSOCIATION: &str = r#"
Association demo::ZTrusted
{
  pureShared: demo::Right[1];
  pureLefts: demo::Left[*];
}
"#;

const UNMATERIALIZABLE_REPLACING_PURE_ASSOCIATION: &str = r#"
Association demo::ZTrusted
{
  pureShared: demo::Right[1];
  missing: demo::Missing[1];
}
"#;

fn trusted_pmcd_association() -> String {
    json!({
        "_type": "data",
        "elements": [{
            "_type": "association",
            "package": "demo",
            "name": "ZTrusted",
            "stereotypes": [],
            "properties": [
                {
                    "name": "lefts",
                    "genericType": {"rawType": "demo::Left"},
                    "multiplicity": {"lowerBound": 0, "upperBound": null}
                },
                {
                    "name": "shared",
                    "genericType": {"rawType": "demo::Right"},
                    "multiplicity": {"lowerBound": 1, "upperBound": 1}
                }
            ]
        }]
    })
    .to_string()
}

fn second_pmcd_association() -> String {
    json!({
        "_type": "data",
        "elements": [{
            "_type": "association",
            "package": "demo",
            "name": "Second",
            "stereotypes": [],
            "properties": [
                {
                    "name": "lefts",
                    "genericType": {"rawType": "demo::Left"},
                    "multiplicity": {"lowerBound": 0, "upperBound": null}
                },
                {
                    "name": "shared",
                    "genericType": {"rawType": "demo::Right"},
                    "multiplicity": {"lowerBound": 1, "upperBound": 1}
                }
            ]
        }]
    })
    .to_string()
}

fn mixed_association_collision_graph(pure_first: bool) -> pure_analyzer_model::ModelGraph {
    let left = empty_pmcd_class("Left");
    let right = empty_pmcd_class("Right");
    let trusted = trusted_pmcd_association();
    let pure = ModelDocument::Pure(PureDocument::new(
        "colliding.pure",
        COLLIDING_PURE_ASSOCIATION,
    ));
    let left = ModelDocument::Pmcd(PmcdDocument::new("left.pmcd.json", &left));
    let right = ModelDocument::Pmcd(PmcdDocument::new("right.pmcd.json", &right));
    let trusted = ModelDocument::Pmcd(PmcdDocument::new("trusted.pmcd.json", &trusted));
    let documents = if pure_first {
        [pure, left, right, trusted]
    } else {
        [left, right, trusted, pure]
    };
    load_model_documents(&documents).expect("Pure uncertainty must not invalidate PMCD")
}

fn same_path_association_replacement_graph(pure_first: bool) -> pure_analyzer_model::ModelGraph {
    let left = empty_pmcd_class("Left");
    let right = empty_pmcd_class("Right");
    let pmcd = trusted_pmcd_association();
    let pure = ModelDocument::Pure(PureDocument::new(
        "replacement.pure",
        REPLACING_PURE_ASSOCIATION,
    ));
    let left = ModelDocument::Pmcd(PmcdDocument::new("left.pmcd.json", &left));
    let right = ModelDocument::Pmcd(PmcdDocument::new("right.pmcd.json", &right));
    let pmcd = ModelDocument::Pmcd(PmcdDocument::new("trusted.pmcd.json", &pmcd));
    let documents = if pure_first {
        [pure, left, right, pmcd]
    } else {
        [left, right, pmcd, pure]
    };

    load_model_documents(&documents).expect("same-path associations must merge")
}

fn assert_trusted_pmcd_association(graph: &pure_analyzer_model::ModelGraph, trusted_source: u32) {
    assert_eq!(graph.associations().len(), 1);
    let trusted = graph.associations().first().expect("trusted association");
    assert_eq!(trusted.path().as_str(), "demo::ZTrusted");
    assert_eq!(trusted.provenance(), Provenance::Pmcd);
    assert_eq!(trusted.source().index(), trusted_source);

    let left = graph.class("demo::Left").expect("left");
    assert!(left.coverage_gap());
    assert_eq!(
        left.properties()["shared"]
            .association()
            .expect("PMCD association provenance")
            .as_str(),
        "demo::ZTrusted"
    );
    let right = graph.class("demo::Right").expect("right");
    assert!(right.coverage_gap());
    assert!(!right.properties().contains_key("others"));
}

fn assert_unresolved_pure_collision(graph: &pure_analyzer_model::ModelGraph, pure_source: u32) {
    assert_eq!(graph.diagnostics().len(), 1);
    let diagnostic = &graph.diagnostics()[0];
    assert_eq!(diagnostic.code, DiagCode::UnresolvedModelAssociation);
    assert_eq!(diagnostic.severity, Severity::Error);
    assert_eq!(diagnostic.primary.file.index(), pure_source);
    assert!(diagnostic.message.contains("demo::APure"));
    assert!(diagnostic.message.contains("demo::Left.shared"));
    assert!(
        graph
            .diagnostics()
            .iter()
            .all(|diagnostic| diagnostic.code != MODEL_MERGE_CONFLICT)
    );
}

#[test]
fn pure_document_accessors_preserve_borrowed_input() {
    let document = PureDocument::new("memory:accessors.pure", "Class demo::Input {}");

    assert_eq!(document.label(), "memory:accessors.pure");
    assert_eq!(document.source(), "Class demo::Input {}");
}

#[test]
fn pure_domain_class_facts_lower_with_source_provenance() {
    let graph = pure(
        r#"
Class <<temporal.businesstemporal>> demo::Entity
{
}

Class <<temporal.processingtemporal>> demo::Processing
{
}

Class demo::Order extends demo::Entity
{
  id: Integer[1];
  tags: List<String>[*];
  priceAfter(asOf: StrictDate[1]): Decimal[0..1] { $this.id; };
}

Association <<temporal.processingtemporal>> demo::Order_Entity
{
  order: demo::Order[1];
  entities: demo::Entity[*];
}
"#,
    );

    assert_pure_order_class_facts(&graph);
    assert_pure_entity_association_facts(&graph);
    assert_pure_association_end_facts(&graph);
    assert_pure_association_metadata(&graph);
}

fn assert_pure_order_class_facts(graph: &pure_analyzer_model::ModelGraph) {
    let order = graph.class("demo::Order").expect("order");
    assert_eq!(order.supertypes()[0].as_str(), "demo::Entity");
    assert_eq!(order.provenance(), Provenance::PureFile);
    assert!(!order.coverage_gap());
    assert_eq!(order.properties()["tags"].multiplicity().lower(), 0);
    assert!(order.properties()["tags"].multiplicity().is_unbounded());
    let price_after = &order.qualified_properties()["priceAfter"];
    assert_eq!(price_after.multiplicity().lower(), 0);
    assert_eq!(price_after.multiplicity().upper(), Some(1));
    assert_eq!(
        price_after.signature().expect("user signature")[0]
            .raw_type()
            .as_str(),
        "StrictDate"
    );
}

fn assert_pure_entity_association_facts(graph: &pure_analyzer_model::ModelGraph) {
    let order = graph.class("demo::Order").expect("order");
    let entity = graph.class("demo::Entity").expect("entity");
    assert_eq!(entity.temporal(), Some(Temporal::BusinessTemporal));
    let processing = graph.class("demo::Processing").expect("processing");
    assert_eq!(processing.temporal(), Some(Temporal::ProcessingTemporal));
    assert!(entity.properties()["order"].from_assoc());
    assert!(order.properties()["entities"].from_assoc());
}

fn assert_pure_association_end_facts(graph: &pure_analyzer_model::ModelGraph) {
    let association = &graph.associations()[0];
    assert_eq!(association.end_a().owner().as_str(), "demo::Entity");
    assert_eq!(association.end_a().property().name().as_str(), "order");
    assert_eq!(
        association.end_a().property().target().raw_type().as_str(),
        "demo::Order"
    );
    assert_eq!(association.end_a().property().multiplicity().lower(), 1);
    assert_eq!(association.end_b().owner().as_str(), "demo::Order");
    assert_eq!(association.end_b().property().name().as_str(), "entities");
    assert_eq!(
        association.end_b().property().target().raw_type().as_str(),
        "demo::Entity"
    );
    assert!(association.end_b().property().multiplicity().is_unbounded());
}

fn assert_pure_association_metadata(graph: &pure_analyzer_model::ModelGraph) {
    let association = &graph.associations()[0];
    assert_eq!(association.temporal(), Some(Temporal::ProcessingTemporal));
    assert_eq!(association.provenance(), Provenance::PureFile);
    assert_eq!(association.source().index(), 0);
    assert_eq!(graph.sources()[0].provenance(), Provenance::PureFile);
}

#[test]
fn pure_file_loading_uses_the_same_ingestion_path() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/complete.pure");
    let graph = load_pure_files(&[path]).expect("load Pure fixture");

    let class = graph.class("demo::Fixture").expect("fixture class");
    assert_eq!(class.provenance(), Provenance::PureFile);
    assert!(class.properties().contains_key("value"));
}

#[test]
fn conflicting_temporal_stereotypes_leave_the_class_open_world() {
    let graph = pure(
        r#"
Class <<temporal.bitemporal, temporal.businesstemporal>> demo::Conflicting
{
  value: String[1];
}
"#,
    );

    let class = graph.class("demo::Conflicting").expect("conflicting class");
    assert_eq!(class.temporal(), None);
    assert!(class.coverage_gap());
}

#[test]
fn bitemporal_stereotypes_lower_to_the_bitemporal_variant() {
    let graph = pure(
        r#"
Class <<temporal.bitemporal>> demo::Bitemporal
{
  value: String[1];
}
"#,
    );

    let class = graph.class("demo::Bitemporal").expect("bitemporal class");
    assert_eq!(class.temporal(), Some(Temporal::Bitemporal));
    assert!(!class.coverage_gap());
}

#[test]
fn leading_root_paths_lower_to_canonical_model_qnames() {
    let graph = pure(
        r#"
Class ::demo::Thing extends ::demo::Base, other::Stamped
{
  value: Map<::demo::Key, List<::demo::Value>>[0..*];
}
"#,
    );

    let thing = graph.class("demo::Thing").expect("canonical class path");
    assert!(!thing.coverage_gap());
    assert_eq!(thing.supertypes()[0].as_str(), "demo::Base");
    assert_eq!(thing.supertypes()[1].as_str(), "other::Stamped");

    let value = &thing.properties()["value"];
    assert_eq!(value.target().raw_type().as_str(), "Map");
    assert_eq!(
        value.target().type_arguments()[0].raw_type().as_str(),
        "demo::Key"
    );
    let list = &value.target().type_arguments()[1];
    assert_eq!(list.raw_type().as_str(), "List");
    assert_eq!(list.type_arguments()[0].raw_type().as_str(), "demo::Value");
}

#[test]
fn underscores_are_valid_at_the_start_and_inside_pure_names() {
    let graph = pure(
        r#"
Class demo::_Hidden_Class
{
  _value_name: String[1];
}
"#,
    );

    let class = graph
        .class("demo::_Hidden_Class")
        .expect("underscore class");
    assert!(class.properties().contains_key("_value_name"));
}

#[test]
fn malformed_multiple_root_separators_do_not_lower_model_facts() {
    let graph = pure(
        r#"
Class ::::demo::Malformed
{
  value: String[1];
}
"#,
    );

    assert!(graph.class("demo::Malformed").is_none());
    assert!(
        graph
            .diagnostics()
            .iter()
            .any(|diagnostic| diagnostic.code == DiagCode::MalformedSyntax)
    );
}

#[test]
fn only_double_angle_applications_confirm_temporal_facts() {
    let graph = pure(
        r#"
Class {meta::pure::profiles::temporal.bitemporal = 'tag value'} demo::Tagged
{
  value: String[1];
}

Class {meta::pure::profiles::temporal.bitemporal} demo::Malformed
{
  value: String[1];
}
"#,
    );

    let tagged = graph.class("demo::Tagged").expect("tagged class");
    assert_eq!(tagged.temporal(), None);
    assert!(!tagged.coverage_gap());

    let malformed = graph.class("demo::Malformed").expect("malformed class");
    assert_eq!(malformed.temporal(), None);
    assert!(malformed.coverage_gap());
    assert!(
        graph
            .diagnostics()
            .iter()
            .any(|diagnostic| diagnostic.code == DiagCode::MalformedSyntax)
    );
}

#[test]
fn confirmed_pure_declarations_retain_exact_source_spans() {
    let left_declaration = "Class demo::Left\n{\n  value: String[1];\n  query(): String[1] {};\n}";
    let right_declaration = "Class demo::Right\n{\n}";
    let association_declaration =
        "Association demo::Links\n{\n  left: demo::Left[1];\n  right: demo::Right[*];\n}";
    let source = format!("{left_declaration}\n{right_declaration}\n{association_declaration}");
    let graph = pure(&source);

    let left = graph.class("demo::Left").expect("left");
    assert_eq!(
        left.declaration_span(),
        Some(exact_span(&source, left_declaration))
    );
    assert_eq!(
        left.properties()["value"].declaration_span(),
        Some(exact_span(&source, "value: String[1];"))
    );
    assert_eq!(
        left.qualified_properties()["query"].declaration_span(),
        Some(exact_span(&source, "query(): String[1] {};"))
    );

    let association = graph.associations().first().expect("association");
    assert_eq!(
        association.declaration_span(),
        Some(exact_span(&source, association_declaration))
    );
    assert_eq!(
        association.end_a().declaration_span(),
        Some(exact_span(&source, "left: demo::Left[1];"))
    );
    assert_eq!(
        association.end_a().property().declaration_span(),
        association.end_a().declaration_span()
    );
    assert_eq!(
        association.end_b().declaration_span(),
        Some(exact_span(&source, "right: demo::Right[*];"))
    );
    assert_eq!(
        graph.class("demo::Right").expect("right").properties()["left"].declaration_span(),
        association.end_a().declaration_span(),
        "the materialized navigation end retains its source declaration range"
    );
    assert_eq!(
        left.properties()["right"].declaration_span(),
        association.end_b().declaration_span(),
        "both materialized navigation ends retain their source declaration ranges"
    );
}

#[test]
fn pmcd_declarations_remain_spanless() {
    let pmcd = json!({
        "_type": "data",
        "elements": [
            {
                "_type": "class",
                "package": "demo",
                "name": "Left",
                "superTypes": [],
                "stereotypes": [],
                "properties": [{
                    "name": "value",
                    "genericType": {"rawType": "String"},
                    "multiplicity": {"lowerBound": 1, "upperBound": 1}
                }],
                "qualifiedProperties": [{
                    "name": "query",
                    "returnGenericType": {"rawType": "String"},
                    "returnMultiplicity": {"lowerBound": 1, "upperBound": 1},
                    "stereotypes": [],
                    "parameters": []
                }]
            },
            {
                "_type": "class",
                "package": "demo",
                "name": "Right",
                "superTypes": [],
                "stereotypes": [],
                "properties": [],
                "qualifiedProperties": []
            },
            {
                "_type": "association",
                "package": "demo",
                "name": "Links",
                "stereotypes": [],
                "properties": [
                    {
                        "name": "left",
                        "genericType": {"rawType": "demo::Left"},
                        "multiplicity": {"lowerBound": 1, "upperBound": 1}
                    },
                    {
                        "name": "right",
                        "genericType": {"rawType": "demo::Right"},
                        "multiplicity": {"lowerBound": 0, "upperBound": null}
                    }
                ]
            }
        ]
    })
    .to_string();
    let graph =
        load_model_documents(&[ModelDocument::Pmcd(PmcdDocument::new("model.json", &pmcd))])
            .expect("load PMCD");

    let left = graph.class("demo::Left").expect("left");
    assert_eq!(left.declaration_span(), None);
    assert_eq!(left.properties()["value"].declaration_span(), None);
    assert_eq!(
        left.qualified_properties()["query"].declaration_span(),
        None
    );
    let association = graph.associations().first().expect("association");
    assert_eq!(association.declaration_span(), None);
    assert_eq!(association.end_a().declaration_span(), None);
    assert_eq!(association.end_b().declaration_span(), None);
    assert_eq!(association.end_a().property().declaration_span(), None);
}

#[test]
fn confirmed_generated_milestoning_qps_are_classified_from_stereotypes() {
    let source = r#"
Class <<temporal.businesstemporal>> demo::Holder
{
  <<milestoning.generatedmilestoningproperty>>
  quotes(): demo::Quote[0..1] {};
  <<milestoning.generatedmilestoningproperty>>
  quotesAllVersions(): demo::Quote[*] {};
  <<milestoning.generatedmilestoningproperty>>
  quotesAllVersionsInRange(): demo::Quote[*] {};
  <<milestoning.generatedmilestoningproperty>>
  quotesEdge(): demo::Quote[*] {};
  userAllVersions(): demo::Quote[*] {};
}
"#;
    let pure_graph = pure(source);
    let pure_properties = pure_graph
        .class("demo::Holder")
        .expect("Pure holder")
        .qualified_properties();
    assert_eq!(pure_properties["quotes"].kind(), QpKind::MilestonedPoint);
    assert_eq!(
        pure_properties["quotesAllVersions"].kind(),
        QpKind::AllVersions
    );
    assert_eq!(
        pure_properties["quotesAllVersionsInRange"].kind(),
        QpKind::AllVersionsInRange
    );
    assert_eq!(pure_properties["quotesEdge"].kind(), QpKind::EdgePoint);
    assert_eq!(
        pure_properties["userAllVersions"].kind(),
        QpKind::UserQualified,
        "name suffixes alone must not synthesize generated navigation"
    );
    assert!(pure_properties["quotes"].signature().is_none());
}

#[test]
fn malformed_or_opaque_pure_regions_preserve_only_confirmed_facts() {
    let source = r#"
Enum demo::Future { enabled }
Class demo::Partial
{
    bad: Foo;
    good: String[1];
    query(value: String[1]): String[1] {};
}
"#;
    let graph = pure(source);

    let partial = graph.class("demo::Partial").expect("partial class");
    assert!(partial.coverage_gap());
    assert_eq!(
        partial.declaration_span(),
        Some(exact_span(
            source,
            "Class demo::Partial\n{\n    bad: Foo;\n    good: String[1];\n    query(value: String[1]): String[1] {};\n}"
        )),
        "a confirmed class path retains its declaration span despite an open-world coverage gap"
    );
    assert!(partial.properties().contains_key("good"));
    assert!(partial.qualified_properties().contains_key("query"));
    assert_eq!(
        partial.properties()["good"].declaration_span(),
        Some(exact_span(source, "good: String[1];"))
    );
    assert!(
        !partial.properties().contains_key("bad"),
        "a malformed property must not become a confirmed graph fact"
    );
    assert!(
        graph
            .diagnostics()
            .iter()
            .any(|diagnostic| diagnostic.code.as_str() == "PUR1200")
    );
}

#[test]
fn malformed_but_structurally_complete_property_is_not_lowered() {
    let graph = pure(
        r#"
Class demo::Partial
{
    broken: String[1]
}
"#,
    );

    let partial = graph.class("demo::Partial").expect("partial class");
    assert!(partial.coverage_gap());
    assert!(
        !partial.properties().contains_key("broken"),
        "a property without its required terminator must remain unconfirmed"
    );
}

#[test]
fn unsupported_later_pure_source_opens_prior_unrelated_pmcd_classes() {
    let existing = empty_pmcd_class("Existing");
    let graph = load_model_documents(&[
        ModelDocument::Pmcd(PmcdDocument::new("existing.pmcd.json", &existing)),
        ModelDocument::Pure(PureDocument::new(
            "unsupported.pure",
            r#"
Enum demo::Unsupported { enabled }
"#,
        )),
    ])
    .expect("unsupported Pure must preserve prior PMCD facts");

    let existing = graph.class("demo::Existing").expect("prior class");
    assert!(
        existing.coverage_gap(),
        "an unrelated unsupported later Pure source leaves the complete model open-world"
    );
}

#[test]
fn incomplete_associations_open_same_source_classes() {
    let graph = pure(
        r#"
Class demo::Left
{
}
Class demo::Right
{
}
Association demo::Broken
{
  left: demo::Left[1];
}
"#,
    );

    let left = graph.class("demo::Left").expect("left");
    let right = graph.class("demo::Right").expect("right");
    assert!(left.coverage_gap());
    assert!(right.coverage_gap());
    assert!(!left.properties().contains_key("right"));
    assert!(!right.properties().contains_key("left"));
    assert!(graph.associations().is_empty());
}

#[test]
fn confirmed_pure_association_materializes_after_later_sources_supply_its_owners() {
    let left = empty_pmcd_class("Left");
    let right = empty_pmcd_class("Right");
    let graph = load_model_documents(&[
        ModelDocument::Pure(PureDocument::new(
            "links.pure",
            r#"
Association demo::Links
{
  left: demo::Left[1];
  rights: demo::Right[*];
}
"#,
        )),
        ModelDocument::Pmcd(PmcdDocument::new("left.pmcd.json", &left)),
        ModelDocument::Pmcd(PmcdDocument::new("right.pmcd.json", &right)),
    ])
    .expect("load mixed model");

    let association = graph.associations().first().expect("association");
    assert_eq!(association.provenance(), Provenance::PureFile);
    assert_eq!(association.source().index(), 0);
    assert_eq!(association.end_a().owner().as_str(), "demo::Right");
    assert_eq!(association.end_b().owner().as_str(), "demo::Left");
    assert!(
        graph
            .class("demo::Right")
            .expect("right")
            .properties()
            .contains_key("left")
    );
    assert!(
        graph
            .class("demo::Left")
            .expect("left")
            .properties()
            .contains_key("rights")
    );
}

#[test]
fn pmcd_association_survives_a_colliding_pure_association_in_either_order() {
    let pmcd_first = mixed_association_collision_graph(false);
    assert_trusted_pmcd_association(&pmcd_first, 2);
    assert_unresolved_pure_collision(&pmcd_first, 3);

    let pure_first = mixed_association_collision_graph(true);
    assert_trusted_pmcd_association(&pure_first, 3);
    assert_unresolved_pure_collision(&pure_first, 0);
}

#[test]
fn one_pmcd_association_wins_against_multiple_colliding_pure_ends() {
    let left = empty_pmcd_class("Left");
    let right = empty_pmcd_class("Right");
    let trusted = trusted_pmcd_association();
    let graph = load_model_documents(&[
        ModelDocument::Pmcd(PmcdDocument::new("left.pmcd.json", &left)),
        ModelDocument::Pmcd(PmcdDocument::new("right.pmcd.json", &right)),
        ModelDocument::Pmcd(PmcdDocument::new("trusted.pmcd.json", &trusted)),
        ModelDocument::Pure(PureDocument::new(
            "first-colliding.pure",
            COLLIDING_PURE_ASSOCIATION,
        )),
        ModelDocument::Pure(PureDocument::new(
            "second-colliding.pure",
            SECOND_COLLIDING_PURE_ASSOCIATION,
        )),
    ])
    .expect("Pure uncertainty must not invalidate the sole PMCD association");

    assert_trusted_pmcd_association(&graph, 2);
}

#[test]
fn two_pmcd_associations_do_not_survive_a_colliding_pure_end() {
    let left = empty_pmcd_class("Left");
    let right = empty_pmcd_class("Right");
    let trusted = trusted_pmcd_association();
    let second = second_pmcd_association();
    let error = load_model_documents(&[
        ModelDocument::Pmcd(PmcdDocument::new("left.pmcd.json", &left)),
        ModelDocument::Pmcd(PmcdDocument::new("right.pmcd.json", &right)),
        ModelDocument::Pmcd(PmcdDocument::new("trusted.pmcd.json", &trusted)),
        ModelDocument::Pmcd(PmcdDocument::new("second.pmcd.json", &second)),
        ModelDocument::Pure(PureDocument::new(
            "colliding.pure",
            COLLIDING_PURE_ASSOCIATION,
        )),
    ])
    .expect_err("two closed-world PMCD ends must not be suppressed by a Pure collision");

    assert!(error.to_string().contains("demo::Second"));
}

struct AssociationWinnerExpectation<'a> {
    provenance: Provenance,
    expected_left_property: &'a str,
    absent_left_property: &'a str,
    expected_right_property: &'a str,
    absent_right_property: &'a str,
    prior_source: u32,
}

fn assert_same_path_association_winner(
    graph: &pure_analyzer_model::ModelGraph,
    expected: AssociationWinnerExpectation<'_>,
) {
    let association = graph.associations().first().expect("association winner");
    assert_eq!(association.path().as_str(), "demo::ZTrusted");
    assert_eq!(association.provenance(), expected.provenance);
    assert_eq!(association.source().index(), 3);
    let left = graph.class("demo::Left").expect("left");
    let right = graph.class("demo::Right").expect("right");
    assert!(
        left.properties()
            .contains_key(expected.expected_left_property)
    );
    assert!(
        !left
            .properties()
            .contains_key(expected.absent_left_property)
    );
    assert!(
        right
            .properties()
            .contains_key(expected.expected_right_property)
    );
    assert!(
        !right
            .properties()
            .contains_key(expected.absent_right_property)
    );
    let conflicts = graph
        .diagnostics()
        .iter()
        .filter(|diagnostic| diagnostic.code == MODEL_MERGE_CONFLICT)
        .collect::<Vec<_>>();
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0].primary.file.index(), 3);
    assert_eq!(
        conflicts[0].secondary[0].file.index(),
        expected.prior_source
    );
}

#[test]
fn same_path_pmcd_and_pure_associations_are_last_source_wins() {
    let pmcd_wins = same_path_association_replacement_graph(true);
    assert_same_path_association_winner(
        &pmcd_wins,
        AssociationWinnerExpectation {
            provenance: Provenance::Pmcd,
            expected_left_property: "shared",
            absent_left_property: "pureShared",
            expected_right_property: "lefts",
            absent_right_property: "pureLefts",
            prior_source: 0,
        },
    );

    let pure_wins = same_path_association_replacement_graph(false);
    assert_same_path_association_winner(
        &pure_wins,
        AssociationWinnerExpectation {
            provenance: Provenance::PureFile,
            expected_left_property: "pureShared",
            absent_left_property: "shared",
            expected_right_property: "pureLefts",
            absent_right_property: "lefts",
            prior_source: 2,
        },
    );
}

#[test]
fn unmaterializable_pure_association_supersedes_same_path_pmcd_without_partial_facts() {
    let left = empty_pmcd_class("Left");
    let right = empty_pmcd_class("Right");
    let pmcd = trusted_pmcd_association();
    let graph = load_model_documents(&[
        ModelDocument::Pmcd(PmcdDocument::new("left.pmcd.json", &left)),
        ModelDocument::Pmcd(PmcdDocument::new("right.pmcd.json", &right)),
        ModelDocument::Pmcd(PmcdDocument::new("trusted.pmcd.json", &pmcd)),
        ModelDocument::Pure(PureDocument::new(
            "replacement.pure",
            UNMATERIALIZABLE_REPLACING_PURE_ASSOCIATION,
        )),
    ])
    .expect("unmaterializable replacement remains recoverable");

    assert!(graph.associations().is_empty());
    let left = graph.class("demo::Left").expect("left");
    let right = graph.class("demo::Right").expect("right");
    assert!(left.coverage_gap());
    assert!(right.coverage_gap());
    assert!(!left.properties().contains_key("shared"));
    assert!(!right.properties().contains_key("lefts"));
    assert!(!right.properties().contains_key("missing"));
    assert!(
        graph
            .diagnostics()
            .iter()
            .any(|diagnostic| diagnostic.code == MODEL_MERGE_CONFLICT),
        "the later same-path declaration must report deterministic replacement"
    );
    assert!(
        graph
            .diagnostics()
            .iter()
            .any(|diagnostic| diagnostic.code == DiagCode::UnresolvedModelAssociation),
        "the unmaterializable Pure association needs an explicit diagnostic"
    );
}

#[test]
fn pure_association_with_a_missing_owner_is_diagnosed_without_partial_facts() {
    let source = r#"
Class demo::Known
{
}
Association demo::Broken
{
  known: demo::Known[1];
  missing: demo::Missing[1];
}
"#;
    let graph = pure(source);

    let known = graph.class("demo::Known").expect("known class");
    assert!(known.coverage_gap());
    assert!(known.properties().is_empty());
    assert!(graph.associations().is_empty());
    let diagnostic = graph
        .diagnostics()
        .iter()
        .find(|diagnostic| diagnostic.code == DiagCode::UnresolvedModelAssociation)
        .expect("missing owner diagnostic");
    assert_eq!(diagnostic.severity, Severity::Error);
    assert_eq!(diagnostic.primary.file.index(), 0);
    assert_eq!(
        diagnostic.primary.span,
        exact_span(
            source,
            "Association demo::Broken\n{\n  known: demo::Known[1];\n  missing: demo::Missing[1];\n}"
        ),
        "the preflight diagnostic retains the confirmed association declaration range"
    );
    assert!(diagnostic.message.contains("demo::Missing"));
}

#[test]
fn pure_association_end_conflicting_with_a_declared_property_is_not_materialized() {
    let graph = pure(
        r#"
Class demo::Left
{
  right: String[1];
}
Class demo::Right
{
}
Association demo::Broken
{
  left: demo::Left[1];
  right: demo::Right[1];
}
"#,
    );

    let left = graph.class("demo::Left").expect("left class");
    assert!(left.coverage_gap());
    assert_eq!(
        left.properties()["right"].target().raw_type().as_str(),
        "String"
    );
    assert!(!left.properties()["right"].from_assoc());
    let right = graph.class("demo::Right").expect("right class");
    assert!(right.coverage_gap());
    assert!(right.properties().is_empty());
    assert!(graph.associations().is_empty());
    assert_eq!(
        graph
            .diagnostics()
            .iter()
            .filter(|diagnostic| diagnostic.code == DiagCode::UnresolvedModelAssociation)
            .count(),
        1
    );
}

#[test]
fn colliding_pure_associations_do_not_choose_a_lexical_winner() {
    let graph = pure(
        r#"
Class demo::Left
{
}
Class demo::Right
{
}
Association demo::First
{
  left: demo::Left[1];
  shared: demo::Right[1];
}
Association demo::Second
{
  alternate: demo::Left[1];
  shared: demo::Right[1];
}
"#,
    );

    let left = graph.class("demo::Left").expect("left class");
    let right = graph.class("demo::Right").expect("right class");
    assert!(left.coverage_gap());
    assert!(right.coverage_gap());
    assert!(left.properties().is_empty());
    assert!(right.properties().is_empty());
    assert!(graph.associations().is_empty());
    let diagnostics = graph
        .diagnostics()
        .iter()
        .filter(|diagnostic| diagnostic.code == DiagCode::UnresolvedModelAssociation)
        .collect::<Vec<_>>();
    assert_eq!(diagnostics.len(), 2);
    assert_eq!(diagnostics[0].severity, Severity::Error);
    assert_eq!(diagnostics[1].severity, Severity::Error);
    assert!(diagnostics[0].message.contains("demo::First"));
    assert!(diagnostics[1].message.contains("demo::Second"));
}

#[test]
fn incomplete_pure_association_opens_prior_source_classes_without_a_pure_class() {
    let pmcd = empty_pmcd_class("Existing");
    let graph = load_model_documents(&[
        ModelDocument::Pmcd(PmcdDocument::new("complete.pmcd.json", &pmcd)),
        ModelDocument::Pure(PureDocument::new(
            "association.pure",
            r#"
Association demo::Related
{
  existing: demo::Existing[1];
  other: demo::Other[1];
}
"#,
        )),
    ])
    .expect("load mixed model");

    let existing = graph.class("demo::Existing").expect("prior class");
    assert_eq!(existing.provenance(), Provenance::Pmcd);
    assert!(
        existing.coverage_gap(),
        "an incomplete later association might contribute an unknown end"
    );
    assert!(graph.associations().is_empty());
}

#[test]
fn pure_associations_open_future_source_classes() {
    let future = empty_pmcd_class("Future");
    let graph = load_model_documents(&[
        ModelDocument::Pure(PureDocument::new(
            "association.pure",
            r#"
Association demo::Related
{
  future: demo::Future[1];
  other: demo::Other[1];
}
"#,
        )),
        ModelDocument::Pmcd(PmcdDocument::new("future.pmcd.json", &future)),
    ])
    .expect("load mixed model");

    let future = graph.class("demo::Future").expect("future class");
    assert!(future.coverage_gap());
    assert!(graph.associations().is_empty());
}

#[test]
fn pure_associations_open_replaced_source_classes() {
    let first = empty_pmcd_class("Replaced");
    let replacement = empty_pmcd_class("Replaced");
    let graph = load_model_documents(&[
        ModelDocument::Pmcd(PmcdDocument::new("first.pmcd.json", &first)),
        ModelDocument::Pure(PureDocument::new(
            "association.pure",
            r#"
Association demo::Related
{
  replaced: demo::Replaced[1];
  other: demo::Other[1];
}
"#,
        )),
        ModelDocument::Pmcd(PmcdDocument::new("replacement.pmcd.json", &replacement)),
    ])
    .expect("load mixed model");

    let replaced = graph.class("demo::Replaced").expect("replacement class");
    assert_eq!(replaced.source().index(), 2);
    assert!(replaced.coverage_gap());
}

#[test]
fn duplicate_pure_members_do_not_choose_a_last_declaration() {
    let graph = pure(
        r#"
Class demo::Duplicate
{
  value: String[1];
  value: Integer[1];
  query(): String[1] {};
  query(): Integer[1] {};
}
"#,
    );

    let duplicate = graph.class("demo::Duplicate").expect("duplicate class");
    assert!(duplicate.coverage_gap());
    assert!(
        !duplicate.properties().contains_key("value"),
        "a duplicate member is not a confirmed fact"
    );
    assert!(
        !duplicate.qualified_properties().contains_key("query"),
        "a duplicate qualified member is not a confirmed fact"
    );
    let diagnostics = graph
        .diagnostics()
        .iter()
        .filter(|diagnostic| diagnostic.code == DiagCode::DuplicateModelDeclaration)
        .collect::<Vec<_>>();
    assert_eq!(diagnostics.len(), 2);
    assert!(diagnostics.iter().all(|diagnostic| {
        diagnostic.severity == Severity::Error
            && diagnostic.primary.file.index() == 0
            && diagnostic.secondary.len() == 1
            && diagnostic.secondary[0].file.index() == 0
    }));
    assert!(diagnostics.iter().any(|diagnostic| {
        diagnostic.message
            == "Pure class `demo::Duplicate` declares property `value` more than once"
    }));
    assert!(diagnostics.iter().any(|diagnostic| {
        diagnostic.message
            == "Pure class `demo::Duplicate` declares qualified property `query` more than once"
    }));
}

#[test]
fn duplicate_pure_associations_report_the_association_collision() {
    let graph = pure(
        r#"
Class demo::Left {}
Class demo::Right {}
Association demo::Links
{
  left: demo::Left[1];
  right: demo::Right[1];
}
Association demo::Links
{
  left: demo::Left[1];
  right: demo::Right[1];
}
"#,
    );

    assert!(graph.diagnostics().iter().any(|diagnostic| {
        diagnostic.code == DiagCode::DuplicateModelDeclaration
            && diagnostic.message == "Pure source declares association `demo::Links` more than once"
    }));
}

#[test]
fn malformed_duplicate_pure_members_do_not_leave_confirmed_facts() {
    for source in [
        r#"
Class demo::MalformedDuplicate {
  value: String[1];
  value: Integer;
  query(): String[1] {};
  query(): Integer {};
}
"#,
        r#"
Class demo::MalformedDuplicate {
  value: Integer;
  value: String[1];
  query(): Integer {};
  query(): String[1] {};
}
"#,
    ] {
        let graph = pure(source);
        let class = graph
            .class("demo::MalformedDuplicate")
            .expect("malformed duplicate class");
        assert!(class.coverage_gap(), "{source}");
        assert!(
            !class.properties().contains_key("value"),
            "a malformed duplicate property must invalidate any same-name fact: {source}"
        );
        assert!(
            !class.qualified_properties().contains_key("query"),
            "a malformed duplicate qualified property must invalidate any same-name fact: {source}"
        );
        assert_eq!(
            graph
                .diagnostics()
                .iter()
                .filter(|diagnostic| diagnostic.code == DiagCode::DuplicateModelDeclaration)
                .count(),
            2,
            "each malformed duplicate needs a collision diagnostic: {source}"
        );
    }
}

#[test]
fn duplicate_pure_classes_do_not_choose_a_last_declaration() {
    let graph = pure(
        r#"
Class demo::Collision
{
  first: String[1];
}
Class demo::Collision
{
  second: Integer[1];
}
Class demo::Independent
{
  kept: Boolean[1];
}
"#,
    );

    assert!(
        graph.class("demo::Collision").is_none(),
        "duplicate class declarations cannot select a last definition"
    );
    let independent = graph.class("demo::Independent").expect("independent class");
    assert!(
        independent.coverage_gap(),
        "a duplicate declaration leaves same-source class coverage open-world"
    );
    assert!(independent.properties().contains_key("kept"));
    let diagnostics = graph
        .diagnostics()
        .iter()
        .filter(|diagnostic| diagnostic.code == DiagCode::DuplicateModelDeclaration)
        .collect::<Vec<_>>();
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].severity, Severity::Error);
    assert_eq!(
        diagnostics[0].message,
        "Pure source declares class `demo::Collision` more than once"
    );
    assert_eq!(diagnostics[0].secondary.len(), 1);
}

#[test]
fn duplicate_pure_top_level_paths_do_not_choose_a_declaration_group() {
    let graph = pure(
        r#"
Class demo::Left
{
}
Class demo::Right
{
}
Association demo::Collision
{
  left: demo::Left[1];
  right: demo::Right[1];
}
Class demo::Collision
{
  value: String[1];
}
"#,
    );

    assert!(
        graph.class("demo::Collision").is_none(),
        "a class/association path collision cannot select a class"
    );
    assert!(
        graph
            .associations()
            .iter()
            .all(|association| association.path().as_str() != "demo::Collision"),
        "a class/association path collision cannot select an association"
    );
    assert!(graph.class("demo::Left").expect("left").coverage_gap());
    assert!(graph.class("demo::Right").expect("right").coverage_gap());
    let diagnostics = graph
        .diagnostics()
        .iter()
        .filter(|diagnostic| diagnostic.code == DiagCode::DuplicateModelDeclaration)
        .collect::<Vec<_>>();
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].severity, Severity::Error);
    assert_eq!(
        diagnostics[0].message,
        "Pure source declares `demo::Collision` as both a class and association"
    );
    assert_eq!(diagnostics[0].secondary.len(), 1);
}

#[test]
fn mixed_documents_are_last_wins_with_the_existing_merge_diagnostic() {
    let pmcd = json!({
        "_type": "data",
        "elements": [{
            "_type": "class",
            "package": "demo",
            "name": "Winner",
            "superTypes": [],
            "stereotypes": [],
            "properties": [{
                "name": "value",
                "genericType": {"rawType": "Integer"},
                "multiplicity": {"lowerBound": 1, "upperBound": 1}
            }],
            "qualifiedProperties": []
        }]
    })
    .to_string();
    let pure_source = r#"
Class demo::Winner
{
  value: String[0..1];
}
"#;
    let graph = load_model_documents(&[
        ModelDocument::Pmcd(PmcdDocument::new("first.pmcd.json", &pmcd)),
        ModelDocument::Pure(PureDocument::new("second.pure", pure_source)),
    ])
    .expect("merge");

    let winner = graph.class("demo::Winner").expect("winner");
    assert_eq!(winner.provenance(), Provenance::PureFile);
    assert_eq!(winner.source().index(), 1);
    assert_eq!(
        winner.declaration_span(),
        Some(exact_span(
            pure_source,
            "Class demo::Winner\n{\n  value: String[0..1];\n}"
        ))
    );
    assert_eq!(
        winner.properties()["value"].target().raw_type().as_str(),
        "String"
    );
    assert_eq!(
        winner.properties()["value"].declaration_span(),
        Some(exact_span(pure_source, "value: String[0..1];"))
    );
    assert_eq!(graph.diagnostics().len(), 1);
    let diagnostic = &graph.diagnostics()[0];
    assert_eq!(diagnostic.code, MODEL_MERGE_CONFLICT);
    assert_eq!(diagnostic.primary.file.index(), 1);
    assert_eq!(diagnostic.secondary[0].file.index(), 0);
}

#[test]
fn mixed_documents_allow_a_later_pmcd_class_to_replace_pure_deterministically() {
    let pmcd = json!({
        "_type": "data",
        "elements": [{
            "_type": "class",
            "package": "demo",
            "name": "Winner",
            "superTypes": [],
            "stereotypes": [],
            "properties": [{
                "name": "value",
                "genericType": {"rawType": "Integer"},
                "multiplicity": {"lowerBound": 1, "upperBound": 1}
            }],
            "qualifiedProperties": []
        }]
    })
    .to_string();
    let pure_source = r#"
Class demo::Winner
{
  value: String[0..1];
}
"#;
    let graph = load_model_documents(&[
        ModelDocument::Pure(PureDocument::new("first.pure", pure_source)),
        ModelDocument::Pmcd(PmcdDocument::new("second.pmcd.json", &pmcd)),
    ])
    .expect("merge");

    let winner = graph.class("demo::Winner").expect("winner");
    assert_eq!(winner.provenance(), Provenance::Pmcd);
    assert_eq!(winner.source().index(), 1);
    assert_eq!(
        winner.properties()["value"].target().raw_type().as_str(),
        "Integer"
    );
    assert_eq!(graph.diagnostics().len(), 1);
    let diagnostic = &graph.diagnostics()[0];
    assert_eq!(diagnostic.code, MODEL_MERGE_CONFLICT);
    assert_eq!(diagnostic.primary.file.index(), 1);
    assert_eq!(diagnostic.secondary[0].file.index(), 0);
}
