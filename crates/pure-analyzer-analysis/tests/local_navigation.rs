//! End-to-end local navigation analysis contracts.
#![allow(clippy::disallowed_methods)]

use pure_analyzer_analysis::{LocalResolution, analyze_m3_locals};
use pure_analyzer_diagnostics::FileId;
use pure_analyzer_model::{
    ModelGraph, Multiplicity, PmcdDocument, QName, TypeRef, load_pmcd_documents,
};
use pure_analyzer_parser::parse_query;
use pure_analyzer_resolve::{
    LocalValueKind, NavigationChain, NavigationResolution, NavigationTarget,
    NavigationUnderResolution, RelationColumnId, RelationRow, Resolution, UnknownValue,
};
use pure_analyzer_syntax::{GreenElement, GreenNode, SyntaxKind, TextRange};
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

fn found_chain(navigation: &pure_analyzer_analysis::LocalResolutionSite) -> &NavigationChain {
    match navigation.outcome() {
        LocalResolution::Navigation(NavigationResolution::Found(chain)) => chain,
        outcome => panic!("typed relation-column navigation must resolve: {outcome:#?}"),
    }
}

fn relation_row_source(chain: &NavigationChain) -> &RelationRow {
    match chain.source().kind() {
        LocalValueKind::RelationRow(row) => row,
        source => panic!("typed lambda binder must retain its relation row: {source:#?}"),
    }
}

fn assert_typed_relation_columns(row: &RelationRow, source: &str) {
    let [zeta, alpha] = row.columns() else {
        panic!(
            "expected the two declared columns, got {:#?}",
            row.columns()
        );
    };
    assert_eq!(zeta.id(), RelationColumnId::new(ZERO));
    assert_eq!(zeta.id().index(), ZERO);
    assert_eq!(zeta.name().as_str(), "zeta");
    assert_eq!(zeta.type_ref().raw_type().as_str(), "String");
    assert_eq!(
        zeta.multiplicity(),
        Multiplicity::new(ONE, Some(ONE)).expect("exact multiplicity must be valid")
    );
    assert_eq!(range_text(source, zeta.span()), "zeta:String[1]");

    assert_eq!(alpha.id(), RelationColumnId::new(ONE));
    assert_eq!(alpha.id().index(), ONE);
    assert_eq!(alpha.name().as_str(), "alpha");
    assert_eq!(alpha.type_ref().raw_type().as_str(), "Map");
    assert_eq!(
        alpha.type_ref().type_arguments(),
        &[
            TypeRef::new(
                QName::new("String").expect("fixture type must be valid"),
                Vec::new()
            ),
            TypeRef::new(
                QName::new("Integer").expect("fixture type must be valid"),
                Vec::new()
            ),
        ]
    );
    assert_eq!(
        alpha.multiplicity(),
        Multiplicity::new(ZERO, None).expect("unbounded multiplicity must be valid")
    );
    assert_eq!(
        range_text(source, alpha.span()),
        "alpha:Map<String,Integer>[0..*]"
    );
}

fn first_node_of_kind(node: &GreenNode, kind: SyntaxKind) -> Option<GreenNode> {
    (node.kind() == kind).then(|| node.clone()).or_else(|| {
        node.children()
            .iter()
            .filter_map(GreenElement::as_node)
            .find_map(|child| first_node_of_kind(child, kind))
    })
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
                    NavigationTarget::RelationColumn(_) => None,
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
fn preserves_map_filter_and_lambda_results_for_later_navigation() {
    let graph = graph(vec![
        class("Person", vec![property("manager", "model::Manager")]),
        class(
            "Manager",
            vec![property("name", "String"), property("title", "String")],
        ),
    ]);
    let source = "model::Person.all()->map(x| $x.manager)->filter(x| $x.name).title";
    let analysis = analyze(source, &graph);
    let sites = analysis.sites();

    assert_eq!(sites.len(), 4);
    assert_eq!(range_text(source, sites[0].span()), "model::Person.all()");
    for (site, (span, owner)) in sites[1..].iter().zip([
        (".manager", "model::Person"),
        (".name", "model::Manager"),
        (".title", "model::Manager"),
    ]) {
        assert_eq!(range_text(source, site.span()), span);
        assert!(matches!(
            site.outcome(),
            LocalResolution::Navigation(NavigationResolution::Found(chain))
                if matches!(chain.hops()[0].target(), NavigationTarget::Member(member)
                    if member.owner().path().as_str() == owner)
        ));
    }
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
fn does_not_analyze_ungrounded_regular_function_lambdas() {
    let graph = graph(Vec::new());
    let source = "audit(x| $x.name)";
    let analysis = analyze(source, &graph);

    assert!(
        analysis.sites().is_empty(),
        "an unsupported regular function must not invent a lambda navigation outcome"
    );
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
    let source = "{prefix:String[1], row: Relation<(zeta:String[1], alpha:Map<String,Integer>[0..*])>| $row.alpha}";
    let analysis = analyze(source, &graph);
    let navigation = analysis
        .sites()
        .iter()
        .find(|site| range_text(source, site.span()) == ".alpha")
        .expect("relation-column navigation site must be recorded");

    let chain = found_chain(navigation);
    let row = relation_row_source(chain);
    assert_typed_relation_columns(row, source);
    assert!(matches!(
        chain.hops()[0].target(),
        NavigationTarget::RelationColumn(target) if target == &row.columns()[1]
    ));
    assert_eq!(analysis, analyze(source, &graph));
}

#[test]
fn typed_relation_binder_accepts_trivia_after_the_type_separator() {
    let graph = graph(Vec::new());
    let source = "row:\n Relation<(name:String[1])>| $row.name";
    let analysis = analyze(source, &graph);
    let navigation = analysis
        .sites()
        .iter()
        .find(|site| range_text(source, site.span()) == ".name")
        .expect("relation-column navigation site must be recorded");

    assert!(matches!(
        found_chain(navigation).hops()[0].target(),
        NavigationTarget::RelationColumn(column) if column.name().as_str() == "name"
    ));
}

#[test]
fn typed_class_column_preserves_multiplicity_for_follow_on_navigation() {
    let graph = graph(vec![class("Person", vec![property("name", "String")])]);
    let source = "row: Relation<(person:model::Person[1..*])>| $row.person.name";
    let analysis = analyze(source, &graph);

    let person = analysis
        .sites()
        .iter()
        .find(|site| range_text(source, site.span()) == ".person")
        .expect("relation-column navigation site must be recorded");
    let person_chain = found_chain(person);
    assert!(matches!(
        person_chain.hops()[0].target(),
        NavigationTarget::RelationColumn(column)
            if column.name().as_str() == "person"
                && column.type_ref().raw_type().as_str() == "model::Person"
    ));
    assert!(matches!(
        person_chain.value().kind(),
        LocalValueKind::Class(class) if class.path().as_str() == "model::Person"
    ));
    let expected_multiplicity =
        Multiplicity::new(ONE, None).expect("unbounded multiplicity must be valid");
    assert_eq!(person_chain.value().multiplicity(), expected_multiplicity);

    let name = analysis
        .sites()
        .iter()
        .find(|site| range_text(source, site.span()) == ".name")
        .expect("follow-on member navigation site must be recorded");
    let name_chain = found_chain(name);
    assert_eq!(name_chain.source().multiplicity(), expected_multiplicity);
    assert!(matches!(
        name_chain.hops()[0].target(),
        NavigationTarget::Member(member) if member.owner().path().as_str() == "model::Person"
    ));
}

#[test]
fn relation_binder_keeps_its_position_when_a_later_binder_is_not_a_relation() {
    let graph = graph(vec![class("Person", vec![property("name", "String")])]);
    let source = "model::Person.all()->filter({row: Relation<(name:String[1])>, plain:String[1]| $row.name; $plain.name})";
    let analysis = analyze(source, &graph);
    let navigations = analysis
        .sites()
        .iter()
        .filter(|site| range_text(source, site.span()) == ".name")
        .collect::<Vec<_>>();

    assert_eq!(navigations.len(), 2);
    assert!(matches!(
        navigations[0].outcome(),
        LocalResolution::Navigation(NavigationResolution::Found(chain))
            if matches!(chain.hops()[0].target(), NavigationTarget::RelationColumn(column)
                if column.name().as_str() == "name")
    ));
    assert!(matches!(
        navigations[1].outcome(),
        LocalResolution::Navigation(NavigationResolution::UnderResolved(_))
    ));
}

#[test]
fn typed_relation_generic_column_navigation_retains_type_and_multiplicity() {
    let graph = graph(Vec::new());
    let source = r#"
        row: Relation<(
            lookup: Map<
                String /* key */,
                Integer /* value */
            > [0 .. *]
        )> | $row.lookup
    "#;
    let analysis = analyze(source, &graph);
    let navigation = analysis
        .sites()
        .iter()
        .find(|site| range_text(source, site.span()) == ".lookup")
        .expect("relation-column navigation site must be recorded");
    let chain = found_chain(navigation);
    let expected_type = TypeRef::new(
        QName::new("Map").expect("fixture type must be valid"),
        vec![
            TypeRef::new(
                QName::new("String").expect("fixture type must be valid"),
                Vec::new(),
            ),
            TypeRef::new(
                QName::new("Integer").expect("fixture type must be valid"),
                Vec::new(),
            ),
        ],
    );

    assert!(matches!(
        chain.hops()[0].target(),
        NavigationTarget::RelationColumn(column) if column.type_ref() == &expected_type
    ));
    assert!(matches!(
        chain.value().kind(),
        LocalValueKind::Unknown(UnknownValue::UnmodeledType(actual)) if actual == &expected_type
    ));
    assert_eq!(
        chain.value().multiplicity(),
        Multiplicity::new(ZERO, None).expect("unbounded multiplicity must be valid")
    );
}

#[test]
fn generic_relation_type_without_a_row_schema_stays_under_resolved() {
    let graph = graph(Vec::new());
    let source = "row: Relation<String>| $row.name";
    let analysis = analyze(source, &graph);
    let navigation = analysis
        .sites()
        .iter()
        .find(|site| range_text(source, site.span()) == ".name")
        .expect("navigation site must be recorded");

    assert!(matches!(
        navigation.outcome(),
        LocalResolution::Navigation(NavigationResolution::UnderResolved(_))
    ));
}

#[test]
fn malformed_or_duplicate_typed_relation_columns_stay_under_resolved() {
    let graph = graph(Vec::new());
    for source in [
        "row: Relation<(name:String)>| $row.name",
        "row: Relation<(name:String[1], name:Integer[1])>| $row.name",
    ] {
        let analysis = analyze(source, &graph);
        let navigation = analysis
            .sites()
            .iter()
            .find(|site| range_text(source, site.span()) == ".name")
            .expect("relation-column navigation site must be recorded");
        assert!(
            matches!(
                navigation.outcome(),
                LocalResolution::Navigation(NavigationResolution::UnderResolved(_))
            ),
            "malformed relation row must not choose a column: {source} => {:#?}",
            navigation.outcome()
        );
    }
}

#[test]
fn invalid_typed_relation_binder_never_falls_back_to_the_incoming_class() {
    let graph = graph(vec![class("Person", vec![property("name", "String")])]);
    let source = "model::Person.all()->filter(row: Relation<(name:String)>| $row.name)";
    let analysis = analyze(source, &graph);
    let navigation = analysis
        .sites()
        .iter()
        .find(|site| range_text(source, site.span()) == ".name")
        .expect("relation-column navigation site must be recorded");

    assert!(
        matches!(
            navigation.outcome(),
            LocalResolution::Navigation(NavigationResolution::UnderResolved(_))
        ),
        "invalid typed binder must not use the incoming Person value: {:#?}",
        navigation.outcome()
    );
}

#[test]
fn duplicate_lambda_binders_never_select_one_relation_schema() {
    let graph = graph(vec![class("Person", vec![property("name", "String")])]);
    let source =
        "model::Person.all()->filter({row: Relation<(name:String[1])>, row:String[1]| $row.name})";
    let analysis = analyze(source, &graph);
    let navigation = analysis
        .sites()
        .iter()
        .find(|site| range_text(source, site.span()) == ".name")
        .expect("duplicate-binder navigation site must be recorded");

    assert!(
        matches!(
            navigation.outcome(),
            LocalResolution::Navigation(NavigationResolution::UnderResolved(_))
        ),
        "duplicate binders must not select a relation row: {:#?}",
        navigation.outcome()
    );
}

#[test]
fn preserves_typed_lambda_values_for_parenthesized_and_subtree_analysis() {
    let graph = graph(Vec::new());
    let source = "(row: Relation<(name:String[1])>| $row.name; $row).name";
    let parsed = parse_query(source, FileId::new(TEST_FILE_ID)).expect("fixture must parse");
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);

    let root_analysis = analyze_m3_locals(&parsed.green, &graph);
    let root_navigations = root_analysis
        .sites()
        .iter()
        .filter(|site| range_text(source, site.span()) == ".name")
        .collect::<Vec<_>>();
    assert_eq!(root_navigations.len(), 2);
    assert!(root_navigations.iter().all(|site| {
        matches!(
            site.outcome(),
            LocalResolution::Navigation(NavigationResolution::Found(chain))
                if matches!(chain.hops()[0].target(), NavigationTarget::RelationColumn(_))
        )
    }));

    let lambda = first_node_of_kind(&parsed.green, SyntaxKind::LAMBDA_EXPR)
        .expect("fixture must contain a lambda subtree");
    let subtree_analysis = analyze_m3_locals(&lambda, &graph);
    assert_eq!(subtree_analysis.sites().len(), 1);
    assert!(matches!(
        subtree_analysis.sites()[0].outcome(),
        LocalResolution::Navigation(NavigationResolution::Found(chain))
            if matches!(chain.hops()[0].target(), NavigationTarget::RelationColumn(_))
    ));
}

#[test]
fn evaluates_parenthesized_function_bases_before_the_call() {
    let graph = graph(vec![
        class("Person", vec![property("manager", "model::Manager")]),
        class("Manager", Vec::new()),
    ]);
    let source = "(model::Person.all().manager)()";
    let analysis = analyze(source, &graph);
    let sites = analysis.sites();

    assert_eq!(sites.len(), 2);
    assert_eq!(range_text(source, sites[0].span()), "model::Person.all()");
    assert_eq!(range_text(source, sites[1].span()), ".manager");
    assert!(matches!(
        sites[1].outcome(),
        LocalResolution::Navigation(NavigationResolution::Found(chain))
            if matches!(chain.hops()[0].target(), NavigationTarget::Member(member)
                if member.owner().path().as_str() == "model::Person")
    ));
}

#[test]
fn resolves_zero_argument_qualified_navigation_as_a_property_step() {
    let mut person = class("Person", Vec::new());
    person["qualifiedProperties"] = json!([qualified_property("zero", "String", &[])]);
    let graph = graph(vec![person]);
    let source = "model::Person.all()->filter(x| $x.zero())";
    let analysis = analyze(source, &graph);
    let navigation = analysis
        .sites()
        .iter()
        .find(|site| range_text(source, site.span()) == ".zero()")
        .expect("zero-argument navigation site must be recorded");

    let LocalResolution::Navigation(NavigationResolution::Found(chain)) = navigation.outcome()
    else {
        panic!(
            "expected a resolved zero-argument qualified navigation, got {:#?}",
            navigation.outcome()
        );
    };
    assert_eq!(chain.hops()[0].step().argument_count(), 0);
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
