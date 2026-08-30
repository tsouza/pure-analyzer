//! Local type-environment and navigation-resolution contracts.

#![allow(clippy::disallowed_methods)]

use std::collections::BTreeMap;

use pure_analyzer_diagnostics::{DiagCode, ReasonCode, TextRange};
use pure_analyzer_model::{
    ModelDocument, ModelGraph, Multiplicity, Name, PmcdDocument, PureDocument, QName, QpKind,
    TypeRef, load_model_documents, load_pmcd_documents, load_pure_documents,
};
use pure_analyzer_resolve::{
    LocalValue, LocalValueKind, NavigationResolution, NavigationResolver, NavigationStep,
    NavigationTarget, NavigationUnderResolution, NavigationUnderResolutionReason, RelationRow,
    Resolution, ResolvedMemberKind, Resolver, TypeEnvironment, UnknownValue,
};
use serde_json::{Value, json};

const PACKAGE: &str = "model";
const ZERO: u32 = 0;
const ONE: u32 = 1;
const NO_ARGUMENTS: usize = 0;
const ONE_ARGUMENT: usize = 1;

fn graph(elements: Vec<Value>) -> ModelGraph {
    let source = json!({"_type": "data", "elements": elements}).to_string();
    load_pmcd_documents(&[PmcdDocument::new("local-resolution-fixture", &source)])
        .expect("fixture must load")
}

#[test]
fn arity_mismatch_retains_the_resolved_generated_member() {
    let graph = graph(vec![
        temporal_class("TemporalTarget"),
        class(
            "Source",
            &[],
            Vec::new(),
            vec![generated_property("point", "model::TemporalTarget")],
        ),
    ]);
    let resolver = NavigationResolver::new(&graph);
    let source = class_value(&resolver, "Source");
    let mismatch = resolver.resolve(&source, &[NavigationStep::property(name("point"))]);

    let NavigationResolution::WrongArity(mismatch) = mismatch else {
        panic!("a generated point property without its date must be wrong arity");
    };
    assert!(mismatch.is_generated_milestoned());
}

#[test]
fn generated_milestoning_navigation_to_non_temporal_target_requires_no_dates() {
    let graph = graph(vec![
        class("PlainTarget", &[], Vec::new(), Vec::new()),
        class(
            "Source",
            &[],
            Vec::new(),
            vec![generated_property("point", "model::PlainTarget")],
        ),
    ]);
    let resolver = NavigationResolver::new(&graph);
    let source = class_value(&resolver, "Source");

    found(resolver.resolve(&source, &[NavigationStep::property(name("point"))]));

    let outcome = resolver.resolve(
        &source,
        &[NavigationStep::call(name("point"), ONE_ARGUMENT)],
    );
    let NavigationResolution::WrongArity(mismatch) = outcome else {
        panic!("a non-temporal generated point property must reject explicit dates");
    };
    assert!(mismatch.is_generated_milestoned());
    assert_eq!(mismatch.expected(), NO_ARGUMENTS);
    assert_eq!(mismatch.actual(), ONE_ARGUMENT);
}

fn exact_span(source: &str, declaration: &str) -> TextRange {
    let start = source.find(declaration).expect("declaration occurs once");
    let end = start + declaration.len();
    TextRange::new(
        u32::try_from(start).expect("source fits TextRange").into(),
        u32::try_from(end).expect("source fits TextRange").into(),
    )
}

fn qname(name: &str) -> QName {
    QName::new(format!("{PACKAGE}::{name}")).expect("fixture path must be valid")
}

fn name(value: &str) -> Name {
    Name::new(value).expect("fixture name must be valid")
}

fn type_ref(value: &str) -> TypeRef {
    TypeRef::new(
        QName::new(value).expect("fixture type must be valid"),
        Vec::new(),
    )
}

fn single() -> Multiplicity {
    Multiplicity::new(ONE, Some(ONE)).expect("one must be a valid multiplicity")
}

fn property(name: &str, target: &str) -> Value {
    json!({
        "name": name,
        "genericType": {"rawType": target, "typeArguments": []},
        "multiplicity": {"lowerBound": ZERO, "upperBound": ONE},
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

fn temporal_class(name: &str) -> Value {
    let mut value = class(name, &[], Vec::new(), Vec::new());
    value["stereotypes"] = json!([{
        "profile": "meta::pure::profiles::temporal",
        "value": "processingtemporal",
    }]);
    value
}

fn generated_property(name: &str, target: &str) -> Value {
    json!({
        "name": name,
        "returnGenericType": {"rawType": target, "typeArguments": []},
        "returnMultiplicity": {"lowerBound": ZERO, "upperBound": ONE},
        "stereotypes": [{
            "profile": "meta::pure::profiles::milestoning",
            "value": "generatedmilestoningproperty",
        }],
    })
}

fn user_qualified_property(name: &str, target: &str, parameters: &[&str]) -> Value {
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

fn non_generated_milestoning_property(name: &str, target: &str, parameters: &[&str]) -> Value {
    let mut property = user_qualified_property(name, target, parameters);
    property["stereotypes"] = json!([{
        "profile": "meta::pure::profiles::milestoning",
        "value": "notgenerated",
    }]);
    property
}

fn pure_non_generated_milestoning_graph() -> ModelGraph {
    let source = r#"
Class model::Source
{
  <<milestoning.notgenerated>>
  manualAllVersions(asOf: String[1]): String[0..1] {};
  <<milestoning.notgenerated>>
  manualAllVersionsInRange(asOf: String[1]): String[0..1] {};
}

Class model::GeneratedParent
{
  <<milestoning.generatedmilestoningproperty>>
  manualAllVersions(): String[0..1] {};
  <<milestoning.generatedmilestoningproperty>>
  manualAllVersionsInRange(): String[0..1] {};
}

Class model::ManualParent
{
  <<milestoning.notgenerated>>
  manualAllVersions(asOf: String[1]): String[0..1] {};
  <<milestoning.notgenerated>>
  manualAllVersionsInRange(asOf: String[1]): String[0..1] {};
}

Class model::Child extends model::GeneratedParent, model::ManualParent
{
}
"#;
    load_pure_documents(&[PureDocument::new("pure-milestoning.pure", source)])
        .expect("Pure parity fixture must load")
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

fn class_value(resolver: &NavigationResolver<'_>, class: &str) -> LocalValue {
    match resolver.class_all(&qname(class)) {
        Resolution::Found(value) => value,
        outcome => panic!("expected class value, got {outcome:#?}"),
    }
}

fn found(result: NavigationResolution) -> pure_analyzer_resolve::NavigationChain {
    match result {
        NavigationResolution::Found(chain) => chain,
        outcome => panic!("expected resolved navigation, got {outcome:#?}"),
    }
}

#[test]
fn class_all_tracks_a_zero_or_more_class_value() {
    let graph = graph(vec![class("Person", &[], Vec::new(), Vec::new())]);
    let resolver = NavigationResolver::new(&graph);

    let people = class_value(&resolver, "Person");

    assert_eq!(people.multiplicity(), Multiplicity::zero_or_more());
    assert!(matches!(
        people.kind(),
        LocalValueKind::Class(class) if class.path() == &qname("Person")
    ));
}

#[test]
fn nested_lambda_and_let_scopes_restore_shadowed_bindings() {
    let graph = graph(vec![class("Person", &[], Vec::new(), Vec::new())]);
    let resolver = NavigationResolver::new(&graph);
    let outer_person = class_value(&resolver, "Person");
    let outer_name = name("value");
    let scalar = LocalValue::scalar(type_ref("String"), single());
    let mut environment = TypeEnvironment::new();
    assert!(
        environment
            .bind(outer_name.clone(), outer_person.clone())
            .is_none()
    );

    {
        let mut lambda = environment.scope();
        assert!(lambda.bind(outer_name.clone(), scalar.clone()).is_none());
        assert_eq!(lambda.lookup(&outer_name), Some(&scalar));
        assert_eq!(
            lambda.bind(outer_name.clone(), outer_person.clone()),
            Some(scalar.clone())
        );

        {
            let mut let_scope = lambda.scope();
            assert!(let_scope.bind(outer_name.clone(), scalar.clone()).is_none());
            assert_eq!(let_scope.lookup(&outer_name), Some(&scalar));
        }

        assert_eq!(lambda.lookup(&outer_name), Some(&outer_person));
    }

    assert_eq!(environment.lookup(&outer_name), Some(&outer_person));
}

#[test]
fn navigation_resolves_inherited_members_and_retains_definition_anchors() {
    let graph = graph(vec![
        class("Target", &[], vec![property("name", "String")], Vec::new()),
        class(
            "Base",
            &[],
            vec![property("target", "model::Target")],
            Vec::new(),
        ),
        class("Child", &["model::Base"], Vec::new(), Vec::new()),
    ]);
    let resolver = NavigationResolver::new(&graph);
    let source = class_value(&resolver, "Child");

    let chain = found(resolver.resolve(
        &source,
        &[
            NavigationStep::property(name("target")),
            NavigationStep::property(name("name")),
        ],
    ));

    assert_eq!(chain.hops().len(), 2);
    let NavigationTarget::Member(first) = chain.hops()[0].target() else {
        panic!("first hop must be a model member");
    };
    assert_eq!(first.owner().path(), &qname("Base"));
    assert_eq!(chain.hops()[0].definition(), Some(first.definition()));
    let NavigationTarget::Member(second) = chain.hops()[1].target() else {
        panic!("second hop must be a model member");
    };
    assert_eq!(second.owner().path(), &qname("Target"));
    assert!(matches!(
        chain.value().kind(),
        LocalValueKind::Scalar(scalar) if scalar.raw_type().as_str() == "String"
    ));
}

#[test]
fn same_named_properties_resolve_from_the_current_class_not_globally() {
    let graph = graph(vec![
        class("Left", &[], vec![property("label", "String")], Vec::new()),
        class("Right", &[], vec![property("label", "Integer")], Vec::new()),
    ]);
    let resolver = NavigationResolver::new(&graph);

    let left = found(resolver.resolve(
        &class_value(&resolver, "Left"),
        &[NavigationStep::property(name("label"))],
    ));
    let right = found(resolver.resolve(
        &class_value(&resolver, "Right"),
        &[NavigationStep::property(name("label"))],
    ));

    let NavigationTarget::Member(left_member) = left.hops()[0].target() else {
        panic!("left must resolve a model member");
    };
    let NavigationTarget::Member(right_member) = right.hops()[0].target() else {
        panic!("right must resolve a model member");
    };
    assert_eq!(left_member.owner().path(), &qname("Left"));
    assert_eq!(right_member.owner().path(), &qname("Right"));
    assert!(matches!(
        left.value().kind(),
        LocalValueKind::Scalar(scalar) if scalar.raw_type().as_str() == "String"
    ));
    assert!(matches!(
        right.value().kind(),
        LocalValueKind::Scalar(scalar) if scalar.raw_type().as_str() == "Integer"
    ));
}

#[test]
fn association_navigation_tracks_the_opposite_end() {
    let graph = graph(vec![
        class("Left", &[], Vec::new(), Vec::new()),
        class("Right", &[], Vec::new(), Vec::new()),
        association(
            "Link",
            property("toLeft", "model::Left"),
            property("toRight", "model::Right"),
        ),
    ]);
    let resolver = NavigationResolver::new(&graph);

    let chain = found(resolver.resolve(
        &class_value(&resolver, "Left"),
        &[NavigationStep::property(name("toRight"))],
    ));

    let NavigationTarget::Member(member) = chain.hops()[0].target() else {
        panic!("association navigation must resolve a member");
    };
    assert!(matches!(
        member.kind(),
        pure_analyzer_resolve::ResolvedMemberKind::AssociationEnd { association }
            if association == &qname("Link")
    ));
    assert!(matches!(
        chain.value().kind(),
        LocalValueKind::Class(class) if class.path() == &qname("Right")
    ));
}

#[test]
fn relation_rows_bind_columns_and_require_zero_context_arguments() {
    let graph = graph(Vec::new());
    let resolver = NavigationResolver::new(&graph);
    let rank = LocalValue::scalar(type_ref("Integer"), single());
    let row = LocalValue::relation_row(
        RelationRow::new(BTreeMap::from([(name("rank"), rank.clone())])),
        single(),
    );

    let chain = found(resolver.resolve(&row, &[NavigationStep::property(name("rank"))]));
    assert!(matches!(
        chain.hops()[0].target(),
        NavigationTarget::RelationColumn
    ));
    assert_eq!(chain.value(), &rank);

    let mismatch = resolver.resolve(&row, &[NavigationStep::call(name("rank"), ONE_ARGUMENT)]);
    let NavigationResolution::WrongArity(mismatch) = mismatch else {
        panic!("relation column calls with arguments must be rejected");
    };
    assert_eq!(mismatch.expected(), NO_ARGUMENTS);
    assert_eq!(mismatch.actual(), ONE_ARGUMENT);
    assert_eq!(mismatch.definition(), None);
}

#[test]
fn navigation_failures_retain_ambiguity_cycle_and_member_arity_metadata() {
    let ambiguous_graph = graph(vec![
        class("Left", &[], vec![property("shared", "String")], Vec::new()),
        class(
            "Right",
            &[],
            vec![property("shared", "Integer")],
            Vec::new(),
        ),
        class(
            "Child",
            &["model::Right", "model::Left"],
            Vec::new(),
            Vec::new(),
        ),
    ]);
    let ambiguous_resolver = NavigationResolver::new(&ambiguous_graph);
    let ambiguity = ambiguous_resolver.resolve(
        &class_value(&ambiguous_resolver, "Child"),
        &[NavigationStep::property(name("shared"))],
    );
    let NavigationResolution::Ambiguous(ambiguity) = ambiguity else {
        panic!("equally preferred inherited members must remain ambiguous");
    };
    assert_eq!(
        ambiguity
            .candidates()
            .iter()
            .map(|candidate| candidate.owner().path().as_str())
            .collect::<Vec<_>>(),
        ["model::Left", "model::Right"]
    );
    assert!(ambiguity.failure().completed().hops().is_empty());
    assert_eq!(ambiguity.failure().step().name(), &name("shared"));

    let cycle_graph = graph(vec![
        class("A", &["model::B"], Vec::new(), Vec::new()),
        class("B", &["model::A"], Vec::new(), Vec::new()),
    ]);
    let cycle_resolver = NavigationResolver::new(&cycle_graph);
    let cycle = cycle_resolver.resolve(
        &class_value(&cycle_resolver, "A"),
        &[NavigationStep::property(name("missing"))],
    );
    let NavigationResolution::Cycle(cycle) = cycle else {
        panic!("generalization cycles must remain distinct navigation failures");
    };
    assert_eq!(cycle.cycle(), &[qname("A"), qname("B"), qname("A")]);
    assert!(cycle.failure().completed().hops().is_empty());
    assert_eq!(cycle.failure().step().name(), &name("missing"));

    let arity_graph = graph(vec![class(
        "Source",
        &[],
        Vec::new(),
        vec![user_qualified_property("byKey", "String", &["String"])],
    )]);
    let arity_resolver = NavigationResolver::new(&arity_graph);
    let arity = arity_resolver.resolve(
        &class_value(&arity_resolver, "Source"),
        &[NavigationStep::property(name("byKey"))],
    );
    let NavigationResolution::WrongArity(arity) = arity else {
        panic!("user qualified properties require their declared arguments");
    };
    assert_eq!(arity.expected(), ONE_ARGUMENT);
    assert_eq!(arity.actual(), NO_ARGUMENTS);
    assert!(arity.definition().is_some());
    assert!(arity.failure().completed().hops().is_empty());
    assert_eq!(arity.failure().step().name(), &name("byKey"));
}

#[test]
fn wrong_arity_navigation_anchors_the_winning_cross_source_definition() {
    let pure_member = "query(key: String[1]): String[1] {};";
    let pure_source = format!("Class model::Source\n{{\n  {pure_member}\n}}");
    let pmcd = json!({
        "_type": "data",
        "elements": [class(
            "Source",
            &[],
            Vec::new(),
            vec![user_qualified_property("query", "Integer", &["Integer"])],
        )]
    })
    .to_string();

    let pmcd_winner = load_model_documents(&[
        ModelDocument::Pure(PureDocument::new("first.pure", &pure_source)),
        ModelDocument::Pmcd(PmcdDocument::new("second.json", &pmcd)),
    ])
    .expect("mixed model must load");
    let pmcd_resolver = NavigationResolver::new(&pmcd_winner);
    let NavigationResolution::WrongArity(pmcd_mismatch) = pmcd_resolver.resolve(
        &class_value(&pmcd_resolver, "Source"),
        &[NavigationStep::property(name("query"))],
    ) else {
        panic!("the winning qualified property requires one argument");
    };
    assert_eq!(pmcd_mismatch.expected(), ONE_ARGUMENT);
    assert_eq!(pmcd_mismatch.actual(), NO_ARGUMENTS);
    let pmcd_anchor = pmcd_mismatch
        .definition()
        .expect("a member arity mismatch retains its definition");
    assert_eq!(pmcd_anchor.source().index(), 1);
    assert_eq!(pmcd_anchor.span(), None);

    let pure_winner = load_model_documents(&[
        ModelDocument::Pmcd(PmcdDocument::new("first.json", &pmcd)),
        ModelDocument::Pure(PureDocument::new("second.pure", &pure_source)),
    ])
    .expect("mixed model must load");
    let pure_resolver = NavigationResolver::new(&pure_winner);
    let NavigationResolution::WrongArity(pure_mismatch) = pure_resolver.resolve(
        &class_value(&pure_resolver, "Source"),
        &[NavigationStep::property(name("query"))],
    ) else {
        panic!("the winning qualified property requires one argument");
    };
    assert_eq!(pure_mismatch.expected(), ONE_ARGUMENT);
    assert_eq!(pure_mismatch.actual(), NO_ARGUMENTS);
    let pure_anchor = pure_mismatch
        .definition()
        .expect("a member arity mismatch retains its definition");
    assert_eq!(pure_anchor.source().index(), 1);
    assert_eq!(
        pure_anchor.span(),
        Some(exact_span(&pure_source, pure_member))
    );
}

#[test]
fn intrinsic_scalars_do_not_turn_unmodeled_types_into_scalars() {
    let graph = graph(vec![class(
        "Source",
        &[],
        vec![
            property("builtIn", "String"),
            property("external", "vendor::Unmodeled"),
        ],
        Vec::new(),
    )]);
    let resolver = NavigationResolver::new(&graph);
    let source = class_value(&resolver, "Source");

    let built_in = found(resolver.resolve(&source, &[NavigationStep::property(name("builtIn"))]));
    assert!(matches!(
        built_in.value().kind(),
        LocalValueKind::Scalar(value) if value.raw_type().as_str() == "String"
    ));

    let external = found(resolver.resolve(&source, &[NavigationStep::property(name("external"))]));
    assert!(matches!(
        external.value().kind(),
        LocalValueKind::Unknown(UnknownValue::UnmodeledType(value))
            if value.raw_type().as_str() == "vendor::Unmodeled"
    ));
}

#[test]
fn each_navigation_hop_uses_its_own_milestoning_context() {
    let graph = graph(vec![
        temporal_class("TemporalTarget"),
        class(
            "Middle",
            &[],
            Vec::new(),
            vec![generated_property("point", "model::TemporalTarget")],
        ),
        class(
            "Start",
            &[],
            vec![property("middle", "model::Middle")],
            Vec::new(),
        ),
    ]);
    let resolver = NavigationResolver::new(&graph);
    let source = class_value(&resolver, "Start");

    let mismatch = resolver.resolve(
        &source,
        &[
            NavigationStep::call(name("middle"), ONE_ARGUMENT),
            NavigationStep::call(name("point"), ONE_ARGUMENT),
        ],
    );
    let NavigationResolution::WrongArity(mismatch) = mismatch else {
        panic!("non-milestoned first hop must require zero arguments");
    };
    assert_eq!(mismatch.expected(), NO_ARGUMENTS);
    assert_eq!(mismatch.actual(), ONE_ARGUMENT);
    assert!(mismatch.failure().completed().hops().is_empty());

    let chain = found(resolver.resolve(
        &source,
        &[
            NavigationStep::property(name("middle")),
            NavigationStep::call(name("point"), ONE_ARGUMENT),
        ],
    ));
    assert_eq!(chain.hops().len(), 2);
    let NavigationTarget::Member(member) = chain.hops()[1].target() else {
        panic!("point hop must resolve a model member");
    };
    assert_eq!(
        member.kind(),
        &pure_analyzer_resolve::ResolvedMemberKind::Qualified(QpKind::MilestonedPoint)
    );
}

#[test]
fn user_qualified_properties_and_generated_navigation_have_distinct_arity_gates() {
    let graph = graph(vec![
        temporal_class("TemporalTarget"),
        class(
            "Source",
            &[],
            Vec::new(),
            vec![
                user_qualified_property("byKey", "String", &["String"]),
                generated_property("point", "model::TemporalTarget"),
            ],
        ),
    ]);
    let resolver = NavigationResolver::new(&graph);
    let source = class_value(&resolver, "Source");

    let user = found(resolver.resolve(
        &source,
        &[NavigationStep::call(name("byKey"), ONE_ARGUMENT)],
    ));
    assert!(matches!(
        user.value().kind(),
        LocalValueKind::Scalar(scalar) if scalar.raw_type().as_str() == "String"
    ));
    let generated = found(resolver.resolve(
        &source,
        &[NavigationStep::call(name("point"), ONE_ARGUMENT)],
    ));
    assert!(matches!(
        generated.value().kind(),
        LocalValueKind::Class(class) if class.path() == &qname("TemporalTarget")
    ));

    for step in [
        NavigationStep::property(name("byKey")),
        NavigationStep::property(name("point")),
    ] {
        let outcome = resolver.resolve(&source, &[step]);
        assert!(matches!(outcome, NavigationResolution::WrongArity(_)));
    }
}

#[test]
fn non_generated_milestoning_suffixes_use_their_declared_signatures() {
    let graph = graph(vec![class(
        "Source",
        &[],
        Vec::new(),
        vec![
            non_generated_milestoning_property("manualAllVersions", "String", &["String"]),
            non_generated_milestoning_property("manualAllVersionsInRange", "String", &["String"]),
        ],
    )]);
    let resolver = NavigationResolver::new(&graph);
    let source = class_value(&resolver, "Source");

    for member_name in ["manualAllVersions", "manualAllVersionsInRange"] {
        let chain = found(resolver.resolve(
            &source,
            &[NavigationStep::call(name(member_name), ONE_ARGUMENT)],
        ));
        let NavigationTarget::Member(member) = chain.hops()[0].target() else {
            panic!("{member_name} must resolve a model member");
        };
        assert_eq!(
            member.kind(),
            &pure_analyzer_resolve::ResolvedMemberKind::Qualified(QpKind::UserQualified)
        );

        let mismatch = resolver.resolve(&source, &[NavigationStep::property(name(member_name))]);
        let NavigationResolution::WrongArity(mismatch) = mismatch else {
            panic!("{member_name} must require its declared argument");
        };
        assert_eq!(mismatch.expected(), ONE_ARGUMENT);
        assert_eq!(mismatch.actual(), NO_ARGUMENTS);
    }
}

#[test]
fn non_generated_milestoning_suffixes_match_between_pmcd_and_pure() {
    let pmcd_graph = graph(vec![
        class(
            "Source",
            &[],
            Vec::new(),
            vec![
                non_generated_milestoning_property("manualAllVersions", "String", &["String"]),
                non_generated_milestoning_property(
                    "manualAllVersionsInRange",
                    "String",
                    &["String"],
                ),
            ],
        ),
        class(
            "GeneratedParent",
            &[],
            Vec::new(),
            vec![
                generated_property("manualAllVersions", "String"),
                generated_property("manualAllVersionsInRange", "String"),
            ],
        ),
        class(
            "ManualParent",
            &[],
            Vec::new(),
            vec![
                non_generated_milestoning_property("manualAllVersions", "String", &["String"]),
                non_generated_milestoning_property(
                    "manualAllVersionsInRange",
                    "String",
                    &["String"],
                ),
            ],
        ),
        class(
            "Child",
            &["model::GeneratedParent", "model::ManualParent"],
            Vec::new(),
            Vec::new(),
        ),
    ]);
    let pure_graph = pure_non_generated_milestoning_graph();

    for member_name in ["manualAllVersions", "manualAllVersionsInRange"] {
        let pmcd = &pmcd_graph
            .class("model::Source")
            .expect("PMCD source class")
            .qualified_properties()[member_name];
        let pure = &pure_graph
            .class("model::Source")
            .expect("Pure source class")
            .qualified_properties()[member_name];

        assert_eq!(pmcd.kind(), QpKind::UserQualified, "PMCD {member_name}");
        assert_eq!(pure.kind(), QpKind::UserQualified, "Pure {member_name}");

        let pmcd_signature = pmcd.signature().expect("PMCD declared signature");
        let pure_signature = pure.signature().expect("Pure declared signature");
        assert_eq!(pmcd_signature.len(), ONE_ARGUMENT, "PMCD {member_name}");
        assert_eq!(
            pmcd_signature[NO_ARGUMENTS].raw_type().as_str(),
            "String",
            "PMCD {member_name}"
        );
        assert_eq!(
            pure_signature, pmcd_signature,
            "signature for {member_name}"
        );
    }

    for (loader, graph) in [("PMCD", &pmcd_graph), ("Pure", &pure_graph)] {
        let navigation = NavigationResolver::new(graph);
        let source = class_value(&navigation, "Source");

        for member_name in ["manualAllVersions", "manualAllVersionsInRange"] {
            let chain = found(navigation.resolve(
                &source,
                &[NavigationStep::call(name(member_name), ONE_ARGUMENT)],
            ));
            let NavigationTarget::Member(member) = chain.hops()[NO_ARGUMENTS].target() else {
                panic!("{loader} {member_name} must resolve a model member");
            };
            assert_eq!(
                member.kind(),
                &ResolvedMemberKind::Qualified(QpKind::UserQualified),
                "{loader} {member_name} call"
            );

            let outcome =
                navigation.resolve(&source, &[NavigationStep::property(name(member_name))]);
            let NavigationResolution::WrongArity(mismatch) = outcome else {
                panic!("{loader} {member_name} property access must reject its missing argument");
            };
            assert_eq!(mismatch.expected(), ONE_ARGUMENT, "{loader} {member_name}");
            assert_eq!(mismatch.actual(), NO_ARGUMENTS, "{loader} {member_name}");
        }

        let resolver = Resolver::new(graph);
        for (member_name, generated_kind) in [
            ("manualAllVersions", QpKind::AllVersions),
            ("manualAllVersionsInRange", QpKind::AllVersionsInRange),
        ] {
            let Resolution::Found(member) =
                resolver.resolve_member(&qname("Child"), &name(member_name))
            else {
                panic!("{loader} {member_name} must resolve the generated parent member");
            };
            assert_eq!(
                member.owner().path().as_str(),
                "model::GeneratedParent",
                "{loader} {member_name} precedence"
            );
            assert_eq!(
                member.kind(),
                &ResolvedMemberKind::Qualified(generated_kind),
                "{loader} {member_name} precedence"
            );
        }
    }
}

#[test]
fn unknown_higher_order_flows_emit_typed_under_resolution() {
    let graph = graph(Vec::new());
    let resolver = NavigationResolver::new(&graph);
    let mut environment = TypeEnvironment::new();
    let flow = name("flow");
    let unknown = LocalValue::unknown(UnknownValue::HigherOrder, Multiplicity::zero_or_more());
    assert!(environment.bind(flow.clone(), unknown).is_none());

    let outcome = resolver.resolve_variable(
        &environment,
        &flow,
        &[NavigationStep::property(name("name"))],
    );
    let NavigationResolution::UnderResolved(under_resolution) = outcome else {
        panic!("higher-order flow must not become a hard unknown-property result");
    };
    assert_eq!(under_resolution.diagnostic_code(), DiagCode::UnknownSource);
    assert_eq!(under_resolution.reason_code(), ReasonCode::ModelIncomplete);
    let NavigationUnderResolution::AtStep { failure, reason } = under_resolution else {
        panic!("under-resolution must retain its failed step");
    };
    assert_eq!(
        reason.as_ref(),
        &NavigationUnderResolutionReason::UnknownValue(UnknownValue::HigherOrder)
    );
    assert_eq!(failure.step().name(), &name("name"));
}
