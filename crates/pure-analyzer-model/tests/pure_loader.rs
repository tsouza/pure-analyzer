//! Contracts for resilient Pure Domain source ingestion.

#![allow(clippy::disallowed_methods)]

use pure_analyzer_diagnostics::{DiagCode, Severity};
use pure_analyzer_model::{
    MODEL_MERGE_CONFLICT, ModelDocument, PmcdDocument, Provenance, PureDocument, QpKind, Temporal,
    load_model_documents, load_pure_documents, load_pure_files,
};
use serde_json::json;
use std::path::PathBuf;

fn pure(source: &str) -> pure_analyzer_model::ModelGraph {
    load_pure_documents(&[PureDocument::new("memory:model.pure", source)]).expect("load Pure")
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

    let entity = graph.class("demo::Entity").expect("entity");
    assert_eq!(entity.temporal(), Some(Temporal::BusinessTemporal));
    let processing = graph.class("demo::Processing").expect("processing");
    assert_eq!(processing.temporal(), Some(Temporal::ProcessingTemporal));
    assert!(entity.properties()["order"].from_assoc());
    assert!(order.properties()["entities"].from_assoc());
    let association = &graph.associations()[0];
    assert_eq!(association.temporal(), Some(Temporal::ProcessingTemporal));
    assert_eq!(association.provenance(), Provenance::PureFile);
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
    let graph = pure(
        r#"
Enum demo::Future { enabled }
Class demo::Partial
{
    bad: Foo;
    good: String[1];
    query(value: String[1]): String[1] {};
}
"#,
    );

    let partial = graph.class("demo::Partial").expect("partial class");
    assert!(partial.coverage_gap());
    assert!(partial.properties().contains_key("good"));
    assert!(partial.qualified_properties().contains_key("query"));
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
fn pure_associations_open_prior_source_classes_without_a_pure_class() {
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
        winner.properties()["value"].target().raw_type().as_str(),
        "String"
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
