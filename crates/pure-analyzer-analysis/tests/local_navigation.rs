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
    class_with_supertypes(name, &[], properties)
}

fn class_with_supertypes(name: &str, supertypes: &[&str], properties: Vec<Value>) -> Value {
    json!({
        "_type": "class",
        "package": PACKAGE,
        "name": name,
        "stereotypes": [],
        "superTypes": supertypes,
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

fn qualified_property(name: &str, target: &str, parameters: &[&str]) -> Value {
    let parameters = parameters
        .iter()
        .map(|parameter| json!({"genericType": {"rawType": parameter, "typeArguments": []}}))
        .collect::<Vec<_>>();
    json!({
        "name": name,
        "returnGenericType": {"rawType": target, "typeArguments": []},
        "returnMultiplicity": {"lowerBound": ZERO, "upperBound": ONE},
        "stereotypes": [],
        "parameters": parameters,
    })
}

fn association(name: &str, first: Value, second: Value) -> Value {
    json!({
        "_type": "association",
        "package": PACKAGE,
        "name": name,
        "stereotypes": [],
        "properties": [first, second],
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
fn visits_navigation_inside_regular_function_arguments() {
    let graph = graph(vec![class("Person", vec![property("name", "String")])]);
    let source = "audit(model::Person.all()->filter(x| $x.name))";
    let analysis = analyze(source, &graph);
    let sites = analysis.sites();

    assert_eq!(sites.len(), 2);
    assert_eq!(range_text(source, sites[0].span()), "model::Person.all()");
    assert_eq!(range_text(source, sites[1].span()), ".name");
    assert!(matches!(
        sites[1].outcome(),
        LocalResolution::Navigation(NavigationResolution::Found(chain))
            if matches!(chain.hops()[0].target(), NavigationTarget::Member(member)
                if member.owner().path().as_str() == "model::Person")
    ));
}

#[test]
fn resolves_qualified_navigation_calls_with_arguments() {
    let mut person = class("Person", Vec::new());
    person["qualifiedProperties"] = json!([qualified_property("byKey", "String", &["Integer"])]);
    let graph = graph(vec![person]);
    let source = "model::Person.all()->filter(x| $x.byKey(25))";
    let analysis = analyze(source, &graph);
    let navigation = analysis
        .sites()
        .iter()
        .find(|site| range_text(source, site.span()) == ".byKey(25)")
        .expect("qualified navigation site must be recorded");

    let LocalResolution::Navigation(NavigationResolution::Found(chain)) = navigation.outcome()
    else {
        panic!(
            "expected a resolved qualified navigation, got {:#?}",
            navigation.outcome()
        );
    };
    assert_eq!(chain.hops()[0].step().argument_count(), 1);
    assert!(matches!(
        chain.hops()[0].target(),
        NavigationTarget::Member(member)
            if matches!(member.kind(), pure_analyzer_resolve::ResolvedMemberKind::Qualified(_))
    ));
}

#[test]
fn resolves_inherited_member_navigation_end_to_end() {
    let graph = graph(vec![
        class("Base", vec![property("inherited", "String")]),
        class_with_supertypes("Child", &["model::Base"], Vec::new()),
    ]);
    let source = "model::Child.all()->filter(x| $x.inherited)";
    let analysis = analyze(source, &graph);
    let navigation = analysis
        .sites()
        .iter()
        .find(|site| range_text(source, site.span()) == ".inherited")
        .expect("inherited navigation site must be recorded");

    let LocalResolution::Navigation(NavigationResolution::Found(chain)) = navigation.outcome()
    else {
        panic!(
            "expected inherited member navigation, got {:#?}",
            navigation.outcome()
        );
    };
    assert_eq!(chain.hops().len(), 1);
    let NavigationTarget::Member(member) = chain.hops()[0].target() else {
        panic!("expected an inherited model-member navigation");
    };
    assert_eq!(member.owner().path().as_str(), "model::Base");
    assert!(matches!(
        member.kind(),
        pure_analyzer_resolve::ResolvedMemberKind::Property
    ));
    assert_eq!(chain.hops()[0].definition(), Some(member.definition()));
}

#[test]
fn resolves_association_end_navigation_end_to_end() {
    let graph = graph(vec![
        class("Person", Vec::new()),
        class("Manager", vec![property("name", "String")]),
        association(
            "Person_Manager",
            property("manager", "model::Manager"),
            property("reports", "model::Person"),
        ),
    ]);
    let source = "model::Person.all()->filter(x| $x.manager.name)";
    let analysis = analyze(source, &graph);
    let navigations = analysis
        .sites()
        .iter()
        .filter(|site| matches!(site.outcome(), LocalResolution::Navigation(_)))
        .collect::<Vec<_>>();

    assert_eq!(navigations.len(), 2);
    assert_eq!(range_text(source, navigations[0].span()), ".manager");
    let LocalResolution::Navigation(NavigationResolution::Found(chain)) = navigations[0].outcome()
    else {
        panic!(
            "expected association-end navigation, got {:#?}",
            navigations[0].outcome()
        );
    };
    assert_eq!(chain.hops().len(), 1);
    let NavigationTarget::Member(member) = chain.hops()[0].target() else {
        panic!("expected an association-derived model-member navigation");
    };
    assert_eq!(member.owner().path().as_str(), "model::Person");
    assert!(matches!(
        member.kind(),
        pure_analyzer_resolve::ResolvedMemberKind::AssociationEnd { association }
            if association.as_str() == "model::Person_Manager"
    ));
    assert_eq!(chain.hops()[0].definition(), Some(member.definition()));

    assert_eq!(range_text(source, navigations[1].span()), ".name");
    assert!(matches!(
        navigations[1].outcome(),
        LocalResolution::Navigation(NavigationResolution::Found(chain))
            if matches!(chain.hops()[0].target(), NavigationTarget::Member(member)
                if member.owner().path().as_str() == "model::Manager")
    ));
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
