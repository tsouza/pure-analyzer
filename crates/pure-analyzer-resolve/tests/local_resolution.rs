//! Local type-environment and navigation-resolution contracts.

#![allow(clippy::disallowed_methods)]

use std::collections::BTreeMap;

use pure_analyzer_diagnostics::{DiagCode, ReasonCode};
use pure_analyzer_model::{
    ModelGraph, Multiplicity, Name, PmcdDocument, QName, QpKind, TypeRef, load_pmcd_documents,
};
use pure_analyzer_resolve::{
    LocalValue, LocalValueKind, NavigationResolution, NavigationResolver, NavigationStep,
    NavigationTarget, NavigationUnderResolution, NavigationUnderResolutionReason, RelationRow,
    Resolution, TypeEnvironment, UnknownValue,
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
