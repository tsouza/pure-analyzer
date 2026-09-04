//! End-to-end contracts for conservative resolved-query lowering.
#![allow(clippy::disallowed_methods)]

use proptest::prelude::*;
use pure_analyzer_analysis::{
    AnalysisInput, ColumnId, JoinKind, ModelOrigin, Nullability, ProjectionKind, RelationOperator,
    RelationSource, RelationalOutcome, RowSemantics, ScalarLiteral, ScalarOperator, SortDirection,
    SourceSpan, lower_m3_query,
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
        RelationOperator::Project {
            input,
            projections,
            kind,
        } if projections.len() == 1 => {
            assert_eq!(
                *kind,
                ProjectionKind::Scalar,
                "a `->map`/property-navigation result must lower as ProjectionKind::Scalar"
            );
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

fn distinct_input(
    expression: &pure_analyzer_analysis::RelationExpression,
) -> &pure_analyzer_analysis::RelationExpression {
    match expression.operator() {
        RelationOperator::Distinct { input } => input,
        other => panic!("expected distinct, got {other:#?}"),
    }
}

fn selected_distinct_parts(
    expression: &pure_analyzer_analysis::RelationExpression,
) -> (
    &pure_analyzer_analysis::RelationExpression,
    &[pure_analyzer_analysis::ColumnId],
) {
    match expression.operator() {
        RelationOperator::DistinctOn { input, columns } => (input, columns),
        other => panic!("expected selected distinct, got {other:#?}"),
    }
}

fn join_parts(
    expression: &pure_analyzer_analysis::RelationExpression,
) -> (
    &pure_analyzer_analysis::RelationExpression,
    &pure_analyzer_analysis::RelationExpression,
    &pure_analyzer_analysis::ScalarExpression,
) {
    match expression.operator() {
        RelationOperator::Join {
            kind: JoinKind::Inner,
            left,
            right,
            condition,
        } => (left, right, condition),
        other => panic!("expected an inner join, got {other:#?}"),
    }
}

fn sort_parts(
    expression: &pure_analyzer_analysis::RelationExpression,
) -> (
    &pure_analyzer_analysis::RelationExpression,
    &[pure_analyzer_analysis::SortKey],
) {
    match expression.operator() {
        RelationOperator::Sort { input, keys } => (input, keys),
        other => panic!("expected sort, got {other:#?}"),
    }
}

fn class_scan(expression: &pure_analyzer_analysis::RelationExpression) -> &ResolvedClass {
    match expression.operator() {
        RelationOperator::Scan(RelationSource::Class(class)) => class,
        other => panic!("expected class scan, got {other:#?}"),
    }
}

fn resolved_class(model: &ModelGraph, path: &str) -> ResolvedClass {
    let path = QName::new(path).expect("fixture path must be valid");
    match Resolver::new(model).resolve_class(&path) {
        Resolution::Found(class) => class,
        other => panic!("fixture class must resolve, got {other:#?}"),
    }
}

fn resolved_member(model: &ModelGraph, owner: &ResolvedClass, name: &str) -> ResolvedMember {
    let name = Name::new(name).expect("fixture name must be valid");
    match Resolver::new(model).resolve_member(owner.path(), &name) {
        Resolution::Found(member) => member,
        other => panic!("fixture member must resolve, got {other:#?}"),
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
fn declines_non_equality_binary_operators_without_reinterpreting_them() {
    let model = filter_map_model();

    for operator in ["<", "<=", ">", ">=", "+", "-", "*", "/"] {
        let source = format!("model::Person.all()->filter(x| $x.name {operator} 'Ada')");
        assert_reason(lower(&source, Some(&model)), ReasonCode::IndOpaquePredicate);
    }
}

#[test]
fn lowers_false_as_a_boolean_literal() {
    let model = filter_map_model();
    let query = supported(lower("model::Person.all()->map(x| false)", Some(&model)));
    let (_, projection) = map_parts(&query);

    assert!(matches!(
        projection.expression().operator(),
        ScalarOperator::Literal(ScalarLiteral::Boolean(false))
    ));
}

#[test]
fn lowers_parenthesized_and_integer_literals() {
    let model = filter_map_model();
    let parenthesized = supported(lower("model::Person.all()->map(x| (false))", Some(&model)));
    let (_, parenthesized_projection) = map_parts(&parenthesized);
    assert!(matches!(
        parenthesized_projection.expression().operator(),
        ScalarOperator::Literal(ScalarLiteral::Boolean(false))
    ));

    let integer = supported(lower("model::Person.all()->map(x| 7)", Some(&model)));
    let (_, integer_projection) = map_parts(&integer);
    assert!(matches!(
        integer_projection.expression().operator(),
        ScalarOperator::Literal(ScalarLiteral::Integer(7))
    ));
}

#[test]
fn lowers_parenthesized_relation_roots_and_continuations() {
    let model = filter_map_model();

    let root = supported(lower("(model::Person.all())", Some(&model)));
    assert_eq!(class_scan(root.root()).path().as_str(), "model::Person");

    let nested_root = supported(lower("((model::Person.all()))", Some(&model)));
    assert_eq!(
        class_scan(nested_root.root()).path().as_str(),
        "model::Person"
    );

    let continued = supported(lower(
        "(model::Person.all())->map(x| $x.manager)",
        Some(&model),
    ));
    let (input, projection) = map_parts(&continued);
    assert_eq!(class_scan(input).path().as_str(), "model::Person");
    assert!(matches!(
        projection.expression().operator(),
        ScalarOperator::Navigation { input, .. }
            if matches!(input.operator(), ScalarOperator::Column(id) if *id == ColumnId::new(0))
    ));
}

#[test]
fn lowers_bare_distinct_with_explicit_set_facts() {
    let model = filter_map_model();
    let source = "model::Person.all()->distinct()";
    let query = supported(lower(source, Some(&model)));
    let input = distinct_input(query.root());

    assert_eq!(query.output(), input.schema());
    assert_eq!(class_scan(input).path().as_str(), "model::Person");
    assert_eq!(
        span_text(source, query.root().origin().source()),
        "->distinct()"
    );
    assert!(query.facts().candidate_keys().is_unknown());
    let (semantics, fact_origin) = query
        .facts()
        .row_semantics()
        .as_proven()
        .expect("distinct must establish set semantics");
    assert_eq!(*semantics, RowSemantics::Set);
    assert_eq!(span_text(source, fact_origin.source()), "->distinct()");
    assert_eq!(
        fact_origin.model_origins(),
        query.root().origin().model_origins()
    );
}

#[test]
fn distinct_chains_after_filter_and_retains_the_element_binding() {
    let model = filter_map_model();
    let source = "model::Person.all()->filter(x| $x.name == 'Ada')->distinct()->map(x| $x.manager)";
    let query = supported(lower(source, Some(&model)));
    let (distinct, projection) = map_parts(&query);
    let filter = distinct_input(distinct);
    let (scan, _) = filter_parts(filter);

    assert_eq!(class_scan(scan).path().as_str(), "model::Person");
    assert!(matches!(
        projection.expression().operator(),
        ScalarOperator::Navigation { input, .. }
            if matches!(input.operator(), ScalarOperator::Column(id) if *id == ColumnId::new(0))
    ));
    let (semantics, _) = distinct
        .facts()
        .row_semantics()
        .as_proven()
        .expect("distinct must establish set semantics before a later map");
    assert_eq!(*semantics, RowSemantics::Set);
}

#[test]
fn distinct_rejects_unproven_overloads_and_preserves_model_requirements() {
    let model = filter_map_model();
    for source in [
        "model::Person.all()->distinct(x| $x.name)",
        "model::Person.all()->distinct(1)",
        "model::Person.all()->model::distinct()",
    ] {
        assert_reason(lower(source, Some(&model)), ReasonCode::IndUnmodeledOp);
    }
    assert_reason(
        lower("model::Person.all()->distinct(", Some(&model)),
        ReasonCode::IndUnparseable,
    );
    assert_reason(
        lower("model::Person.all()->distinct()", None),
        ReasonCode::ModelIncomplete,
    );
}

const RELATION_PROJECT_SOURCE: &str = "model::Person.all()->project(~[legal: person | $person.name, manager: person | $person.manager])";

fn relation_project_model() -> ModelGraph {
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

fn relation_project_parts(
    query: &pure_analyzer_analysis::RelationalQuery,
) -> (
    &pure_analyzer_analysis::RelationExpression,
    &[pure_analyzer_analysis::Projection],
) {
    match query.root().operator() {
        RelationOperator::Project {
            input,
            projections,
            kind,
        } => {
            assert_eq!(
                *kind,
                ProjectionKind::Relation,
                "an explicit `->project(~[...])` must lower as ProjectionKind::Relation"
            );
            (input, projections)
        }
        other => panic!("expected relation project, got {other:#?}"),
    }
}

struct ProjectColumnExpectation {
    id: ColumnId,
    name: &'static str,
    type_name: &'static str,
    lower: u32,
    source_text: &'static str,
}

fn assert_relation_project_schema_and_spans(
    query: &pure_analyzer_analysis::RelationalQuery,
    source: &str,
) {
    let columns = query.output().columns();
    assert_eq!(columns.len(), 2);
    assert_relation_project_column(
        &columns[0],
        ProjectColumnExpectation {
            id: ColumnId::new(ONE),
            name: "legal",
            type_name: "String",
            lower: ONE,
            source_text: "legal: person | $person.name",
        },
        source,
    );
    assert_relation_project_column(
        &columns[1],
        ProjectColumnExpectation {
            id: ColumnId::new(2),
            name: "manager",
            type_name: "model::Manager",
            lower: ZERO,
            source_text: "manager: person | $person.manager",
        },
        source,
    );
    assert_eq!(
        span_text(source, query.root().origin().source()),
        "->project(~[legal: person | $person.name, manager: person | $person.manager])"
    );
}

fn assert_relation_project_column(
    column: &pure_analyzer_analysis::Column,
    expected: ProjectColumnExpectation,
    source: &str,
) {
    assert_eq!(column.id(), expected.id);
    assert_eq!(column.name().as_str(), expected.name);
    assert_eq!(column.type_ref().raw_type().as_str(), expected.type_name);
    assert_eq!(column.multiplicity().lower(), expected.lower);
    assert_eq!(column.multiplicity().upper(), Some(ONE));
    assert_eq!(column.nullability(), Nullability::Unknown);
    assert_eq!(
        span_text(source, column.origin().source()),
        expected.source_text
    );
}

fn assert_relation_project_provenance(
    query: &pure_analyzer_analysis::RelationalQuery,
    model: &ModelGraph,
) {
    let person = resolved_class(model, "model::Person");
    let manager = resolved_class(model, "model::Manager");
    let name = resolved_member(model, &person, "name");
    let manager_member = resolved_member(model, &person, "manager");
    assert_eq!(
        query.root().origin().model_origins(),
        &[
            ModelOrigin::from_class(&person),
            ModelOrigin::from_member(&name),
            ModelOrigin::from_member(&manager_member),
            ModelOrigin::from_class(&manager),
        ]
    );
}

fn assert_relation_project_projection_inputs(projections: &[pure_analyzer_analysis::Projection]) {
    for projection in projections {
        assert!(matches!(
            navigation_parts(projection.expression()).0.operator(),
            ScalarOperator::Column(id) if *id == ColumnId::new(ZERO)
        ));
    }
}

#[test]
fn lowers_schema_aware_relation_project_with_ordered_metadata_and_provenance() {
    let model = relation_project_model();
    let first = lower(RELATION_PROJECT_SOURCE, Some(&model));
    let second = lower(RELATION_PROJECT_SOURCE, Some(&model));
    assert_eq!(first, second);

    let query = supported(first);
    let (input, projections) = relation_project_parts(&query);
    assert_eq!(class_scan(input).path().as_str(), "model::Person");
    assert_eq!(projections.len(), 2);
    assert_relation_project_schema_and_spans(&query, RELATION_PROJECT_SOURCE);
    assert!(query.facts().candidate_keys().is_unknown());
    assert!(query.facts().row_semantics().is_unknown());
    assert!(
        projections
            .iter()
            .all(|projection| projection.expression().totality().is_unknown())
    );
    assert_relation_project_provenance(&query, &model);
    assert_relation_project_projection_inputs(projections);
}

#[test]
fn relation_project_binds_its_output_as_a_resolved_row_for_following_lambdas() {
    let model = relation_project_model();
    let source = "model::Person.all()->project(~[legal: person | $person.name, manager: person | $person.manager])->map(row| $row.legal)";
    let query = supported(lower(source, Some(&model)));
    let (project, map_projection) = map_parts(&query);
    let (_, projections) = match project.operator() {
        RelationOperator::Project {
            input,
            projections,
            kind,
        } => {
            assert_eq!(*kind, ProjectionKind::Relation);
            (input, projections)
        }
        other => panic!("expected nested relation project, got {other:#?}"),
    };
    assert_eq!(projections.len(), 2);
    assert_eq!(query.output().columns()[0].id(), ColumnId::new(3));
    assert_eq!(query.output().columns()[0].name().as_str(), "value");
    assert_eq!(
        query.output().columns()[0].type_ref().raw_type().as_str(),
        "String"
    );
    assert_eq!(query.output().columns()[0].multiplicity().lower(), ONE);
    assert_eq!(
        query.output().columns()[0].multiplicity().upper(),
        Some(ONE)
    );
    assert_eq!(
        query.output().columns()[0].nullability(),
        Nullability::Unknown
    );
    assert!(matches!(
        map_projection.expression().operator(),
        ScalarOperator::Column(id) if *id == ColumnId::new(ONE)
    ));
    assert_eq!(
        span_text(source, map_projection.expression().origin().source()),
        ".legal"
    );
}

#[test]
fn selected_distinct_rebinds_selected_relation_project_columns_in_source_order() {
    let model = relation_project_model();
    let source = "model::Person.all()->project(~[legal: person | $person.name, manager: person | $person.manager])->distinct(~[manager, legal])->sort([ascending(~manager), ~legal->descending()])->map(row| $row.legal)";
    let query = supported(lower(source, Some(&model)));

    let (sort, map_projection) = map_parts(&query);
    let (selected, keys) = sort_parts(sort);
    let (project, columns) = selected_distinct_parts(selected);
    assert_eq!(columns, &[ColumnId::new(2), ColumnId::new(ONE)]);
    assert_eq!(
        selected.schema().columns(),
        &[
            project.schema().columns()[1].clone(),
            project.schema().columns()[0].clone(),
        ]
    );
    assert!(selected.facts().candidate_keys().is_unknown());
    assert!(selected.facts().row_semantics().is_unknown());
    assert_eq!(
        keys.iter().map(|key| key.column()).collect::<Vec<_>>(),
        [ColumnId::new(2), ColumnId::new(ONE)]
    );
    assert_eq!(
        keys.iter().map(|key| key.direction()).collect::<Vec<_>>(),
        [SortDirection::Ascending, SortDirection::Descending]
    );
    assert!(matches!(
        map_projection.expression().operator(),
        ScalarOperator::Column(id) if *id == ColumnId::new(ONE)
    ));
    assert_eq!(
        span_text(source, selected.origin().source()),
        "->distinct(~[manager, legal])"
    );
    assert_eq!(
        span_text(source, sort.origin().source()),
        "->sort([ascending(~manager), ~legal->descending()])"
    );
}

#[test]
fn relation_project_preserves_pure_model_members_and_quoted_aliases() {
    let model = pure_graph(
        r#"
            Class model::Person {
                legalName: String[1];
            }
        "#,
    );
    let source = "model::Person.all()->project(~['Legal Name': person | $person.legalName])";
    let query = supported(lower(source, Some(&model)));
    let (_, projections) = relation_project_parts(&query);
    let person = resolved_class(&model, "model::Person");
    let legal_name = resolved_member(&model, &person, "legalName");

    assert_eq!(query.output().columns()[0].id(), ColumnId::new(ONE));
    assert_eq!(query.output().columns()[0].name().as_str(), "Legal Name");
    assert_eq!(
        span_text(source, query.output().columns()[0].origin().source()),
        "'Legal Name': person | $person.legalName"
    );
    assert_eq!(
        projections[0].expression().origin().model_origins(),
        &[
            ModelOrigin::from_class(&person),
            ModelOrigin::from_member(&legal_name),
        ]
    );
}

#[test]
fn relation_project_declines_unverified_forms_without_partial_output() {
    let model = relation_project_model();
    for source in [
        "model::Person.all()->project(~legal: person | $person.name)",
        "model::Person.all()->project(~[legal])",
        "model::Person.all()->project(~[legal: person: model::Person[1] | $person.name])",
        "model::Person.all()->project(~[legal: {person| $person.name}])",
        "model::Person.all()->project(~[legal: person | $person.name, legal: person | $person.name])",
        "model::Person.all()->project(~[legal: person | $person.missing])",
    ] {
        let expected = if source.contains("missing") {
            ReasonCode::IndUnresolvedSchema
        } else {
            ReasonCode::IndUnmodeledOp
        };
        assert_reason(lower(source, Some(&model)), expected);
    }
    assert_reason(
        lower(
            "model::Person.all()->project(~[legal: person |])",
            Some(&model),
        ),
        ReasonCode::IndUnmodeledOp,
    );
    assert_reason(
        lower(
            "model::Person.all()->project(~[legal: person | $person.name",
            Some(&model),
        ),
        ReasonCode::IndUnparseable,
    );
}

const INNER_JOIN_SOURCE: &str = "model::Person.all()->join(model::Membership.all(), JoinKind.INNER, {person, membership | $person.personId == $membership.personId})";
const SELECTED_DISTINCT_SOURCE: &str = "model::Person.all()->join(model::Membership.all(), JoinKind.INNER, {person, membership | $person.personId == $membership.personId})->distinct(~[Membership, Person])";

fn inner_join_model() -> ModelGraph {
    graph(vec![
        class(
            "Person",
            vec![property("personId", "String", ONE, Some(ONE))],
        ),
        class(
            "Membership",
            vec![property("personId", "String", ONE, Some(ONE))],
        ),
    ])
}

fn pure_inner_join_model() -> ModelGraph {
    pure_graph(
        r#"
            Class model::Person {
                personId: String[1];
            }
            Class model::Membership {
                personId: String[1];
            }
        "#,
    )
}

fn assert_inner_join_schema_and_facts(query: &pure_analyzer_analysis::RelationalQuery) {
    assert_eq!(
        query
            .output()
            .columns()
            .iter()
            .map(|column| (column.id(), column.name().as_str()))
            .collect::<Vec<_>>(),
        vec![
            (ColumnId::new(ZERO), "Person"),
            (ColumnId::new(ONE), "Membership"),
        ]
    );
    assert!(query.facts().candidate_keys().is_unknown());
    assert!(query.facts().row_semantics().is_unknown());
}

fn assert_selected_distinct_contract(
    source: &str,
    query: &pure_analyzer_analysis::RelationalQuery,
) {
    let (input, columns) = selected_distinct_parts(query.root());
    assert_eq!(columns, &[ColumnId::new(ONE), ColumnId::new(ZERO)]);
    assert_eq!(
        span_text(source, query.root().origin().source()),
        "->distinct(~[Membership, Person])"
    );
    assert_eq!(
        query.output().columns(),
        &[
            input.schema().columns()[1].clone(),
            input.schema().columns()[0].clone()
        ]
    );
    assert!(query.facts().candidate_keys().is_unknown());
    assert!(query.facts().row_semantics().is_unknown());
}

fn assert_inner_join_binder_columns(condition: &pure_analyzer_analysis::ScalarExpression) {
    let (left_predicate, right_predicate) = equality_parts(condition);
    let (left_input, left_member) = navigation_parts(left_predicate);
    let (right_input, right_member) = navigation_parts(right_predicate);
    assert_eq!(left_member.owner().path().as_str(), "model::Person");
    assert_eq!(right_member.owner().path().as_str(), "model::Membership");
    assert!(matches!(
        left_input.operator(),
        ScalarOperator::Column(id) if *id == ColumnId::new(ZERO)
    ));
    assert!(matches!(
        right_input.operator(),
        ScalarOperator::Column(id) if *id == ColumnId::new(ONE)
    ));
}

fn assert_inner_join_provenance(
    query: &pure_analyzer_analysis::RelationalQuery,
    model: &ModelGraph,
) {
    let person = resolved_class(model, "model::Person");
    let membership = resolved_class(model, "model::Membership");
    let person_id = resolved_member(model, &person, "personId");
    let membership_person_id = resolved_member(model, &membership, "personId");
    assert_eq!(
        query.root().origin().model_origins(),
        &[
            ModelOrigin::from_class(&person),
            ModelOrigin::from_class(&membership),
            ModelOrigin::from_member(&person_id),
            ModelOrigin::from_member(&membership_person_id),
        ]
    );
}

#[test]
fn lowers_pinned_inner_join_with_ordered_schema_and_resolved_binders() {
    let model = inner_join_model();
    let first = lower(INNER_JOIN_SOURCE, Some(&model));
    let second = lower(INNER_JOIN_SOURCE, Some(&model));
    assert_eq!(first, second);

    let query = supported(first);
    let (left, right, condition) = join_parts(query.root());
    assert_eq!(class_scan(left).path().as_str(), "model::Person");
    assert_eq!(class_scan(right).path().as_str(), "model::Membership");
    assert_inner_join_schema_and_facts(&query);
    assert_eq!(
        span_text(INNER_JOIN_SOURCE, query.root().origin().source()),
        "->join(model::Membership.all(), JoinKind.INNER, {person, membership | $person.personId == $membership.personId})"
    );
    assert_eq!(
        span_text(INNER_JOIN_SOURCE, condition.origin().source()),
        " $person.personId == $membership.personId"
    );
    assert_inner_join_binder_columns(condition);
    assert_inner_join_provenance(&query, &model);
}

#[test]
fn lowers_selected_distinct_from_pmcd_with_ordered_cloned_columns() {
    let model = inner_join_model();
    let first = lower(SELECTED_DISTINCT_SOURCE, Some(&model));
    let second = lower(SELECTED_DISTINCT_SOURCE, Some(&model));
    assert_eq!(first, second);

    let query = supported(first);
    assert_selected_distinct_contract(SELECTED_DISTINCT_SOURCE, &query);
    let (input, _) = selected_distinct_parts(query.root());
    let (left, right, _) = join_parts(input);
    assert_eq!(class_scan(left).path().as_str(), "model::Person");
    assert_eq!(class_scan(right).path().as_str(), "model::Membership");
}

#[test]
fn lowers_selected_distinct_from_pure_with_ordered_cloned_columns() {
    let model = pure_inner_join_model();
    let query = supported(lower(SELECTED_DISTINCT_SOURCE, Some(&model)));

    assert_selected_distinct_contract(SELECTED_DISTINCT_SOURCE, &query);
}

#[test]
fn sort_accepts_schema_only_selected_distinct_output_after_a_join() {
    let model = inner_join_model();
    let source = "model::Person.all()->join(model::Membership.all(), JoinKind.INNER, {person, membership | $person.personId == $membership.personId})->distinct(~[Membership, Person])->sort([descending(~Membership), ~Person->ascending()])";
    let query = supported(lower(source, Some(&model)));

    let (selected, keys) = sort_parts(query.root());
    let (join, columns) = selected_distinct_parts(selected);
    assert!(matches!(join.operator(), RelationOperator::Join { .. }));
    assert_eq!(columns, &[ColumnId::new(ONE), ColumnId::new(ZERO)]);
    assert_eq!(
        keys.iter().map(|key| key.column()).collect::<Vec<_>>(),
        [ColumnId::new(ONE), ColumnId::new(ZERO)]
    );
    assert_eq!(
        keys.iter().map(|key| key.direction()).collect::<Vec<_>>(),
        [SortDirection::Descending, SortDirection::Ascending]
    );
    assert_eq!(query.output(), selected.schema());
    assert!(query.facts().candidate_keys().is_unknown());
    assert!(query.facts().row_semantics().is_unknown());
}

#[test]
fn selected_distinct_retains_a_selected_element_binding() {
    let model = filter_map_model();
    let source = "model::Person.all()->distinct(~[Person])->map(x| $x.manager)";
    let query = supported(lower(source, Some(&model)));

    let (selected, projection) = map_parts(&query);
    let (input, columns) = selected_distinct_parts(selected);
    assert_eq!(columns, &[ColumnId::new(ZERO)]);
    assert_eq!(class_scan(input).path().as_str(), "model::Person");
    assert!(matches!(
        projection.expression().operator(),
        ScalarOperator::Navigation { input, .. }
            if matches!(input.operator(), ScalarOperator::Column(id) if *id == ColumnId::new(ZERO))
    ));
}

#[test]
fn selected_distinct_requires_the_exact_resolved_array_form() {
    let model = filter_map_model();
    for source in [
        "model::Person.all()->distinct(~Person)",
        "model::Person.all()->distinct(~[])",
        "model::Person.all()->distinct(~[Person: String])",
        "model::Person.all()->distinct(~[Person, Person])",
        "model::Person.all()->distinct(~[Person], ~[Person])",
        "model::Person.all()->distinct((~[Person]))",
    ] {
        assert_reason(lower(source, Some(&model)), ReasonCode::IndUnmodeledOp);
    }

    let missing_source = "model::Person.all()->distinct(~[Missing])";
    let missing = opaque(lower(missing_source, Some(&model)));
    assert_eq!(missing.reason(), ReasonCode::IndUnresolvedSchema);
    assert_eq!(
        span_text(missing_source, missing.origin().source()),
        "Missing"
    );

    let duplicate_schema_source = "model::Person.all()->join(model::Person.all(), JoinKind.INNER, {left, right | $left.personId == $right.personId})->distinct(~[Person])";
    assert_reason(
        lower(duplicate_schema_source, Some(&inner_join_model())),
        ReasonCode::IndUnresolvedSchema,
    );
}

#[test]
fn inner_join_retains_input_evidence_without_inferring_join_facts() {
    let model = inner_join_model();
    let source = "model::Person.all()->distinct()->join(model::Membership.all(), JoinKind.INNER, {person, membership | $person.personId == $membership.personId})";
    let query = supported(lower(source, Some(&model)));
    let (left, right, _) = join_parts(query.root());
    let (semantics, fact_origin) = left
        .facts()
        .row_semantics()
        .as_proven()
        .expect("left distinct evidence must remain nested under the join");
    assert_eq!(*semantics, RowSemantics::Set);
    assert_eq!(span_text(source, fact_origin.source()), "->distinct()");
    assert_eq!(
        class_scan(distinct_input(left)).path().as_str(),
        "model::Person"
    );
    let person = resolved_class(&model, "model::Person");
    let membership = resolved_class(&model, "model::Membership");
    assert_eq!(
        left.origin().model_origins(),
        &[ModelOrigin::from_class(&person)]
    );
    assert_eq!(class_scan(right).path().as_str(), "model::Membership");
    assert_eq!(
        right.origin().model_origins(),
        &[ModelOrigin::from_class(&membership)]
    );
    assert!(query.facts().candidate_keys().is_unknown());
    assert!(query.facts().row_semantics().is_unknown());
}

#[test]
fn inner_join_declines_unproven_forms_without_rebinding_rows() {
    let model = inner_join_model();
    for source in [
        "model::Person.all()->join(model::Membership.all(), JoinKind.INNER, {person, person | $person.personId == $person.personId})",
        "model::Person.all()->join(model::Membership.all(), JoinKind.INNER, {person, membership | true})",
        "model::Person.all()->join(model::Membership.all(), JoinKind.INNER, {person, membership | $person.personId == $person.personId})",
        "model::Person.all()->join(model::Membership.all(), JoinKind.INNER, {person, membership | $person.personId})",
    ] {
        assert_reason(lower(source, Some(&model)), ReasonCode::IndOpaquePredicate);
    }
    assert_reason(
        lower(
            "model::Person.all()->join(model::Membership.all(), JoinKind.LEFT, {person, membership | $person.personId == $membership.personId})",
            Some(&model),
        ),
        ReasonCode::IndUnmodeledOp,
    );
    assert_reason(
        lower(
            "model::Person.all()->join(model::Membership.all(), JoinKind.INNER, {person, membership | $person.personId == $membership.personId})->map(x| $x.personId)",
            Some(&model),
        ),
        ReasonCode::IndUnmodeledOp,
    );
    assert_reason(
        lower(
            "model::Person.all()->join(model::Membership.all(), JoinKind.INNER, {person, membership | $person.personId == $membership.personId})->distinct()",
            Some(&model),
        ),
        ReasonCode::IndUnmodeledOp,
    );
    assert_reason(
        lower(
            "model::Missing.all()->join(model::Membership.all(), JoinKind.INNER, {person, membership | $person.personId == $membership.personId})",
            Some(&model),
        ),
        ReasonCode::IndUnresolvedSchema,
    );
    assert_reason(
        lower(
            "model::Person.all()->join(model::Missing.all(), JoinKind.INNER, {person, membership | $person.personId == $membership.personId})",
            Some(&model),
        ),
        ReasonCode::IndUnresolvedSchema,
    );
    assert_reason(
        lower(
            "model::Person.all()->join(model::Membership.all(), JoinKind.INNER, {person, membership | $person.personId == $unbound.personId})",
            Some(&model),
        ),
        ReasonCode::IndUnresolvedSchema,
    );
}

#[test]
fn lowers_proven_ascending_sort_key_with_exact_origin() {
    let model = filter_map_model();
    let source = "model::Person.all()->distinct()->sort(ascending(~Person))";
    let query = supported(lower(source, Some(&model)));
    let (input, keys) = sort_parts(query.root());

    assert_eq!(query.output(), input.schema());
    assert_eq!(query.facts(), input.facts());
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0].column(), ColumnId::new(ZERO));
    assert_eq!(keys[0].direction(), SortDirection::Ascending);
    assert_eq!(
        span_text(source, query.root().origin().source()),
        "->sort(ascending(~Person))"
    );
    assert_eq!(
        span_text(source, keys[0].origin().source()),
        "ascending(~Person)"
    );
    let scan = distinct_input(input);
    let class = class_scan(scan);
    assert_eq!(
        keys[0].origin().model_origins(),
        &[ModelOrigin::from_class(class)]
    );
}

#[test]
fn lowers_proven_descending_sort_key_with_exact_origin() {
    let model = filter_map_model();
    let source = "model::Person.all()->sort(~Person->descending())";
    let query = supported(lower(source, Some(&model)));
    let (input, keys) = sort_parts(query.root());

    assert_eq!(query.output(), input.schema());
    assert_eq!(query.facts(), input.facts());
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0].column(), ColumnId::new(ZERO));
    assert_eq!(keys[0].direction(), SortDirection::Descending);
    assert_eq!(
        span_text(source, keys[0].origin().source()),
        "~Person->descending()"
    );
    assert_eq!(
        query.root().origin().model_origins(),
        &[ModelOrigin::from_class(class_scan(input))]
    );
}

#[test]
fn sort_retains_the_element_binding_for_later_supported_operations() {
    let model = filter_map_model();
    let source = "model::Person.all()->sort(~Person->ascending())->map(x| $x.manager)";
    let query = supported(lower(source, Some(&model)));
    let (sort, projection) = map_parts(&query);
    let (scan, keys) = sort_parts(sort);

    assert_eq!(class_scan(scan).path().as_str(), "model::Person");
    assert_eq!(keys[0].column(), ColumnId::new(ZERO));
    assert!(matches!(
        projection.expression().operator(),
        ScalarOperator::Navigation { input, .. }
            if matches!(input.operator(), ScalarOperator::Column(id) if *id == ColumnId::new(ZERO))
    ));
}

#[test]
fn sort_rejects_unproven_forms_and_keeps_resolution_and_parse_failures_typed() {
    let model = filter_map_model();
    for source in [
        "model::Person.all()->sort()",
        "model::Person.all()->sort(~Person)",
        "model::Person.all()->sort([~Person])",
        "model::Person.all()->sort([ascending(~Person), descending(~Person)])",
        "model::Person.all()->sort(~Person->ascending()->nullsFirst())",
        "model::Person.all()->sort([~Person->descending()->nullsFirst()])",
        "model::Person.all()->sort(x| $x)",
        "model::Person.all()->sort(unknown(~Person))",
        "model::Person.all()->sort([unknown(~Person)])",
        "model::Person.all()->sort(~Person->unknown())",
    ] {
        assert_reason(lower(source, Some(&model)), ReasonCode::IndUnmodeledOp);
    }

    let missing_source = "model::Person.all()->sort([ascending(~Missing)])";
    let missing = opaque(lower(missing_source, Some(&model)));
    assert_eq!(missing.reason(), ReasonCode::IndUnresolvedSchema);
    assert_eq!(
        span_text(missing_source, missing.origin().source()),
        "Missing"
    );
    assert_reason(
        lower(
            "model::Person.all()->sort(~Person->ascending(",
            Some(&model),
        ),
        ReasonCode::IndUnparseable,
    );
}

#[test]
fn sort_lowering_is_deterministic_for_repeated_input() {
    let model = filter_map_model();
    let source = "model::Person.all()->distinct()->sort([descending(~Person)])";

    assert_eq!(lower(source, Some(&model)), lower(source, Some(&model)));
}

#[test]
fn sort_accepts_direct_comma_separated_call_arguments_without_a_bracket_array() {
    let model = relation_project_model();
    let source =
        format!("{RELATION_PROJECT_SOURCE}->sort(ascending(~legal), descending(~manager))");
    let query = supported(lower(&source, Some(&model)));
    let (_, keys) = sort_parts(query.root());

    assert_eq!(keys.len(), 2);
    assert_eq!(keys[0].column(), ColumnId::new(ONE));
    assert_eq!(keys[0].direction(), SortDirection::Ascending);
    assert_eq!(keys[1].column(), ColumnId::new(2));
    assert_eq!(keys[1].direction(), SortDirection::Descending);
    assert_eq!(
        span_text(&source, query.root().origin().source()),
        "->sort(ascending(~legal), descending(~manager))"
    );
}

#[test]
fn selected_distinct_rebinds_every_selected_column_to_its_own_identity_under_full_reordering() {
    let model = graph(vec![class(
        "Person",
        vec![
            property("id", "String", ONE, Some(ONE)),
            property("age", "Integer", ONE, Some(ONE)),
            property("flag", "Boolean", ONE, Some(ONE)),
        ],
    )]);
    let source = "model::Person.all()->project(~[id: p | $p.id, age: p | $p.age, flag: p | $p.flag])->distinct(~[flag, id, age])";

    for (name, type_name) in [("id", "String"), ("age", "Integer"), ("flag", "Boolean")] {
        let map_source = format!("{source}->map(row| $row.{name})");
        let query = supported(lower(&map_source, Some(&model)));
        let (_, projection) = map_parts(&query);
        assert_eq!(
            projection.expression().type_ref().raw_type().as_str(),
            type_name,
            "selecting {name} under a full reorder must keep its own column identity"
        );
    }
}

#[test]
fn inner_join_rejects_a_non_joinkind_qualifier_even_when_the_member_name_matches() {
    let model = inner_join_model();
    let source = "model::Person.all()->join(model::Membership.all(), Foo.INNER, {person, membership | $person.personId == $membership.personId})";

    assert_reason(lower(source, Some(&model)), ReasonCode::IndUnmodeledOp);
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

    let RelationOperator::Project {
        input,
        projections,
        kind,
    } = query.root().operator()
    else {
        panic!("direct navigation must lower through project");
    };
    assert_eq!(
        *kind,
        ProjectionKind::Scalar,
        "`.property` navigation is a scalar collection, never a Relation<>"
    );
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
    let RelationOperator::Project {
        projections, kind, ..
    } = query.root().operator()
    else {
        panic!("map must lower to project");
    };
    assert_eq!(*kind, ProjectionKind::Scalar);
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
    let RelationOperator::Project {
        projections, kind, ..
    } = query.root().operator()
    else {
        panic!("map must lower to project");
    };
    assert_eq!(*kind, ProjectionKind::Scalar);
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
            property("maybeName", "String", ZERO, Some(ONE)),
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
        lower("model::Person.allVersionsInRange()", Some(&model)),
        ReasonCode::IndUnmodeledOp,
    );
    assert_reason(
        lower(
            "model::Person.all()->filter(x| $x.maybeName == 'Ada')",
            Some(&model),
        ),
        ReasonCode::IndOpaquePredicate,
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
        RelationOperator::Filter { input, .. }
        | RelationOperator::Distinct { input }
        | RelationOperator::DistinctOn { input, .. }
        | RelationOperator::Sort { input, .. } => {
            project_ids(input, ids);
        }
        RelationOperator::Project {
            input, projections, ..
        } => {
            project_ids(input, ids);
            ids.extend(projections.iter().map(|projection| projection.column()));
        }
        RelationOperator::Join { left, right, .. } => {
            project_ids(left, ids);
            project_ids(right, ids);
        }
    }
}

// A plain two-case loop, not a `proptest!` property: `reverse` is a `bool`,
// a two-element input space, so there is no property space for proptest's
// generator/shrinker machinery to explore — running it 256 times would just
// repeat the same two cases hundreds of times over.
#[test]
fn relation_project_is_deterministic_and_retains_declared_alias_order() {
    let model = relation_project_model();
    for reverse in [false, true] {
        let (source, expected_names) = if reverse {
            (
                "model::Person.all()->project(~[manager: person | $person.manager, legal: person | $person.name])",
                ["manager", "legal"],
            )
        } else {
            (RELATION_PROJECT_SOURCE, ["legal", "manager"])
        };
        let first = lower(source, Some(&model));
        let second = lower(source, Some(&model));

        assert_eq!(first, second, "reverse={reverse}");
        let RelationalOutcome::Supported(query) = first else {
            panic!("supported relation project lowered opaque (reverse={reverse}): {first:#?}");
        };
        assert_eq!(
            query
                .output()
                .columns()
                .iter()
                .map(|column| column.name().as_str())
                .collect::<Vec<_>>(),
            expected_names.to_vec(),
            "reverse={reverse}",
        );
        assert!(
            query.facts().candidate_keys().is_unknown(),
            "reverse={reverse}"
        );
        assert!(
            query.facts().row_semantics().is_unknown(),
            "reverse={reverse}"
        );
    }
}

proptest! {
    #[test]
    fn lowering_is_deterministic_for_bounded_supported_pipelines(
        steps in proptest::collection::vec(any::<bool>(), 0..=8),
    ) {
        let model = supported_pipeline_graph();
        let source = format!("{}->distinct()", supported_pipeline(&steps));
        let first = lower(&source, Some(&model));
        let second = lower(&source, Some(&model));

        prop_assert_eq!(&first, &second);
        let RelationalOutcome::Supported(query) = first else {
            prop_assert!(false, "supported pipeline lowered opaque: {first:#?}");
            return Ok(());
        };
        let (semantics, _) = query
            .facts()
            .row_semantics()
            .as_proven()
            .expect("terminal distinct must establish set semantics");
        prop_assert_eq!(*semantics, RowSemantics::Set);
        prop_assert_eq!(query.output().columns().len(), 1);
        let mut ids = Vec::new();
        project_ids(query.root(), &mut ids);
        let expected = (ONE..=u32::try_from(steps.iter().filter(|step| **step).count()).unwrap_or(ZERO))
            .map(ColumnId::new)
            .collect::<Vec<_>>();
        prop_assert_eq!(ids, expected);
    }
}
