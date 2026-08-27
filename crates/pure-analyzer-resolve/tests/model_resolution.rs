//! PMCD-backed resolver contracts.

#![allow(clippy::disallowed_methods)]

use proptest::prelude::*;
use pure_analyzer_diagnostics::TextRange;
use pure_analyzer_model::{
    ModelGraph, PmcdDocument, Provenance, QName, QpKind, Temporal, load_pmcd_documents,
};
use pure_analyzer_resolve::{
    DefinitionAnchor, Resolution, ResolvedMemberKind, Resolver, UnderResolution,
};
use serde_json::{Value, json};

const PACKAGE: &str = "model";

fn graph(elements: Vec<Value>) -> ModelGraph {
    let source = json!({"_type": "data", "elements": elements}).to_string();
    load_pmcd_documents(&[PmcdDocument::new("resolver-fixture", &source)])
        .expect("fixture must load")
}

fn graph_documents(documents: Vec<Vec<Value>>) -> ModelGraph {
    let sources = documents
        .into_iter()
        .map(|elements| json!({"_type": "data", "elements": elements}).to_string())
        .collect::<Vec<_>>();
    let labels = (0..sources.len())
        .map(|index| format!("resolver-fixture-{index}"))
        .collect::<Vec<_>>();
    let documents = labels
        .iter()
        .zip(&sources)
        .map(|(label, source)| PmcdDocument::new(label, source))
        .collect::<Vec<_>>();
    load_pmcd_documents(&documents).expect("fixtures must load")
}

fn path(name: &str) -> String {
    format!("{PACKAGE}::{name}")
}

fn qname(name: &str) -> QName {
    QName::new(path(name)).expect("fixture name must be valid")
}

fn member_name(name: &str) -> pure_analyzer_model::Name {
    pure_analyzer_model::Name::new(name).expect("fixture member name must be valid")
}

fn property_with_multiplicity(name: &str, target: &str, lower: u32, upper: u32) -> Value {
    json!({
        "name": name,
        "genericType": {"rawType": target, "typeArguments": []},
        "multiplicity": {"lowerBound": lower, "upperBound": upper},
    })
}

fn property(name: &str, target: &str) -> Value {
    property_with_multiplicity(name, target, 0, 1)
}

fn qualified_property(name: &str, target: &str, generated: bool) -> Value {
    let stereotypes = if generated {
        json!([{
            "profile": "meta::pure::profiles::milestoning",
            "value": "generatedmilestoningproperty",
        }])
    } else {
        json!([])
    };
    json!({
        "name": name,
        "returnGenericType": {"rawType": target, "typeArguments": []},
        "returnMultiplicity": {"lowerBound": 0, "upperBound": 1},
        "stereotypes": stereotypes,
    })
}

fn user_qualified_property(
    name: &str,
    target: &str,
    lower: u32,
    upper: u32,
    parameter_types: &[&str],
) -> Value {
    let parameters = parameter_types
        .iter()
        .map(|parameter| {
            json!({
                "genericType": {"rawType": parameter, "typeArguments": []},
            })
        })
        .collect::<Vec<_>>();
    json!({
        "name": name,
        "returnGenericType": {"rawType": target, "typeArguments": []},
        "returnMultiplicity": {"lowerBound": lower, "upperBound": upper},
        "stereotypes": [],
        "parameters": parameters,
    })
}

fn class(name: &str, supertypes: &[&str], properties: Vec<Value>, qualified: Vec<Value>) -> Value {
    json!({
        "_type": "class",
        "package": PACKAGE,
        "name": name,
        "superTypes": supertypes,
        "stereotypes": [],
        "properties": properties,
        "qualifiedProperties": qualified,
    })
}

fn temporal_class(name: &str, temporal: &str) -> Value {
    temporal_class_with_supertypes(name, &[], temporal)
}

fn temporal_class_with_supertypes(name: &str, supertypes: &[&str], temporal: &str) -> Value {
    let mut value = class(name, supertypes, Vec::new(), Vec::new());
    value["stereotypes"] = json!([{
        "profile": "meta::pure::profiles::temporal",
        "value": temporal,
    }]);
    value
}

fn association(
    name: &str,
    first_name: &str,
    first_target: &str,
    second_name: &str,
    second_target: &str,
    temporal: Option<&str>,
) -> Value {
    association_with_properties(
        name,
        property(first_name, first_target),
        property(second_name, second_target),
        temporal,
    )
}

fn association_with_properties(
    name: &str,
    first: Value,
    second: Value,
    temporal: Option<&str>,
) -> Value {
    let stereotypes = temporal.map_or_else(
        || json!([]),
        |value| {
            json!([{
                "profile": "meta::pure::profiles::temporal",
                "value": value,
            }])
        },
    );
    json!({
        "_type": "association",
        "package": PACKAGE,
        "name": name,
        "stereotypes": stereotypes,
        "properties": [first, second],
    })
}

fn precedence_graph(parents: &[&str]) -> ModelGraph {
    graph(vec![
        class(
            "GeneratedParent",
            &[],
            vec![property("hit", "Boolean")],
            vec![qualified_property("hit", "String", true)],
        ),
        class(
            "UserParent",
            &[],
            Vec::new(),
            vec![qualified_property("hit", "Integer", false)],
        ),
        class(
            "PlainParent",
            &[],
            vec![property("hit", "Decimal")],
            Vec::new(),
        ),
        class("AssociationParent", &[], Vec::new(), Vec::new()),
        class("AssociationTarget", &[], Vec::new(), Vec::new()),
        association(
            "ParentTarget",
            "otherEnd",
            "model::AssociationParent",
            "hit",
            "model::AssociationTarget",
            None,
        ),
        class("Child", parents, Vec::new(), Vec::new()),
    ])
}

fn found_member(
    resolver: &Resolver<'_>,
    class: &str,
    member: &str,
) -> pure_analyzer_resolve::ResolvedMember {
    match resolver.resolve_member(&qname(class), &member_name(member)) {
        Resolution::Found(member) => member,
        outcome => panic!("expected a member, got {outcome:#?}"),
    }
}

#[test]
fn resolves_qualified_names_and_class_ids_with_source_metadata() {
    let graph = graph(vec![temporal_class("Trade", "processingtemporal")]);
    let resolver = Resolver::new(&graph);
    let trade = match resolver.resolve_class(&qname("Trade")) {
        Resolution::Found(class) => class,
        outcome => panic!("expected a class, got {outcome:#?}"),
    };

    assert_eq!(
        trade.id(),
        graph.class_id("model::Trade").expect("class id")
    );
    assert_eq!(trade.path().as_str(), "model::Trade");
    assert_eq!(trade.temporal(), Some(Temporal::ProcessingTemporal));
    assert_eq!(trade.provenance(), Provenance::Pmcd);
    assert_eq!(trade.definition().source().index(), 0);
    assert_eq!(trade.definition().span(), None);
    let precise_span = TextRange::new(3.into(), 9.into());
    let precise_anchor = DefinitionAnchor::new(trade.definition().source(), Some(precise_span));
    assert_eq!(precise_anchor.source(), trade.definition().source());
    assert_eq!(precise_anchor.span(), Some(precise_span));
    assert_eq!(
        resolver.resolve_class_id(trade.id()),
        Resolution::Found(trade.clone())
    );
    assert_eq!(
        resolver.resolve_class(&qname("Missing")),
        Resolution::Missing
    );
}

#[test]
fn generalization_walk_is_breadth_first_lexical_and_deduplicated() {
    let graph = graph(vec![
        class("Root", &[], Vec::new(), Vec::new()),
        class("Alpha", &["model::Root"], Vec::new(), Vec::new()),
        class("Zeta", &["model::Root"], Vec::new(), Vec::new()),
        class(
            "Child",
            &["model::Zeta", "model::Alpha"],
            Vec::new(),
            Vec::new(),
        ),
    ]);
    let resolver = Resolver::new(&graph);
    let ancestors = match resolver.generalizations(&qname("Child")) {
        Resolution::Found(ancestors) => ancestors,
        outcome => panic!("expected ancestors, got {outcome:#?}"),
    };

    assert_eq!(
        ancestors
            .iter()
            .map(|ancestor| ancestor.path().as_str())
            .collect::<Vec<_>>(),
        ["model::Alpha", "model::Zeta", "model::Root"]
    );
}

#[test]
fn unordered_pmcd_declarations_produce_identical_resolution_facts() {
    let holder = class(
        "Holder",
        &[],
        vec![property("first", "String"), property("second", "Integer")],
        vec![
            qualified_property("firstVersion", "String", false),
            qualified_property("secondVersion", "Integer", false),
        ],
    );
    let reordered_holder = class(
        "Holder",
        &[],
        vec![property("second", "Integer"), property("first", "String")],
        vec![
            qualified_property("secondVersion", "Integer", false),
            qualified_property("firstVersion", "String", false),
        ],
    );
    let left = class("Left", &[], Vec::new(), Vec::new());
    let right = class("Right", &[], Vec::new(), Vec::new());
    let other = class("Other", &[], Vec::new(), Vec::new());
    let first_link = association(
        "ALink",
        "toLeft",
        "model::Left",
        "toOther",
        "model::Other",
        None,
    );
    let second_link = association(
        "ZLink",
        "toLeftAgain",
        "model::Left",
        "toRight",
        "model::Right",
        None,
    );
    let first = graph(vec![
        holder,
        left.clone(),
        right.clone(),
        other.clone(),
        first_link.clone(),
        second_link.clone(),
    ]);
    let second = graph(vec![
        second_link,
        other,
        first_link,
        right,
        reordered_holder,
        left,
    ]);

    assert_eq!(first, second);
    let first_resolution = found_member(&Resolver::new(&first), "Holder", "first");
    let second_resolution = found_member(&Resolver::new(&second), "Holder", "first");
    assert_eq!(first_resolution, second_resolution);
}

#[test]
fn member_lookup_retains_overrides_and_inherited_metadata() {
    let graph = graph(vec![
        class(
            "Base",
            &[],
            vec![property("inherited", "String"), property("value", "String")],
            Vec::new(),
        ),
        class(
            "Child",
            &["model::Base"],
            vec![property("value", "Integer")],
            Vec::new(),
        ),
    ]);
    let resolver = Resolver::new(&graph);
    let inherited = found_member(&resolver, "Child", "inherited");
    let override_value = found_member(&resolver, "Child", "value");

    assert_eq!(inherited.owner().path().as_str(), "model::Base");
    assert_eq!(inherited.target().raw_type().as_str(), "String");
    assert_eq!(inherited.kind(), &ResolvedMemberKind::Property);
    assert_eq!(inherited.definition().span(), None);
    assert_eq!(override_value.owner().path().as_str(), "model::Child");
    assert_eq!(override_value.target().raw_type().as_str(), "Integer");
    assert_eq!(
        resolver.resolve_member(&qname("Child"), &member_name("unknown")),
        Resolution::Missing
    );
}

#[test]
fn qualified_property_resolution_retains_metadata() {
    let graph = graph(vec![class(
        "Holder",
        &[],
        Vec::new(),
        vec![user_qualified_property(
            "query",
            "String",
            1,
            3,
            &["Integer", "Boolean"],
        )],
    )]);
    let member = found_member(&Resolver::new(&graph), "Holder", "query");

    assert_eq!(
        member.kind(),
        &ResolvedMemberKind::Qualified(QpKind::UserQualified)
    );
    assert_eq!(member.target().raw_type().as_str(), "String");
    assert_eq!(member.multiplicity().lower(), 1);
    assert_eq!(member.multiplicity().upper(), Some(3));
    assert_eq!(
        member
            .signature()
            .expect("user qualified property signature")
            .iter()
            .map(|parameter| parameter.raw_type().as_str())
            .collect::<Vec<_>>(),
        ["Integer", "Boolean"]
    );
    assert_eq!(member.provenance(), Provenance::Pmcd);
    assert_eq!(member.definition().source().index(), 0);
    assert_eq!(member.definition().span(), None);
}

#[test]
fn member_lookup_applies_precedence_within_one_inheritance_level() {
    let graph = graph(vec![
        class(
            "GeneratedParent",
            &[],
            vec![property("hit", "Boolean")],
            vec![qualified_property("hit", "String", true)],
        ),
        class(
            "UserParent",
            &[],
            Vec::new(),
            vec![qualified_property("hit", "Integer", false)],
        ),
        class(
            "PlainParent",
            &[],
            vec![property("hit", "Decimal")],
            Vec::new(),
        ),
        class("AssociationParent", &[], Vec::new(), Vec::new()),
        class("AssociationTarget", &[], Vec::new(), Vec::new()),
        association(
            "ParentTarget",
            "otherEnd",
            "model::AssociationParent",
            "hit",
            "model::AssociationTarget",
            None,
        ),
        class(
            "Child",
            &[
                "model::PlainParent",
                "model::AssociationParent",
                "model::UserParent",
                "model::GeneratedParent",
            ],
            Vec::new(),
            Vec::new(),
        ),
    ]);
    let resolver = Resolver::new(&graph);
    let member = found_member(&resolver, "Child", "hit");

    assert_eq!(member.owner().path().as_str(), "model::GeneratedParent");
    assert_eq!(member.target().raw_type().as_str(), "String");
    assert_eq!(
        member.kind(),
        &ResolvedMemberKind::Qualified(QpKind::MilestonedPoint)
    );
}

#[test]
fn each_member_category_outranks_only_lower_categories() {
    let user_graph = precedence_graph(&[
        "model::AssociationParent",
        "model::PlainParent",
        "model::UserParent",
    ]);
    let user = found_member(&Resolver::new(&user_graph), "Child", "hit");
    assert_eq!(user.owner().path().as_str(), "model::UserParent");
    assert_eq!(user.target().raw_type().as_str(), "Integer");
    assert_eq!(
        user.kind(),
        &ResolvedMemberKind::Qualified(QpKind::UserQualified)
    );

    let plain_graph = precedence_graph(&["model::AssociationParent", "model::PlainParent"]);
    let plain = found_member(&Resolver::new(&plain_graph), "Child", "hit");
    assert_eq!(plain.owner().path().as_str(), "model::PlainParent");
    assert_eq!(plain.target().raw_type().as_str(), "Decimal");
    assert_eq!(plain.kind(), &ResolvedMemberKind::Property);

    let association_graph = precedence_graph(&["model::AssociationParent"]);
    let association = found_member(&Resolver::new(&association_graph), "Child", "hit");
    assert_eq!(
        association.owner().path().as_str(),
        "model::AssociationParent"
    );
    assert_eq!(
        association.target().raw_type().as_str(),
        "model::AssociationTarget"
    );
    assert_eq!(
        association.kind(),
        &ResolvedMemberKind::AssociationEnd {
            association: qname("ParentTarget"),
        }
    );
}

#[test]
fn inherited_precedence_is_selected_before_target_temporality() {
    let graph = graph(vec![
        class("Broken", &["model::Absent"], Vec::new(), Vec::new()),
        class(
            "Generated",
            &[],
            Vec::new(),
            vec![qualified_property("hit", "String", true)],
        ),
        class(
            "Plain",
            &[],
            vec![property("hit", "model::Broken")],
            Vec::new(),
        ),
        class(
            "Child",
            &["model::Plain", "model::Generated"],
            Vec::new(),
            Vec::new(),
        ),
    ]);
    let member = found_member(&Resolver::new(&graph), "Child", "hit");

    assert_eq!(member.owner().path().as_str(), "model::Generated");
    assert_eq!(
        member.kind(),
        &ResolvedMemberKind::Qualified(QpKind::MilestonedPoint)
    );
}

#[test]
fn association_directions_keep_source_and_temporal_arity() {
    let graph = graph_documents(vec![
        vec![
            class("Left", &[], Vec::new(), Vec::new()),
            class("Right", &[], Vec::new(), Vec::new()),
        ],
        vec![association(
            "Link",
            "toLeft",
            "model::Left",
            "toRight",
            "model::Right",
            Some("bitemporal"),
        )],
    ]);
    let resolver = Resolver::new(&graph);
    let right = found_member(&resolver, "Left", "toRight");
    let left = found_member(&resolver, "Right", "toLeft");

    assert_eq!(right.target().raw_type().as_str(), "model::Right");
    assert_eq!(left.target().raw_type().as_str(), "model::Left");
    assert_eq!(right.target_temporal_arity(), Some(2));
    assert_eq!(left.target_temporal_arity(), Some(2));
    assert_eq!(right.owner().definition().source().index(), 0);
    assert_eq!(right.definition().source().index(), 1);
    assert_eq!(right.definition().span(), None);
    assert_eq!(
        right.kind(),
        &ResolvedMemberKind::AssociationEnd {
            association: qname("Link"),
        }
    );
}

#[test]
fn association_temporal_overlay_is_authoritative() {
    let graph = graph(vec![
        class("Broken", &["model::Absent"], Vec::new(), Vec::new()),
        class("Holder", &[], Vec::new(), Vec::new()),
        association(
            "Link",
            "toHolder",
            "model::Holder",
            "toBroken",
            "model::Broken",
            Some("processingtemporal"),
        ),
    ]);
    let member = found_member(&Resolver::new(&graph), "Holder", "toBroken");

    assert_eq!(member.target().raw_type().as_str(), "model::Broken");
    assert_eq!(member.target_temporal_arity(), Some(1));
}

#[test]
fn ambiguous_inherited_members_are_canonical_across_parent_order() {
    fn resolution_with_parent_order(
        parent_order: &[&str],
    ) -> Resolution<pure_analyzer_resolve::ResolvedMember> {
        let graph = graph(vec![
            class("Left", &[], vec![property("shared", "String")], Vec::new()),
            class(
                "Right",
                &[],
                vec![property("shared", "Integer")],
                Vec::new(),
            ),
            class("Child", parent_order, Vec::new(), Vec::new()),
        ]);
        Resolver::new(&graph).resolve_member(&qname("Child"), &member_name("shared"))
    }

    let first = resolution_with_parent_order(&["model::Right", "model::Left"]);
    let second = resolution_with_parent_order(&["model::Left", "model::Right"]);

    assert_eq!(first, second);
    let Resolution::Ambiguous(candidates) = first else {
        panic!("expected ambiguous inherited property");
    };
    assert_eq!(
        candidates
            .iter()
            .map(|candidate| candidate.owner().path().as_str())
            .collect::<Vec<_>>(),
        ["model::Left", "model::Right"]
    );
}

#[test]
fn broken_generalizations_are_typed_outcomes() {
    let missing_graph = graph(vec![class(
        "Child",
        &["model::Absent"],
        Vec::new(),
        Vec::new(),
    )]);
    let missing = Resolver::new(&missing_graph).resolve_member(&qname("Child"), &member_name("x"));
    assert_eq!(
        missing,
        Resolution::UnderResolved(UnderResolution::MissingSupertype {
            owner: qname("Child"),
            missing: qname("Absent"),
        })
    );
    assert_eq!(
        Resolver::new(&missing_graph).generalizations(&qname("Child")),
        Resolution::UnderResolved(UnderResolution::MissingSupertype {
            owner: qname("Child"),
            missing: qname("Absent"),
        })
    );

    let cycle_graph = graph(vec![
        class("A", &["model::B"], Vec::new(), Vec::new()),
        class("B", &["model::A"], Vec::new(), Vec::new()),
    ]);
    let cycle = Resolver::new(&cycle_graph).resolve_member(&qname("A"), &member_name("x"));
    assert_eq!(
        cycle,
        Resolution::Cycle(vec![qname("A"), qname("B"), qname("A")])
    );
    assert_eq!(
        Resolver::new(&cycle_graph).generalizations(&qname("A")),
        Resolution::Cycle(vec![qname("A"), qname("B"), qname("A")])
    );
}

#[test]
fn direct_member_wins_without_traversing_a_broken_ancestor() {
    let graph = graph(vec![class(
        "Child",
        &["model::Absent"],
        vec![property("direct", "String")],
        Vec::new(),
    )]);
    let member = found_member(&Resolver::new(&graph), "Child", "direct");

    assert_eq!(member.owner().path().as_str(), "model::Child");
    assert_eq!(member.target().raw_type().as_str(), "String");
}

#[test]
fn target_temporal_hierarchy_faults_remain_typed() {
    let missing_graph = graph(vec![
        class("Target", &["model::Absent"], Vec::new(), Vec::new()),
        class(
            "Holder",
            &[],
            vec![property("target", "model::Target")],
            Vec::new(),
        ),
    ]);
    assert_eq!(
        Resolver::new(&missing_graph).resolve_member(&qname("Holder"), &member_name("target")),
        Resolution::UnderResolved(UnderResolution::MissingSupertype {
            owner: qname("Target"),
            missing: qname("Absent"),
        })
    );

    let cycle_graph = graph(vec![
        class("A", &["model::B"], Vec::new(), Vec::new()),
        class("B", &["model::A"], Vec::new(), Vec::new()),
        class(
            "Holder",
            &[],
            vec![property("target", "model::A")],
            Vec::new(),
        ),
    ]);
    assert_eq!(
        Resolver::new(&cycle_graph).resolve_member(&qname("Holder"), &member_name("target")),
        Resolution::Cycle(vec![qname("A"), qname("B"), qname("A")])
    );
}

#[test]
fn target_temporal_arity_walks_generalizations() {
    let graph = graph(vec![
        temporal_class("TemporalBase", "processingtemporal"),
        class(
            "TemporalChild",
            &["model::TemporalBase"],
            Vec::new(),
            Vec::new(),
        ),
        class(
            "Holder",
            &[],
            vec![property("target", "model::TemporalChild")],
            Vec::new(),
        ),
    ]);
    let member = found_member(&Resolver::new(&graph), "Holder", "target");

    assert_eq!(member.target_temporal_arity(), Some(1));
}

#[test]
fn nearest_conflicting_temporal_ancestors_mask_a_distant_temporal_stereotype() {
    let graph = graph(vec![
        temporal_class("TemporalRoot", "processingtemporal"),
        temporal_class_with_supertypes(
            "BusinessParent",
            &["model::TemporalRoot"],
            "businesstemporal",
        ),
        temporal_class_with_supertypes(
            "ProcessingParent",
            &["model::TemporalRoot"],
            "processingtemporal",
        ),
        class(
            "Target",
            &["model::BusinessParent", "model::ProcessingParent"],
            Vec::new(),
            Vec::new(),
        ),
        class(
            "Holder",
            &[],
            vec![property("target", "model::Target")],
            Vec::new(),
        ),
    ]);

    let member = found_member(&Resolver::new(&graph), "Holder", "target");

    assert_eq!(member.target_temporal_arity(), None);
}

proptest! {
    #[test]
    fn property_multiplicity_survives_resolution(lower in 0_u32..5, width in 0_u32..5) {
        let upper = lower + width;
        let graph = graph(vec![class(
            "Holder",
            &[],
            vec![property_with_multiplicity("value", "String", lower, upper)],
            Vec::new(),
        )]);
        let member = found_member(&Resolver::new(&graph), "Holder", "value");

        prop_assert_eq!(member.multiplicity().lower(), lower);
        prop_assert_eq!(member.multiplicity().upper(), Some(upper));
    }

    #[test]
    fn association_directions_and_multiplicities_survive_resolution(
        left_lower in 0_u32..5,
        left_width in 0_u32..5,
        right_lower in 0_u32..5,
        right_width in 0_u32..5,
    ) {
        let left_upper = left_lower + left_width;
        let right_upper = right_lower + right_width;
        let graph = graph(vec![
            class("Left", &[], Vec::new(), Vec::new()),
            class("Right", &[], Vec::new(), Vec::new()),
            association_with_properties(
                "Link",
                property_with_multiplicity("toLeft", "model::Left", left_lower, left_upper),
                property_with_multiplicity("toRight", "model::Right", right_lower, right_upper),
                None,
            ),
        ]);
        let resolver = Resolver::new(&graph);
        let from_left = found_member(&resolver, "Left", "toRight");
        let from_right = found_member(&resolver, "Right", "toLeft");

        prop_assert_eq!(from_left.target().raw_type().as_str(), "model::Right");
        prop_assert_eq!(from_left.multiplicity().lower(), right_lower);
        prop_assert_eq!(from_left.multiplicity().upper(), Some(right_upper));
        prop_assert_eq!(from_left.kind(), &ResolvedMemberKind::AssociationEnd {
            association: qname("Link"),
        });
        prop_assert_eq!(from_right.target().raw_type().as_str(), "model::Left");
        prop_assert_eq!(from_right.multiplicity().lower(), left_lower);
        prop_assert_eq!(from_right.multiplicity().upper(), Some(left_upper));
        prop_assert_eq!(from_right.kind(), &ResolvedMemberKind::AssociationEnd {
            association: qname("Link"),
        });
    }
}
