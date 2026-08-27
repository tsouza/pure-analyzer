//! Contracts for resilient Pure Domain source ingestion.

#![allow(clippy::disallowed_methods)]

use pure_analyzer_model::{
    MODEL_MERGE_CONFLICT, ModelDocument, PmcdDocument, Provenance, PureDocument, QpKind, Temporal,
    load_model_documents, load_pure_documents,
};
use serde_json::json;

fn pure(source: &str) -> pure_analyzer_model::ModelGraph {
    load_pure_documents(&[PureDocument::new("memory:model.pure", source)]).expect("load Pure")
}

#[test]
fn pure_domain_facts_lower_with_source_provenance_and_association_ends() {
    let graph = pure(
        r#"
Class <<temporal.businesstemporal>> demo::Entity
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
    assert_eq!(
        order.qualified_properties()["priceAfter"]
            .signature()
            .expect("user signature")[0]
            .raw_type()
            .as_str(),
        "StrictDate"
    );

    let entity = graph.class("demo::Entity").expect("entity");
    assert_eq!(entity.temporal(), Some(Temporal::BusinessTemporal));
    assert!(entity.properties()["order"].from_assoc());
    assert!(order.properties()["entities"].from_assoc());
    let association = &graph.associations()[0];
    assert_eq!(association.temporal(), Some(Temporal::ProcessingTemporal));
    assert_eq!(association.provenance(), Provenance::PureFile);
    assert_eq!(graph.sources()[0].provenance(), Provenance::PureFile);
}

#[test]
fn confirmed_generated_milestoning_qps_match_pmcd_truth_table() {
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

    let pmcd = json!({
        "_type": "data",
        "elements": [{
            "_type": "class",
            "package": "demo",
            "name": "Holder",
            "superTypes": [],
            "stereotypes": [{"profile": "temporal", "value": "businesstemporal"}],
            "properties": [],
            "qualifiedProperties": [
                generated_qp("quotes", 0, Some(1)),
                generated_qp("quotesAllVersions", 0, None),
                generated_qp("quotesAllVersionsInRange", 0, None),
                generated_qp("quotesEdge", 0, None)
            ]
        }]
    })
    .to_string();
    let pmcd_graph = load_model_documents(&[ModelDocument::Pmcd(PmcdDocument::new(
        "memory:model.pmcd.json",
        &pmcd,
    ))])
    .expect("load PMCD");
    let pmcd_properties = pmcd_graph
        .class("demo::Holder")
        .expect("PMCD holder")
        .qualified_properties();

    for name in [
        "quotes",
        "quotesAllVersions",
        "quotesAllVersionsInRange",
        "quotesEdge",
    ] {
        assert_eq!(pure_properties[name].kind(), pmcd_properties[name].kind());
        assert_eq!(
            pure_properties[name].target().raw_type(),
            pmcd_properties[name].target().raw_type()
        );
        assert_eq!(
            pure_properties[name].multiplicity(),
            pmcd_properties[name].multiplicity()
        );
    }
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
}
"#,
    );

    let partial = graph.class("demo::Partial").expect("partial class");
    assert!(partial.coverage_gap());
    assert!(partial.properties().contains_key("good"));
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
fn uncertain_associations_make_all_classes_from_that_pure_source_open_world() {
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

    assert!(graph.class("demo::Left").expect("left").coverage_gap());
    assert!(graph.class("demo::Right").expect("right").coverage_gap());
    assert!(graph.associations().is_empty());
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

fn generated_qp(name: &str, lower: u32, upper: Option<u32>) -> serde_json::Value {
    json!({
        "name": name,
        "returnGenericType": {"rawType": "demo::Quote"},
        "returnMultiplicity": {"lowerBound": lower, "upperBound": upper},
        "stereotypes": [{
            "profile": "milestoning",
            "value": "generatedmilestoningproperty"
        }],
        "parameters": []
    })
}
