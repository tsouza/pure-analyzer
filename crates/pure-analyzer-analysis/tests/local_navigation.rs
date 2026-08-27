//! End-to-end local navigation analysis contracts.
#![allow(clippy::disallowed_methods)]

use pure_analyzer_analysis::{LocalResolution, analyze_m3_locals};
use pure_analyzer_diagnostics::FileId;
use pure_analyzer_model::{ModelGraph, PmcdDocument, load_pmcd_documents};
use pure_analyzer_parser::parse_query;
use pure_analyzer_resolve::{
    LocalValueKind, NavigationResolution, NavigationTarget, NavigationUnderResolution, Resolution,
};
use pure_analyzer_syntax::TextRange;
use serde_json::{Value, json};

const PACKAGE: &str = "model";
const ZERO: u32 = 0;
const ONE: u32 = 1;
const TEST_FILE_ID: u32 = 29;

fn graph(elements: Vec<Value>) -> ModelGraph {
    let source = json!({"_type": "data", "elements": elements}).to_string();
    load_pmcd_documents(&[PmcdDocument::new("local-analysis-fixture", &source)])
        .expect("fixture model must load")
}

fn class(name: &str, properties: Vec<Value>) -> Value {
    json!({
        "_type": "class",
        "package": PACKAGE,
        "name": name,
        "stereotypes": [],
        "superTypes": [],
        "properties": properties,
        "qualifiedProperties": [],
    })
}

fn property(name: &str, target: &str) -> Value {
    json!({
        "name": name,
        "genericType": {"rawType": target, "typeArguments": []},
        "multiplicity": {"lowerBound": ZERO, "upperBound": ONE},
    })
}

fn analyze(source: &str, graph: &ModelGraph) -> pure_analyzer_analysis::LocalNavigationAnalysis {
    let parsed = parse_query(source, FileId::new(TEST_FILE_ID)).expect("fixture must parse");
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    analyze_m3_locals(&parsed.green, graph)
}

fn range_text(source: &str, range: TextRange) -> &str {
    &source[usize::from(range.start())..usize::from(range.end())]
}

#[test]
fn resolves_class_filter_let_and_navigation_hops_with_exact_spans() {
    let graph = graph(vec![
        class("Person", vec![property("manager", "model::Manager")]),
        class("Manager", vec![property("name", "String")]),
    ]);
    let source = "model::Person.all()->filter(x| let y = $x.manager; $y.name)";
    let analysis = analyze(source, &graph);
    let sites = analysis.sites();

    assert_eq!(sites.len(), 3);
    assert_eq!(range_text(source, sites[0].span()), "model::Person.all()");
    assert!(matches!(
        sites[0].outcome(),
        LocalResolution::ClassAll(Resolution::Found(value))
            if matches!(value.kind(), LocalValueKind::Class(class) if class.path().as_str() == "model::Person")
    ));

    assert_eq!(range_text(source, sites[1].span()), ".manager");
    assert_eq!(range_text(source, sites[2].span()), ".name");
    for (site, owner) in sites[1..].iter().zip(["model::Person", "model::Manager"]) {
        let LocalResolution::Navigation(NavigationResolution::Found(chain)) = site.outcome() else {
            panic!("expected a resolved navigation, got {:#?}", site.outcome());
        };
        assert_eq!(chain.hops().len(), 1);
        let NavigationTarget::Member(member) = chain.hops()[0].target() else {
            panic!("expected model-member navigation");
        };
        assert_eq!(member.owner().path().as_str(), owner);
        assert_eq!(chain.hops()[0].definition(), Some(member.definition()));
    }
}

#[test]
fn restores_outer_lambda_binding_after_nested_shadowing() {
    let graph = graph(vec![
        class(
            "Person",
            vec![
                property("manager", "model::Manager"),
                property("name", "String"),
            ],
        ),
        class("Manager", vec![property("name", "String")]),
    ]);
    let source = "model::Person.all()->filter(x| $x.manager->filter(x| $x.name); $x.name)";
    let analysis = analyze(source, &graph);
    let sites = analysis.sites();
    let resolved_owners = sites
        .iter()
        .filter_map(|site| match site.outcome() {
            LocalResolution::Navigation(NavigationResolution::Found(chain)) => {
                match chain.hops()[0].target() {
                    NavigationTarget::Member(member) => {
                        Some(member.owner().path().as_str().to_owned())
                    }
                    NavigationTarget::RelationColumn => None,
                }
            }
            LocalResolution::ClassAll(_) | LocalResolution::Navigation(_) => None,
        })
        .collect::<Vec<_>>();

    assert_eq!(
        resolved_owners,
        ["model::Person", "model::Manager", "model::Person"]
    );
}

#[test]
fn reports_unbound_variables_as_under_resolution_not_missing_members() {
    let graph = graph(vec![class("Person", Vec::new())]);
    let source = "model::Person.all()->filter(x| $missing.name)";
    let analysis = analyze(source, &graph);
    let navigation = analysis
        .sites()
        .iter()
        .find(|site| range_text(source, site.span()) == ".name")
        .expect("navigation site must be recorded");

    assert!(matches!(
        navigation.outcome(),
        LocalResolution::Navigation(NavigationResolution::UnderResolved(
            NavigationUnderResolution::UnboundVariable { name }
        )) if name.as_str() == "missing"
    ));
}

#[test]
fn typed_relation_lambda_binder_resolves_known_column() {
    let graph = graph(Vec::new());
    let source = "row: Relation<(name:String[1])>| $row.name";
    let analysis = analyze(source, &graph);
    let navigation = analysis
        .sites()
        .iter()
        .find(|site| range_text(source, site.span()) == ".name")
        .expect("relation-column navigation site must be recorded");

    assert!(matches!(
        navigation.outcome(),
        LocalResolution::Navigation(NavigationResolution::Found(chain))
            if matches!(chain.hops()[0].target(), NavigationTarget::RelationColumn)
    ));
}

#[test]
fn opaque_and_recovered_sources_do_not_produce_missing_member_outcomes() {
    let graph = graph(vec![class("Person", Vec::new())]);
    for source in ["#{opaque}#.name", "model::Person.all()->filter(x| $x.)"] {
        let parsed =
            parse_query(source, FileId::new(TEST_FILE_ID)).expect("fixture must build a tree");
        let analysis = analyze_m3_locals(&parsed.green, &graph);

        assert!(analysis.sites().iter().all(|site| {
            !matches!(
                site.outcome(),
                LocalResolution::Navigation(NavigationResolution::Missing(_))
            )
        }));
    }
}
