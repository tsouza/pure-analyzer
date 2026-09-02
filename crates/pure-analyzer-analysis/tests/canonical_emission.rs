//! Contracts for fail-closed canonical normal-form emission.
#![allow(clippy::disallowed_methods)]

use pure_analyzer_analysis::{
    AnalysisInput, CandidateKey, CanonicalEmissionOutcome, Column, ColumnId, Knowledge,
    NormalizationBudget, NormalizationOutcome, Nullability, Projection, RelationExpression,
    RelationFacts, RelationOperator, RelationSchema, RelationalOutcome, ScalarExpression,
    ScalarLiteral, ScalarOperator, emit_canonical_normal_form, emit_canonical_normalization,
    lower_m3_query, normalize_relational_query, normalize_relational_query_with_budget,
};
use pure_analyzer_diagnostics::{FileId, ReasonCode};
use pure_analyzer_model::{ModelGraph, Name, PmcdDocument, load_pmcd_documents};
use pure_analyzer_parser::parse_query;
use serde_json::json;

const TEST_FILE: u32 = 89;

fn model() -> ModelGraph {
    let source = json!({
        "_type": "data",
        "elements": [
            {
                "_type": "class",
                "package": "model",
                "name": "Person",
                "stereotypes": [],
                "superTypes": [],
                "properties": [
                    {
                        "name": "name",
                        "genericType": {"rawType": "String", "typeArguments": []},
                        "multiplicity": {"lowerBound": 1, "upperBound": 1}
                    },
                    {
                        "name": "manager",
                        "genericType": {"rawType": "model::Manager", "typeArguments": []},
                        "multiplicity": {"lowerBound": 1, "upperBound": 1}
                    },
                    {
                        "name": "personId",
                        "genericType": {"rawType": "String", "typeArguments": []},
                        "multiplicity": {"lowerBound": 1, "upperBound": 1}
                    }
                ],
                "qualifiedProperties": []
            },
            {
                "_type": "class",
                "package": "model",
                "name": "Manager",
                "stereotypes": [],
                "superTypes": [],
                "properties": [],
                "qualifiedProperties": []
            },
            {
                "_type": "class",
                "package": "model",
                "name": "Membership",
                "stereotypes": [],
                "superTypes": [],
                "properties": [
                    {
                        "name": "personId",
                        "genericType": {"rawType": "String", "typeArguments": []},
                        "multiplicity": {"lowerBound": 1, "upperBound": 1}
                    }
                ],
                "qualifiedProperties": []
            }
        ]
    })
    .to_string();
    load_pmcd_documents(&[PmcdDocument::new("canonical-emission-fixture", &source)])
        .expect("fixture model must load")
}

fn lower(source: &str, model: &ModelGraph) -> pure_analyzer_analysis::RelationalQuery {
    let parsed = parse_query(source, FileId::new(TEST_FILE)).expect("fixture source must parse");
    let outcome = lower_m3_query(AnalysisInput::new(
        FileId::new(TEST_FILE),
        source,
        &parsed.green,
        &parsed.diagnostics,
        Some(model),
    ));
    let RelationalOutcome::Supported(query) = outcome else {
        panic!("fixture must lower through the supported core: {outcome:#?}");
    };
    *query
}

fn normal_form(source: &str, model: &ModelGraph) -> pure_analyzer_analysis::NormalizedQuery {
    let query = lower(source, model);
    let NormalizationOutcome::Normalized(normalized) = normalize_relational_query(&query) else {
        panic!("fixture must normalize");
    };
    *normalized
}

fn emitted_text(outcome: CanonicalEmissionOutcome) -> String {
    let CanonicalEmissionOutcome::Emitted(emitted) = outcome else {
        panic!("fixture must emit: {outcome:#?}");
    };
    emitted.into_string()
}

#[test]
fn emits_a_stable_ordered_filter_project_distinct_and_sort_chain() {
    let model = model();
    let source = "model::Person.all()->filter(person| $person.name == 'Ada')->project(~[name: person | $person.name, manager: person | $person.manager])->distinct(~[manager, name])->sort([ascending(~manager), descending(~name)])";
    let original = normal_form(source, &model);

    let first = emitted_text(emit_canonical_normal_form(&original));
    let second = emitted_text(emit_canonical_normal_form(&original));

    assert_eq!(
        first,
        "model::Person.all()->filter(v0| ($v0.name == 'Ada'))->project(~[name: v1 | $v1.name, manager: v1 | $v1.manager])->distinct(~[manager, name])->sort([ascending(~manager), descending(~name)])"
    );
    assert_eq!(first, second);

    let replayed = normal_form(&first, &model);
    assert_eq!(original.equivalence_key(), replayed.equivalence_key());
}

#[test]
fn emits_join_binders_and_selected_output_order_deterministically() {
    let model = model();
    let source = "model::Person.all()->join(model::Membership.all(), JoinKind.INNER, {person, membership | $person.personId == $membership.personId})->distinct(~[Membership, Person])->sort([descending(~Membership), ascending(~Person)])";
    let original = normal_form(source, &model);

    let text = emitted_text(emit_canonical_normal_form(&original));

    assert_eq!(
        text,
        "model::Person.all()->join(model::Membership.all(), JoinKind.INNER, {v0, v1 | ($v0.personId == $v1.personId)})->distinct(~[Membership, Person])->sort([descending(~Membership), ascending(~Person)])"
    );
    let replayed = normal_form(&text, &model);
    assert_eq!(original.equivalence_key(), replayed.equivalence_key());
}

#[test]
fn emits_quoted_terminal_aliases_without_claiming_a_lossless_layout() {
    let model = model();
    let source = "model::Person.all()->project(~['Legal Name': person | $person.name])";
    let original = normal_form(source, &model);

    let text = emitted_text(emit_canonical_normal_form(&original));

    assert_eq!(
        text,
        "model::Person.all()->project(~['Legal Name': v0 | $v0.name])"
    );
    let replayed = normal_form(&text, &model);
    assert_eq!(original.equivalence_key(), replayed.equivalence_key());
}

#[test]
fn normalization_failure_is_preserved_without_partial_text() {
    let model = model();
    let query = lower("model::Person.all()", &model);
    let outcome = normalize_relational_query_with_budget(&query, NormalizationBudget::new(0));

    let CanonicalEmissionOutcome::Indecisive(indecision) = emit_canonical_normalization(&outcome)
    else {
        panic!("exhausted normalization must not emit text");
    };
    assert_eq!(indecision.reason(), ReasonCode::IndMissingRewrite);
    let Some(failure) = outcome.failure() else {
        panic!("zero budget must return a normalization failure");
    };
    assert_eq!(indecision.origin(), failure.origin());
}

#[test]
fn proven_candidate_keys_are_refused_instead_of_being_silently_lost() {
    let model = model();
    let query = lower("model::Person.all()", &model);
    let root = query.root();
    let key = CandidateKey::new(vec![root.schema().columns()[0].id()]);
    let facts = RelationFacts::new(
        Knowledge::proven(vec![key], root.origin().clone()),
        Knowledge::unknown(),
    );
    let keyed = RelationExpression::new(
        root.operator().clone(),
        root.schema().clone(),
        facts,
        root.origin().clone(),
    )
    .expect("keyed scan remains a valid IR value");
    let keyed_query = pure_analyzer_analysis::RelationalQuery::new(keyed);
    let NormalizationOutcome::Normalized(normalized) = normalize_relational_query(&keyed_query)
    else {
        panic!("keyed scan must still normalize");
    };

    let CanonicalEmissionOutcome::Indecisive(indecision) = emit_canonical_normal_form(&normalized)
    else {
        panic!("unrepresentable candidate keys must not emit text");
    };
    assert_eq!(indecision.reason(), ReasonCode::IndUnmodeledOp);
    assert_eq!(indecision.origin(), root.origin());
}

#[test]
fn a_scalar_form_without_supported_pure_syntax_is_refused() {
    let model = model();
    let query = lower("model::Person.all()", &model);
    let input = query.root();
    let scalar = ScalarExpression::new(
        ScalarOperator::Literal(ScalarLiteral::Null),
        input.schema().columns()[0].type_ref().clone(),
        input.schema().columns()[0].multiplicity(),
        Nullability::Nullable,
        Knowledge::unknown(),
        input.origin().clone(),
    );
    let output = Column::new(
        ColumnId::new(91),
        Name::new("value").expect("fixture name must be valid"),
        scalar.type_ref().clone(),
        scalar.multiplicity(),
        scalar.nullability(),
        scalar.origin().clone(),
    );
    let project = RelationExpression::new(
        RelationOperator::Project {
            input: Box::new(input.clone()),
            projections: vec![Projection::new(output.id(), scalar)],
        },
        RelationSchema::new(vec![output]).expect("fixture schema must be valid"),
        RelationFacts::unknown(),
        input.origin().clone(),
    )
    .expect("fixture project must be valid IR");
    let query = pure_analyzer_analysis::RelationalQuery::new(project);
    let NormalizationOutcome::Normalized(normalized) = normalize_relational_query(&query) else {
        panic!("supported IR fixture must normalize");
    };

    let CanonicalEmissionOutcome::Indecisive(indecision) = emit_canonical_normal_form(&normalized)
    else {
        panic!("unsupported scalar syntax must not emit text");
    };
    assert_eq!(indecision.reason(), ReasonCode::IndUnmodeledOp);
    assert_eq!(indecision.origin(), input.origin());
}
