//! End-to-end contracts for conservative resolved-query lowering.
#![allow(clippy::disallowed_methods)]

use proptest::prelude::*;
use pure_analyzer_analysis::{
    AnalysisInput, ColumnId, ModelOrigin, Nullability, RelationOperator, RelationSource,
    RelationalOutcome, ScalarLiteral, ScalarOperator, SourceSpan, lower_m3_query,
};
use pure_analyzer_diagnostics::{FileId, ReasonCode};
use pure_analyzer_model::{
    ModelGraph, Name, PmcdDocument, PureDocument, QName, load_pmcd_documents, load_pure_documents,
};
use pure_analyzer_parser::parse_query;
use pure_analyzer_resolve::{Resolution, ResolvedClass, ResolvedMember, Resolver};
use serde_json::{Value, json};

const PACKAGE: &str = "model";
const TEST_FILE: u32 = 73;
const ZERO: u32 = 0;
const ONE: u32 = 1;

fn graph(elements: Vec<Value>) -> ModelGraph {
    let source = json!({"_type": "data", "elements": elements}).to_string();
    load_pmcd_documents(&[PmcdDocument::new("relational-lowering-fixture", &source)])
        .expect("fixture model must load")
}

fn pure_graph(source: &str) -> ModelGraph {
    load_pure_documents(&[PureDocument::new(
        "relational-lowering-fixture.pure",
        source,
    )])
    .expect("fixture Pure source must load")
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

fn property(name: &str, target: &str, lower: u32, upper: Option<u32>) -> Value {
    json!({
        "name": name,
        "genericType": {"rawType": target, "typeArguments": []},
        "multiplicity": {"lowerBound": lower, "upperBound": upper},
    })
}

fn lower(source: &str, model: Option<&ModelGraph>) -> RelationalOutcome {
    let parsed = parse_query(source, FileId::new(TEST_FILE)).expect("fixture source must build");
    lower_m3_query(AnalysisInput::new(
        FileId::new(TEST_FILE),
        source,
        &parsed.green,
        &parsed.diagnostics,
        model,
    ))
}

fn supported(outcome: RelationalOutcome) -> Box<pure_analyzer_analysis::RelationalQuery> {
    let RelationalOutcome::Supported(query) = outcome else {
        panic!("expected supported relational query, got {outcome:#?}");
    };
    query
}

fn opaque(outcome: RelationalOutcome) -> pure_analyzer_analysis::OpaqueOutcome {
    let RelationalOutcome::Opaque(value) = outcome else {
        panic!("expected opaque outcome, got {outcome:#?}");
    };
    value
}

fn assert_reason(outcome: RelationalOutcome, reason: ReasonCode) {
    assert_eq!(opaque(outcome).reason(), reason);
}

fn span_text(source: &str, span: SourceSpan) -> &str {
    &source[usize::from(span.range().start())..usize::from(span.range().end())]
}

fn map_parts(
    query: &pure_analyzer_analysis::RelationalQuery,
) -> (
    &pure_analyzer_analysis::RelationExpression,
    &pure_analyzer_analysis::Projection,
) {
    match query.root().operator() {
        RelationOperator::Project { input, projections } if projections.len() == 1 => {
            (input, &projections[0])
        }
        other => panic!("expected one map project, got {other:#?}"),
    }
}

fn filter_parts(
    expression: &pure_analyzer_analysis::RelationExpression,
) -> (
    &pure_analyzer_analysis::RelationExpression,
    &pure_analyzer_analysis::ScalarExpression,
) {
    match expression.operator() {
        RelationOperator::Filter { input, predicate } => (input, predicate),
        other => panic!("expected filter, got {other:#?}"),
    }
}

fn class_scan(expression: &pure_analyzer_analysis::RelationExpression) -> &ResolvedClass {
    match expression.operator() {
        RelationOperator::Scan(RelationSource::Class(class)) => class,
        other => panic!("expected class scan, got {other:#?}"),
    }
}

fn navigation_parts(
    expression: &pure_analyzer_analysis::ScalarExpression,
) -> (&pure_analyzer_analysis::ScalarExpression, &ResolvedMember) {
    match expression.operator() {
        ScalarOperator::Navigation { input, navigation } => (input, navigation.member()),
        other => panic!("expected correlated navigation, got {other:#?}"),
    }
}

fn equality_parts(
    expression: &pure_analyzer_analysis::ScalarExpression,
) -> (
    &pure_analyzer_analysis::ScalarExpression,
    &pure_analyzer_analysis::ScalarExpression,
) {
    match expression.operator() {
        ScalarOperator::Equal { left, right } => (left, right),
        other => panic!("expected equality predicate, got {other:#?}"),
    }
}

const FILTER_MAP_SOURCE: &str =
    "model::Person.all()->filter(x| $x.name == 'Ada')->map(x| $x.manager)";

fn filter_map_model() -> ModelGraph {
    graph(vec![
        class(
            "Person",
            vec![
                property("name", "String", ONE, Some(ONE)),
                property("manager", "model::Manager", ZERO, Some(ONE)),
            ],
        ),
        class("Manager", Vec::new()),
    ])
}

#[test]
fn lowers_map_as_a_correlated_single_column_project() {
    let model = filter_map_model();
    let query = supported(lower(FILTER_MAP_SOURCE, Some(&model)));

    let (_, projection) = map_parts(&query);
    assert_eq!(
        span_text(FILTER_MAP_SOURCE, query.root().origin().source()),
        "->map(x| $x.manager)"
    );
    assert_eq!(query.output().columns().len(), 1);
    assert_eq!(query.output().columns()[0].id(), ColumnId::new(ONE));
    assert_eq!(query.output().columns()[0].name().as_str(), "value");
    assert_eq!(
        query.output().columns()[0].nullability(),
        Nullability::Unknown
    );

    let (manager_input, manager) = navigation_parts(projection.expression());
    assert_eq!(
        span_text(FILTER_MAP_SOURCE, projection.expression().origin().source()),
        ".manager"
    );
    assert_eq!(manager.owner().path().as_str(), "model::Person");
    assert!(
        matches!(manager_input.operator(), ScalarOperator::Column(id) if *id == ColumnId::new(0))
    );
}

#[test]
fn lowers_filter_to_an_equality_predicate() {
    let model = filter_map_model();
    let query = supported(lower(FILTER_MAP_SOURCE, Some(&model)));
    let (filter, _) = map_parts(&query);
    let (scan, predicate) = filter_parts(filter);
    assert_eq!(
        span_text(FILTER_MAP_SOURCE, filter.origin().source()),
        "->filter(x| $x.name == 'Ada')"
    );
    let (left, right) = equality_parts(predicate);
    assert_eq!(
        span_text(FILTER_MAP_SOURCE, predicate.origin().source()),
        " $x.name == 'Ada'"
    );
    assert!(matches!(
        left.operator(),
        ScalarOperator::Navigation { navigation, .. }
            if navigation.member().owner().path().as_str() == "model::Person"
    ));
    assert!(
        matches!(right.operator(), ScalarOperator::Literal(ScalarLiteral::String(value)) if value == "Ada")
    );
    assert_eq!(
        span_text(FILTER_MAP_SOURCE, scan.origin().source()),
        "model::Person.all()"
    );
}

#[test]
fn preserves_scan_and_navigation_model_provenance() {
    let model = filter_map_model();
    let query = supported(lower(FILTER_MAP_SOURCE, Some(&model)));
    let (filter, projection) = map_parts(&query);
    let (scan, _) = filter_parts(filter);
    let class = class_scan(scan);
    assert_eq!(
        span_text(FILTER_MAP_SOURCE, scan.origin().source()),
        "model::Person.all()"
    );
    assert_eq!(scan.schema().columns().len(), 1);
    assert_eq!(scan.schema().columns()[0].id(), ColumnId::new(0));
    assert_eq!(scan.schema().columns()[0].name().as_str(), "Person");
    assert_eq!(
        scan.schema().columns()[0].nullability(),
        Nullability::Unknown
    );
    assert_eq!(
        scan.schema().columns()[0].origin().model_origins(),
        &[ModelOrigin::from_class(class)]
    );
    let (_, manager) = navigation_parts(projection.expression());
    let manager_class = match Resolver::new(&model).resolve_class(manager.target().raw_type()) {
        Resolution::Found(class) => class,
        other => panic!("fixture manager class must resolve, got {other:#?}"),
    };
    assert_eq!(
        projection.expression().origin().model_origins(),
        &[
            ModelOrigin::from_class(class),
            ModelOrigin::from_member(manager),
            ModelOrigin::from_class(&manager_class),
        ]
    );
}

#[test]
fn lowers_direct_navigation_as_a_project_not_a_member_scan() {
    let model = graph(vec![
        class(
            "Person",
            vec![property("manager", "model::Manager", ZERO, Some(ONE))],
        ),
        class("Manager", Vec::new()),
    ]);
    let source = "model::Person.all().manager";
    let query = supported(lower(source, Some(&model)));

    let RelationOperator::Project { input, projections } = query.root().operator() else {
        panic!("direct navigation must lower through project");
    };
    assert_eq!(query.output().columns()[0].id(), ColumnId::new(ONE));
    assert_eq!(query.output().columns()[0].name().as_str(), "manager");
    assert!(matches!(
        projections[0].expression().operator(),
        ScalarOperator::Navigation { input, .. }
            if matches!(input.operator(), ScalarOperator::Column(id) if *id == ColumnId::new(0))
    ));
    assert!(matches!(
        input.operator(),
        RelationOperator::Scan(RelationSource::Class(_))
    ));
}

#[test]
fn composes_optional_receiver_multiplicity_through_navigation_chain() {
    let model = graph(vec![
        class(
            "Person",
            vec![property("manager", "model::Manager", ZERO, Some(ONE))],
        ),
        class(
            "Manager",
            vec![property("team", "model::Team", ONE, Some(ONE))],
        ),
        class("Team", vec![property("name", "String", ONE, Some(ONE))]),
    ]);
    let query = supported(lower(
        "model::Person.all()->map(x| $x.manager.team.name)",
        Some(&model),
    ));
    let RelationOperator::Project { projections, .. } = query.root().operator() else {
        panic!("map must lower to project");
    };
    let scalar = projections[0].expression();
    assert_eq!(scalar.multiplicity().lower(), ZERO);
    assert_eq!(scalar.multiplicity().upper(), Some(ONE));
    let ScalarOperator::Navigation { input, .. } = scalar.operator() else {
        panic!("expected outer navigation");
    };
    assert_eq!(input.multiplicity().lower(), ZERO);
    assert_eq!(input.multiplicity().upper(), Some(ONE));
}

#[test]
fn lowers_chained_navigation_from_source_backed_model_with_exact_provenance() {
    let model = pure_graph(
        r#"
            Class model::Person {
                manager: model::Manager[0..1];
            }
            Class model::Manager {
                team: model::Team[1];
            }
            Class model::Team {
                name: String[1];
            }
        "#,
    );
    let source = "model::Person.all()->map(x| $x.manager.team.name)";
    let query = supported(lower(source, Some(&model)));
    let RelationOperator::Project { projections, .. } = query.root().operator() else {
        panic!("map must lower to project");
    };
    let resolver = Resolver::new(&model);
    let class = |path: &str| {
        let path = QName::new(path).expect("fixture path must be valid");
        match resolver.resolve_class(&path) {
            Resolution::Found(class) => class,
            other => panic!("fixture class must resolve, got {other:#?}"),
        }
    };
    let member = |owner: &ResolvedClass, name: &str| {
        let name = Name::new(name).expect("fixture name must be valid");
        match resolver.resolve_member(owner.path(), &name) {
            Resolution::Found(member) => member,
            other => panic!("fixture member must resolve, got {other:#?}"),
        }
    };
    let person = class("model::Person");
    let manager = class("model::Manager");
    let team = class("model::Team");
    let manager_member = member(&person, "manager");
    let team_member = member(&manager, "team");
    let name_member = member(&team, "name");

    assert_eq!(
        projections[0].expression().origin().model_origins(),
        &[
            ModelOrigin::from_class(&person),
            ModelOrigin::from_member(&manager_member),
            ModelOrigin::from_class(&manager),
            ModelOrigin::from_member(&team_member),
            ModelOrigin::from_class(&team),
            ModelOrigin::from_member(&name_member),
        ]
    );
}

#[test]
fn pmcd_model_origins_do_not_collapse_same_source_definitions() {
    let model = graph(vec![
        class(
            "Person",
            vec![
                property("manager", "model::Manager", ONE, Some(ONE)),
                property("mentor", "model::Manager", ONE, Some(ONE)),
            ],
        ),
        class("Manager", Vec::new()),
    ]);
    let resolver = Resolver::new(&model);
    let person_path = QName::new("model::Person").expect("fixture path must be valid");
    let manager_path = QName::new("model::Manager").expect("fixture path must be valid");
    let person = match resolver.resolve_class(&person_path) {
        Resolution::Found(class) => class,
        other => panic!("fixture Person class must resolve, got {other:#?}"),
    };
    let manager = match resolver.resolve_class(&manager_path) {
        Resolution::Found(class) => class,
        other => panic!("fixture Manager class must resolve, got {other:#?}"),
    };
    let member = |name: &str| {
        let name = Name::new(name).expect("fixture name must be valid");
        match resolver.resolve_member(&person_path, &name) {
            Resolution::Found(member) => member,
            other => panic!("fixture member must resolve, got {other:#?}"),
        }
    };

    assert_ne!(
        ModelOrigin::from_class(&person),
        ModelOrigin::from_class(&manager)
    );
    assert_ne!(
        ModelOrigin::from_member(&member("manager")),
        ModelOrigin::from_member(&member("mentor"))
    );
}

#[test]
fn preserves_conservative_outcomes_for_out_of_core_or_incomplete_inputs() {
    let model = graph(vec![class(
        "Person",
        vec![
            property("name", "String", ONE, Some(ONE)),
            property("reports", "model::Person", ZERO, None),
        ],
    )]);

    assert_reason(
        lower("model::Person.all()->filter(x| $x.name)", Some(&model)),
        ReasonCode::IndOpaquePredicate,
    );
    assert_reason(
        lower("model::Person.all()->map(x| $x.reports)", Some(&model)),
        ReasonCode::IndUnmodeledOp,
    );
    assert_reason(
        lower("model::Person.all()->map(x| $x.missing)", Some(&model)),
        ReasonCode::IndUnresolvedSchema,
    );
    assert_reason(
        lower("model::Person.all()->filter(x| $x.name ==)", Some(&model)),
        ReasonCode::IndUnparseable,
    );
    assert_reason(
        lower("model::Person.all(); model::Person.all()", Some(&model)),
        ReasonCode::IndUnmodeledOp,
    );
    assert_reason(
        lower("model::Person.all()", None),
        ReasonCode::ModelIncomplete,
    );
}

#[test]
fn opaque_navigation_retains_the_unsupported_span_and_resolved_model_origins() {
    let source = "model::Person.all()->map(x| $x.reports)";
    let model = graph(vec![class(
        "Person",
        vec![property("reports", "model::Person", ZERO, None)],
    )]);
    let outcome = opaque(lower(source, Some(&model)));
    let resolver = Resolver::new(&model);
    let class_path = QName::new("model::Person").expect("fixture path must be valid");
    let class = match resolver.resolve_class(&class_path) {
        Resolution::Found(class) => class,
        other => panic!("fixture class must resolve, got {other:#?}"),
    };
    let member_name = Name::new("reports").expect("fixture name must be valid");
    let member = match resolver.resolve_member(&class_path, &member_name) {
        Resolution::Found(member) => member,
        other => panic!("fixture member must resolve, got {other:#?}"),
    };

    assert_eq!(outcome.reason(), ReasonCode::IndUnmodeledOp);
    assert_eq!(span_text(source, outcome.origin().source()), ".reports");
    assert_eq!(
        outcome.origin().model_origins(),
        &[
            ModelOrigin::from_class(&class),
            ModelOrigin::from_member(&member)
        ]
    );
}

#[test]
fn rejects_property_calls_captures_lets_and_nested_pipelines_without_approximation() {
    let model = graph(vec![
        class(
            "Person",
            vec![
                property("name", "String", ONE, Some(ONE)),
                property("manager", "model::Manager", ZERO, Some(ONE)),
            ],
        ),
        class("Manager", vec![property("name", "String", ONE, Some(ONE))]),
    ]);

    assert_reason(
        lower("model::Person.all()->filter(x| $x.name())", Some(&model)),
        ReasonCode::IndOpaquePredicate,
    );
    let capture_source = "model::Person.all()->filter(x| $captured.name)";
    let capture = opaque(lower(capture_source, Some(&model)));
    assert_eq!(capture.reason(), ReasonCode::IndUnresolvedSchema);
    assert_eq!(
        span_text(capture_source, capture.origin().source()),
        "$captured"
    );
    assert_reason(
        lower(
            "model::Person.all()->filter(x| let y = $x.manager; $y.name)",
            Some(&model),
        ),
        ReasonCode::IndUnmodeledOp,
    );
    assert_reason(
        lower(
            "model::Person.all()->filter(x| $x.manager->filter(y| $y.name); $x.name)",
            Some(&model),
        ),
        ReasonCode::IndUnmodeledOp,
    );
}

fn supported_pipeline_graph() -> ModelGraph {
    graph(vec![class(
        "Node",
        vec![
            property("next", "model::Node", ONE, Some(ONE)),
            property("flag", "Boolean", ONE, Some(ONE)),
        ],
    )])
}

fn supported_pipeline(steps: &[bool]) -> String {
    let mut source = "model::Node.all()".to_owned();
    for step in steps {
        if *step {
            source.push_str("->map(x| $x.next)");
        } else {
            source.push_str("->filter(x| $x.flag == true)");
        }
    }
    source
}

fn project_ids(expression: &pure_analyzer_analysis::RelationExpression, ids: &mut Vec<ColumnId>) {
    match expression.operator() {
        RelationOperator::Scan(_) => {}
        RelationOperator::Filter { input, .. } | RelationOperator::Distinct { input } => {
            project_ids(input, ids);
        }
        RelationOperator::Project { input, projections } => {
            project_ids(input, ids);
            ids.extend(projections.iter().map(|projection| projection.column()));
        }
        RelationOperator::Join { left, right, .. } => {
            project_ids(left, ids);
            project_ids(right, ids);
        }
    }
}

proptest! {
    #[test]
    fn lowering_is_deterministic_for_bounded_supported_pipelines(
        steps in proptest::collection::vec(any::<bool>(), 0..=8),
    ) {
        let model = supported_pipeline_graph();
        let source = supported_pipeline(&steps);
        let first = lower(&source, Some(&model));
        let second = lower(&source, Some(&model));

        prop_assert_eq!(&first, &second);
        let RelationalOutcome::Supported(query) = first else {
            prop_assert!(false, "supported pipeline lowered opaque: {first:#?}");
            return Ok(());
        };
        prop_assert_eq!(query.output().columns().len(), 1);
        let mut ids = Vec::new();
        project_ids(query.root(), &mut ids);
        let expected = (ONE..=u32::try_from(steps.iter().filter(|step| **step).count()).unwrap_or(ZERO))
            .map(ColumnId::new)
            .collect::<Vec<_>>();
        prop_assert_eq!(ids, expected);
    }
}
