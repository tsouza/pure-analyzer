//! Resolver contracts shared by confirmed PMCD and Pure Domain model facts.

#![allow(clippy::disallowed_methods)]

use pure_analyzer_model::{
    MODEL_MERGE_CONFLICT, ModelDocument, ModelGraph, Multiplicity, Name, PmcdDocument, Provenance,
    PureDocument, QName, QpKind, TypeRef, load_model_documents,
};
use pure_analyzer_resolve::{
    NavigationResolution, NavigationResolver, NavigationStep, NavigationTarget, Resolution,
    ResolvedMember, ResolvedMemberKind, Resolver, UnderResolution,
};
use serde_json::{Value, json};

const PACKAGE: &str = "model";
const BITEMPORAL: &str = "bitemporal";
const GENERATED_MILESTONING_PROPERTY: &str = "generatedmilestoningproperty";
const MILESTONING_PROFILE: &str = "meta::pure::profiles::milestoning";
const TEMPORAL_PROFILE: &str = "meta::pure::profiles::temporal";
const MERGE_REPETITIONS: usize = 16;

fn model_path(name: &str) -> String {
    format!("{PACKAGE}::{name}")
}

fn qname(name: &str) -> QName {
    QName::new(model_path(name)).expect("fixture name must be valid")
}

fn member_name(name: &str) -> Name {
    Name::new(name).expect("fixture member name must be valid")
}

fn multiplicity(lower: u32, upper: Option<u32>) -> Value {
    json!({"lowerBound": lower, "upperBound": upper})
}

fn pmcd_property(name: &str, target: &str, lower: u32, upper: Option<u32>) -> Value {
    json!({
        "name": name,
        "genericType": {"rawType": target, "typeArguments": []},
        "multiplicity": multiplicity(lower, upper),
    })
}

fn pmcd_qualified_property(
    name: &str,
    target: &str,
    lower: u32,
    upper: Option<u32>,
    generated: bool,
    parameter_types: &[&str],
) -> Value {
    let stereotypes = if generated {
        json!([{
            "profile": MILESTONING_PROFILE,
            "value": GENERATED_MILESTONING_PROPERTY,
        }])
    } else {
        json!([])
    };
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
        "returnMultiplicity": multiplicity(lower, upper),
        "stereotypes": stereotypes,
        "parameters": parameters,
    })
}

fn pmcd_non_generated_milestoning_qualified_property(
    name: &str,
    target: &str,
    lower: u32,
    upper: Option<u32>,
    parameter_types: &[&str],
) -> Value {
    let mut property = pmcd_qualified_property(name, target, lower, upper, false, parameter_types);
    property["stereotypes"] = json!([{
        "profile": MILESTONING_PROFILE,
        "value": "notgenerated",
    }]);
    property
}

fn pmcd_class(
    name: &str,
    supertypes: &[&str],
    temporal: Option<&str>,
    properties: Vec<Value>,
    qualified_properties: Vec<Value>,
) -> Value {
    let stereotypes = temporal.map_or_else(
        || json!([]),
        |temporal| {
            json!([{
                "profile": TEMPORAL_PROFILE,
                "value": temporal,
            }])
        },
    );
    json!({
        "_type": "class",
        "package": PACKAGE,
        "name": name,
        "superTypes": supertypes,
        "stereotypes": stereotypes,
        "properties": properties,
        "qualifiedProperties": qualified_properties,
    })
}

fn pmcd_association(name: &str, first: Value, second: Value, temporal: Option<&str>) -> Value {
    let stereotypes = temporal.map_or_else(
        || json!([]),
        |temporal| {
            json!([{
                "profile": TEMPORAL_PROFILE,
                "value": temporal,
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

fn pmcd_source(elements: Vec<Value>) -> String {
    json!({"_type": "data", "elements": elements}).to_string()
}

fn pmcd_graph(elements: Vec<Value>) -> ModelGraph {
    let source = pmcd_source(elements);
    load_model_documents(&[ModelDocument::Pmcd(PmcdDocument::new(
        "parity.pmcd.json",
        &source,
    ))])
    .expect("PMCD fixture must load")
}

fn pure_graph(source: &str) -> ModelGraph {
    load_model_documents(&[ModelDocument::Pure(PureDocument::new(
        "parity.pure",
        source,
    ))])
    .expect("Pure fixture must load")
}

fn paired_graphs(elements: Vec<Value>, pure: &str) -> (ModelGraph, ModelGraph) {
    (pmcd_graph(elements), pure_graph(pure))
}

fn mixed_graph(pmcd: &str, pure: &str, pmcd_first: bool) -> ModelGraph {
    let documents = if pmcd_first {
        [
            ModelDocument::Pmcd(PmcdDocument::new("first.pmcd.json", pmcd)),
            ModelDocument::Pure(PureDocument::new("second.pure", pure)),
        ]
    } else {
        [
            ModelDocument::Pure(PureDocument::new("first.pure", pure)),
            ModelDocument::Pmcd(PmcdDocument::new("second.pmcd.json", pmcd)),
        ]
    };
    load_model_documents(&documents).expect("mixed fixture must load")
}

fn found_member(graph: &ModelGraph, class: &str, member: &str) -> ResolvedMember {
    match Resolver::new(graph).resolve_member(&qname(class), &member_name(member)) {
        Resolution::Found(member) => member,
        outcome => panic!("expected a member, got {outcome:#?}"),
    }
}

fn found_navigation_member(
    graph: &ModelGraph,
    class: &str,
    member: &str,
    argument_count: usize,
) -> ResolvedMember {
    let resolver = NavigationResolver::new(graph);
    let source = match resolver.class_all(&qname(class)) {
        Resolution::Found(value) => value,
        outcome => panic!("expected class value, got {outcome:#?}"),
    };
    let step = if argument_count == 0 {
        NavigationStep::property(member_name(member))
    } else {
        NavigationStep::call(member_name(member), argument_count)
    };
    let chain = match resolver.resolve(&source, &[step]) {
        NavigationResolution::Found(chain) => chain,
        outcome => panic!("expected navigation hop, got {outcome:#?}"),
    };
    let [hop] = chain.hops() else {
        panic!("expected exactly one navigation hop");
    };
    let NavigationTarget::Member(member) = hop.target() else {
        panic!("expected a model-member navigation target");
    };
    assert_eq!(hop.definition(), Some(member.definition()));
    member.clone()
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MemberFacts {
    owner: QName,
    target: TypeRef,
    multiplicity: Multiplicity,
    kind: ResolvedMemberKind,
    signature: Option<Vec<TypeRef>>,
    target_temporal_arity: Option<u8>,
}

impl From<&ResolvedMember> for MemberFacts {
    fn from(member: &ResolvedMember) -> Self {
        Self {
            owner: member.owner().path().clone(),
            target: member.target().clone(),
            multiplicity: member.multiplicity(),
            kind: member.kind().clone(),
            signature: member.signature().map(|signature| signature.to_vec()),
            target_temporal_arity: member.target_temporal_arity(),
        }
    }
}

fn assert_fact_parity(pmcd: &ResolvedMember, pure: &ResolvedMember) {
    assert_eq!(MemberFacts::from(pmcd), MemberFacts::from(pure));
    assert_eq!(pmcd.owner().provenance(), Provenance::Pmcd);
    assert_eq!(pmcd.provenance(), Provenance::Pmcd);
    assert_eq!(pure.owner().provenance(), Provenance::PureFile);
    assert_eq!(pure.provenance(), Provenance::PureFile);
}

fn assert_member_resolution_case(
    pmcd: &ModelGraph,
    pure: &ModelGraph,
    name: &str,
    expected_kind: ResolvedMemberKind,
    expected_lower: u32,
    expected_upper: Option<u32>,
) {
    let pmcd_member = found_member(pmcd, "Holder", name);
    let pure_member = found_member(pure, "Holder", name);

    assert_fact_parity(&pmcd_member, &pure_member);
    assert_eq!(pmcd_member.target().raw_type().as_str(), "model::Target");
    assert_eq!(pmcd_member.multiplicity().lower(), expected_lower);
    assert_eq!(pmcd_member.multiplicity().upper(), expected_upper);
    assert_eq!(pmcd_member.kind(), &expected_kind);
    assert_eq!(pmcd_member.target_temporal_arity(), Some(2));
}

#[test]
fn confirmed_class_members_have_pmcd_pure_resolution_parity() {
    let (pmcd, pure) = paired_graphs(
        vec![
            pmcd_class("Target", &[], Some(BITEMPORAL), Vec::new(), Vec::new()),
            pmcd_class(
                "Holder",
                &[],
                None,
                vec![pmcd_property("plain", "model::Target", 1, Some(1))],
                vec![
                    pmcd_qualified_property("point", "model::Target", 0, Some(1), true, &[]),
                    pmcd_qualified_property(
                        "pointAllVersions",
                        "model::Target",
                        0,
                        None,
                        true,
                        &[],
                    ),
                    pmcd_qualified_property(
                        "pointAllVersionsInRange",
                        "model::Target",
                        1,
                        None,
                        true,
                        &[],
                    ),
                    pmcd_qualified_property("pointEdge", "model::Target", 0, None, true, &[]),
                    pmcd_qualified_property(
                        "userAllVersions",
                        "model::Target",
                        1,
                        Some(2),
                        false,
                        &["StrictDate", "Boolean"],
                    ),
                ],
            ),
        ],
        r#"
Class <<temporal.bitemporal>> model::Target
{
}

Class model::Holder
{
  plain: model::Target[1];
  <<milestoning.generatedmilestoningproperty>>
  point(): model::Target[0..1] {};
  <<milestoning.generatedmilestoningproperty>>
  pointAllVersions(): model::Target[*] {};
  <<milestoning.generatedmilestoningproperty>>
  pointAllVersionsInRange(): model::Target[1..*] {};
  <<milestoning.generatedmilestoningproperty>>
  pointEdge(): model::Target[*] {};
  userAllVersions(asOf: StrictDate[1], enabled: Boolean[1]): model::Target[1..2] {};
}
"#,
    );

    let cases = [
        ("plain", ResolvedMemberKind::Property, 1, Some(1)),
        (
            "point",
            ResolvedMemberKind::Qualified(QpKind::MilestonedPoint),
            0,
            Some(1),
        ),
        (
            "pointAllVersions",
            ResolvedMemberKind::Qualified(QpKind::AllVersions),
            0,
            None,
        ),
        (
            "pointAllVersionsInRange",
            ResolvedMemberKind::Qualified(QpKind::AllVersionsInRange),
            1,
            None,
        ),
        (
            "pointEdge",
            ResolvedMemberKind::Qualified(QpKind::EdgePoint),
            0,
            None,
        ),
        (
            "userAllVersions",
            ResolvedMemberKind::Qualified(QpKind::UserQualified),
            1,
            Some(2),
        ),
    ];

    for (name, expected_kind, expected_lower, expected_upper) in cases {
        assert_member_resolution_case(
            &pmcd,
            &pure,
            name,
            expected_kind,
            expected_lower,
            expected_upper,
        );
    }

    for (name, argument_count) in [
        ("plain", 0),
        ("point", 2),
        ("pointAllVersions", 0),
        ("pointAllVersionsInRange", 2),
        ("pointEdge", 2),
        ("userAllVersions", 2),
    ] {
        let pmcd_member = found_navigation_member(&pmcd, "Holder", name, argument_count);
        let pure_member = found_navigation_member(&pure, "Holder", name, argument_count);

        assert_fact_parity(&pmcd_member, &pure_member);
    }
}

#[test]
fn non_generated_all_versions_forms_are_user_qualified_in_pmcd_and_pure() {
    let (pmcd, pure) = paired_graphs(
        vec![
            pmcd_class("Target", &[], Some(BITEMPORAL), Vec::new(), Vec::new()),
            pmcd_class(
                "Holder",
                &[],
                None,
                Vec::new(),
                vec![
                    pmcd_non_generated_milestoning_qualified_property(
                        "manualAllVersions",
                        "model::Target",
                        0,
                        None,
                        &[],
                    ),
                    pmcd_non_generated_milestoning_qualified_property(
                        "manualAllVersionsInRange",
                        "model::Target",
                        1,
                        None,
                        &[],
                    ),
                ],
            ),
        ],
        r#"
Class <<temporal.bitemporal>> model::Target {
}
Class model::Holder {
  <<milestoning.notgenerated>>
  manualAllVersions(): model::Target[*] {};
  <<milestoning.notgenerated>>
  manualAllVersionsInRange(): model::Target[1..*] {};
}
"#,
    );

    assert_member_resolution_case(
        &pmcd,
        &pure,
        "manualAllVersions",
        ResolvedMemberKind::Qualified(QpKind::UserQualified),
        0,
        None,
    );
    assert_member_resolution_case(
        &pmcd,
        &pure,
        "manualAllVersionsInRange",
        ResolvedMemberKind::Qualified(QpKind::UserQualified),
        1,
        None,
    );
}

fn precedence_pmcd_graph(parents: &[&str]) -> ModelGraph {
    pmcd_graph(vec![
        pmcd_class("Target", &[], Some(BITEMPORAL), Vec::new(), Vec::new()),
        pmcd_class(
            "GeneratedParent",
            &[],
            None,
            Vec::new(),
            vec![pmcd_qualified_property(
                "hit",
                "model::Target",
                0,
                Some(1),
                true,
                &[],
            )],
        ),
        pmcd_class(
            "UserParent",
            &[],
            None,
            Vec::new(),
            vec![pmcd_qualified_property(
                "hit",
                "model::Target",
                1,
                Some(2),
                false,
                &["StrictDate"],
            )],
        ),
        pmcd_class(
            "PlainParent",
            &[],
            None,
            vec![pmcd_property("hit", "model::Target", 1, Some(1))],
            Vec::new(),
        ),
        pmcd_class("AssociationParent", &[], None, Vec::new(), Vec::new()),
        pmcd_association(
            "ParentTarget",
            pmcd_property("otherEnd", "model::AssociationParent", 1, Some(1)),
            pmcd_property("hit", "model::Target", 0, Some(1)),
            None,
        ),
        pmcd_class("Child", parents, None, Vec::new(), Vec::new()),
    ])
}

const PRECEDENCE_PURE_PREFIX: &str = r#"
Class <<temporal.bitemporal>> model::Target
{
}

Class model::GeneratedParent
{
  <<milestoning.generatedmilestoningproperty>>
  hit(): model::Target[0..1] {};
}

Class model::UserParent
{
  hit(asOf: StrictDate[1]): model::Target[1..2] {};
}

Class model::PlainParent
{
  hit: model::Target[1];
}

Class model::AssociationParent
{
}

Association model::ParentTarget
{
  otherEnd: model::AssociationParent[1];
  hit: model::Target[0..1];
}

Class model::Child extends "#;

fn precedence_pure_graph(parents: &[&str]) -> ModelGraph {
    let mut source = PRECEDENCE_PURE_PREFIX.to_owned();
    source.push_str(&parents.join(", "));
    source.push_str("\n{\n}\n");
    pure_graph(&source)
}

fn assert_precedence_parity(parents: &[&str], owner: &str, kind: ResolvedMemberKind) {
    let pmcd = precedence_pmcd_graph(parents);
    let pure = precedence_pure_graph(parents);
    let pmcd_member = found_member(&pmcd, "Child", "hit");
    let pure_member = found_member(&pure, "Child", "hit");

    assert_fact_parity(&pmcd_member, &pure_member);
    assert_eq!(pmcd_member.owner().path().as_str(), model_path(owner));
    assert_eq!(pmcd_member.kind(), &kind);
    assert_eq!(pmcd_member.target().raw_type().as_str(), "model::Target");
    assert_eq!(pmcd_member.target_temporal_arity(), Some(2));
}

#[test]
fn inherited_member_precedence_has_pmcd_pure_parity() {
    assert_precedence_parity(
        &[
            "model::AssociationParent",
            "model::PlainParent",
            "model::UserParent",
            "model::GeneratedParent",
        ],
        "GeneratedParent",
        ResolvedMemberKind::Qualified(QpKind::MilestonedPoint),
    );
    assert_precedence_parity(
        &[
            "model::AssociationParent",
            "model::PlainParent",
            "model::UserParent",
        ],
        "UserParent",
        ResolvedMemberKind::Qualified(QpKind::UserQualified),
    );
    assert_precedence_parity(
        &["model::AssociationParent", "model::PlainParent"],
        "PlainParent",
        ResolvedMemberKind::Property,
    );
    assert_precedence_parity(
        &["model::AssociationParent"],
        "AssociationParent",
        ResolvedMemberKind::AssociationEnd {
            association: qname("ParentTarget"),
        },
    );
}

#[test]
fn bitemporal_association_directions_have_pmcd_pure_parity() {
    let (pmcd, pure) = paired_graphs(
        vec![
            pmcd_class("Left", &[], None, Vec::new(), Vec::new()),
            pmcd_class("Right", &[], None, Vec::new(), Vec::new()),
            pmcd_association(
                "Link",
                pmcd_property("left", "model::Left", 1, Some(2)),
                pmcd_property("rights", "model::Right", 0, None),
                Some(BITEMPORAL),
            ),
        ],
        r#"
Class model::Left
{
}

Class model::Right
{
}

Association <<temporal.bitemporal>> model::Link
{
  left: model::Left[1..2];
  rights: model::Right[*];
}
"#,
    );

    let cases = [
        ("Left", "rights", "model::Right", 0, None),
        ("Right", "left", "model::Left", 1, Some(2)),
    ];
    for (class, member, target, expected_lower, expected_upper) in cases {
        let pmcd_member = found_member(&pmcd, class, member);
        let pure_member = found_member(&pure, class, member);

        assert_fact_parity(&pmcd_member, &pure_member);
        assert_eq!(pmcd_member.target().raw_type().as_str(), target);
        assert_eq!(pmcd_member.multiplicity().lower(), expected_lower);
        assert_eq!(pmcd_member.multiplicity().upper(), expected_upper);
        assert_eq!(
            pmcd_member.kind(),
            &ResolvedMemberKind::AssociationEnd {
                association: qname("Link"),
            }
        );
        assert_eq!(pmcd_member.target_temporal_arity(), Some(2));
    }
}

#[test]
fn pure_coverage_gaps_are_open_world_while_pmcd_is_closed_world() {
    let pmcd = pmcd_graph(vec![pmcd_class(
        "Partial",
        &[],
        None,
        vec![pmcd_property("confirmed", "String", 1, Some(1))],
        Vec::new(),
    )]);
    let pure = pure_graph(
        r#"
Class model::Partial
{
  confirmed: String[1];
}

Association model::Broken
{
  partial: model::Partial[1];
}
"#,
    );
    let expected = Resolution::UnderResolved(UnderResolution::OpenWorld {
        class: qname("Partial"),
    });

    for member in ["confirmed", "missing"] {
        assert_eq!(
            Resolver::new(&pure).resolve_member(&qname("Partial"), &member_name(member)),
            expected
        );
    }
    assert!(matches!(
        Resolver::new(&pmcd).resolve_member(&qname("Partial"), &member_name("confirmed")),
        Resolution::Found(_)
    ));
    assert_eq!(
        Resolver::new(&pmcd).resolve_member(&qname("Partial"), &member_name("missing")),
        Resolution::Missing
    );
}

fn assert_mixed_winner(
    graph: &ModelGraph,
    target: &str,
    lower: u32,
    upper: Option<u32>,
    provenance: Provenance,
) {
    let member = found_member(graph, "Winner", "value");

    assert_eq!(member.target().raw_type().as_str(), target);
    assert_eq!(member.multiplicity().lower(), lower);
    assert_eq!(member.multiplicity().upper(), upper);
    assert_eq!(member.owner().provenance(), provenance);
    assert_eq!(member.provenance(), provenance);
    assert_eq!(member.owner().definition().source().index(), 1);
    assert_eq!(member.definition().source().index(), 1);
    assert_eq!(graph.diagnostics().len(), 1);
    assert_eq!(graph.diagnostics()[0].code, MODEL_MERGE_CONFLICT);
}

#[test]
fn mixed_source_order_deterministically_selects_the_last_winner() {
    let pmcd = pmcd_source(vec![pmcd_class(
        "Winner",
        &[],
        None,
        vec![pmcd_property("value", "Integer", 1, Some(1))],
        Vec::new(),
    )]);
    let pure = r#"
Class model::Winner
{
  value: String[0..1];
}
"#;

    for _ in 0..MERGE_REPETITIONS {
        let pmcd_then_pure = mixed_graph(&pmcd, pure, true);
        assert_mixed_winner(&pmcd_then_pure, "String", 0, Some(1), Provenance::PureFile);

        let pure_then_pmcd = mixed_graph(&pmcd, pure, false);
        assert_mixed_winner(&pure_then_pmcd, "Integer", 1, Some(1), Provenance::Pmcd);
    }
}
