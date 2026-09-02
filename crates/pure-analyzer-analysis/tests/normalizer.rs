//! End-to-end proof contracts for conservative relational normalization.

use proptest::prelude::*;
use pure_analyzer_analysis::{
    CandidateKey, Column, ColumnId, EquivalenceKey, IrOrigin, Knowledge, ModelOrigin,
    NormalizationBudget, NormalizationOutcome, Nullability, Projection, RelationExpression,
    RelationFacts, RelationOperator, RelationSchema, RelationSource, RelationalQuery, RowSemantics,
    ScalarExpression, ScalarLiteral, ScalarOperator, SortDirection, SortKey, SourceSpan, Totality,
    normalize_relational_query, normalize_relational_query_with_budget,
};
use pure_analyzer_diagnostics::{FileId, ReasonCode, TextRange, TextSize};
use pure_analyzer_model::{Multiplicity, PmcdDocument, QName, TypeRef, load_pmcd_documents};
use pure_analyzer_resolve::{Resolution, ResolvedClass, Resolver};
use serde_json::Value;

const FILE: u32 = 31;
const EXACTLY_ONE: u32 = 1;
const SEMANTIC_CORPUS_CASES: &str = include_str!("../corpus/legend-4.113.0/cases.jsonl");

fn origin(file: u32, start: u32, end: u32, model_origins: Vec<ModelOrigin>) -> IrOrigin {
    IrOrigin::new(
        SourceSpan::new(
            FileId::new(file),
            TextRange::new(TextSize::from(start), TextSize::from(end)),
        ),
        model_origins,
    )
}

fn one() -> Multiplicity {
    Multiplicity::new(EXACTLY_ONE, Some(EXACTLY_ONE)).expect("fixture multiplicity is valid")
}

fn type_ref(name: &str) -> TypeRef {
    TypeRef::new(QName::new(name).expect("fixture type is valid"), Vec::new())
}

fn classes() -> (ResolvedClass, ResolvedClass) {
    let source = r#"{
        "_type": "data",
        "elements": [
            {
                "_type": "class",
                "package": "model",
                "name": "Left",
                "stereotypes": [],
                "superTypes": [],
                "properties": [],
                "qualifiedProperties": []
            },
            {
                "_type": "class",
                "package": "model",
                "name": "Right",
                "stereotypes": [],
                "superTypes": [],
                "properties": [],
                "qualifiedProperties": []
            }
        ]
    }"#;
    let graph = load_pmcd_documents(&[PmcdDocument::new("normalizer-fixture", source)])
        .expect("fixture model loads");
    let resolver = Resolver::new(&graph);
    let resolve = |name: &str| {
        let path = QName::new(name).expect("fixture path is valid");
        match resolver.resolve_class(&path) {
            Resolution::Found(class) => class,
            outcome => panic!("fixture class resolves: {outcome:?}"),
        }
    };
    (resolve("model::Left"), resolve("model::Right"))
}

fn column(id: u32, name: &str, source: IrOrigin) -> Column {
    Column::new(
        ColumnId::new(id),
        name.parse().expect("fixture column name is valid"),
        type_ref("String"),
        one(),
        Nullability::NonNullable,
        source,
    )
}

fn scan(ids: &[u32], names: &[&str], source: IrOrigin, class: ResolvedClass) -> RelationExpression {
    let schema = RelationSchema::new(
        ids.iter()
            .zip(names)
            .map(|(id, name)| column(*id, name, source.clone()))
            .collect(),
    )
    .expect("fixture schema is valid");
    RelationExpression::new(
        RelationOperator::Scan(RelationSource::Class(class)),
        schema,
        RelationFacts::unknown(),
        source,
    )
    .expect("fixture scan is valid")
}

fn query(ids: &[u32], names: &[&str], source: IrOrigin) -> RelationalQuery {
    let (class, _) = classes();
    RelationalQuery::new(scan(ids, names, source, class))
}

fn scalar_column(column: &Column, source: IrOrigin) -> ScalarExpression {
    ScalarExpression::new(
        ScalarOperator::Column(column.id()),
        column.type_ref().clone(),
        column.multiplicity(),
        column.nullability(),
        Knowledge::unknown(),
        source,
    )
}

fn identity_project(input: RelationExpression, source: IrOrigin) -> RelationExpression {
    identity_project_with_metadata(
        input,
        source,
        RelationFacts::unknown(),
        Knowledge::unknown(),
    )
}

fn identity_project_with_metadata(
    input: RelationExpression,
    source: IrOrigin,
    facts: RelationFacts,
    totality: Knowledge<Totality>,
) -> RelationExpression {
    let schema = input.schema().clone();
    let projections = schema
        .columns()
        .iter()
        .map(|column| {
            Projection::new(
                column.id(),
                ScalarExpression::new(
                    ScalarOperator::Column(column.id()),
                    column.type_ref().clone(),
                    column.multiplicity(),
                    column.nullability(),
                    totality.clone(),
                    source.clone(),
                ),
            )
        })
        .collect();
    RelationExpression::new(
        RelationOperator::Project {
            input: Box::new(input),
            projections,
        },
        schema,
        facts,
        source,
    )
    .expect("identity project is valid")
}

fn true_predicate(source: IrOrigin) -> ScalarExpression {
    boolean_predicate(true, source, Knowledge::unknown())
}

fn boolean_predicate(
    value: bool,
    source: IrOrigin,
    totality: Knowledge<Totality>,
) -> ScalarExpression {
    ScalarExpression::new(
        ScalarOperator::Literal(ScalarLiteral::Boolean(value)),
        type_ref("Boolean"),
        one(),
        Nullability::NonNullable,
        totality,
        source,
    )
}

fn filter(
    input: RelationExpression,
    predicate: ScalarExpression,
    facts: RelationFacts,
    source: IrOrigin,
) -> RelationExpression {
    let schema = input.schema().clone();
    RelationExpression::new(
        RelationOperator::Filter {
            input: Box::new(input),
            predicate,
        },
        schema,
        facts,
        source,
    )
    .expect("filter fixture is valid")
}

fn distinct(
    input: RelationExpression,
    facts: RelationFacts,
    source: IrOrigin,
) -> RelationExpression {
    let schema = input.schema().clone();
    RelationExpression::new(
        RelationOperator::Distinct {
            input: Box::new(input),
        },
        schema,
        facts,
        source,
    )
    .expect("distinct fixture is valid")
}

fn repeated_distinct(
    input: RelationExpression,
    facts: RelationFacts,
    inner_source: IrOrigin,
    outer_source: IrOrigin,
) -> RelationExpression {
    distinct(
        distinct(input, facts.clone(), inner_source),
        facts,
        outer_source,
    )
}

fn normalized(query: &RelationalQuery) -> pure_analyzer_analysis::NormalizedQuery {
    match normalize_relational_query(query) {
        NormalizationOutcome::Normalized(value) => *value,
        NormalizationOutcome::Indecisive(value) => {
            panic!("normalization unexpectedly stopped: {value:?}")
        }
    }
}

fn key(query: &RelationalQuery) -> EquivalenceKey {
    normalized(query).equivalence_key().clone()
}

#[test]
fn allocation_and_source_span_do_not_change_the_semantic_key() {
    let first = query(&[7, 91], &["left", "right"], origin(FILE, 1, 9, Vec::new()));
    let second = query(
        &[902, 3],
        &["left", "right"],
        origin(FILE + 1, 101, 199, Vec::new()),
    );

    assert_eq!(key(&first), key(&second));
    assert_ne!(
        normalized(&first).structural_key(),
        normalized(&second).structural_key(),
        "the audit key must retain exact source provenance"
    );
}

#[test]
fn nested_projection_scopes_alpha_normalize_reused_column_ids() {
    let project = |input_id: u32, output_id: u32| {
        let source = origin(FILE, 1, 9, Vec::new());
        let base = query(&[input_id], &["input"], source.clone());
        let input = base.root().clone();
        let input_column = input.schema().columns()[0].clone();
        let output = Column::new(
            ColumnId::new(output_id),
            "alias".parse().expect("fixture name is valid"),
            input_column.type_ref().clone(),
            input_column.multiplicity(),
            input_column.nullability(),
            source.clone(),
        );
        RelationalQuery::new(
            RelationExpression::new(
                RelationOperator::Project {
                    input: Box::new(input),
                    projections: vec![Projection::new(
                        output.id(),
                        scalar_column(&input_column, source.clone()),
                    )],
                },
                RelationSchema::new(vec![output]).expect("fixture schema is valid"),
                RelationFacts::unknown(),
                source,
            )
            .expect("rebinding project is valid"),
        )
    };

    let reused = project(4, 4);
    let fresh = project(50, 900);
    let reused = normalized(&reused);
    let fresh = normalized(&fresh);
    assert_eq!(reused.equivalence_key(), fresh.equivalence_key());
    assert_eq!(reused.structural_key(), fresh.structural_key());
}

#[test]
fn structural_key_canonicalizes_model_origin_collection_order() {
    let (left, right) = classes();
    let first_origins = vec![
        ModelOrigin::from_class(&left),
        ModelOrigin::from_class(&right),
    ];
    let second_origins = vec![
        ModelOrigin::from_class(&right),
        ModelOrigin::from_class(&left),
    ];
    let first = RelationalQuery::new(scan(
        &[4],
        &["value"],
        origin(FILE, 1, 4, first_origins),
        left.clone(),
    ));
    let second = RelationalQuery::new(scan(
        &[99],
        &["value"],
        origin(FILE, 1, 4, second_origins),
        left,
    ));

    let first = normalized(&first);
    let second = normalized(&second);
    assert_eq!(first.equivalence_key(), second.equivalence_key());
    assert_eq!(first.structural_key(), second.structural_key());
}

#[test]
fn exact_identity_project_is_eliminated_and_normalization_is_idempotent() {
    let source = origin(FILE, 1, 20, Vec::new());
    let base = query(&[3, 8], &["left", "right"], source.clone());
    let projected = RelationalQuery::new(identity_project(base.root().clone(), source));

    let once = normalized(&projected);
    assert!(matches!(once.root().operator(), RelationOperator::Scan(_)));
    assert_eq!(once.equivalence_key(), &key(&base));

    let twice = normalized(&RelationalQuery::new(once.root().clone()));
    assert_eq!(once.equivalence_key(), twice.equivalence_key());
    assert_eq!(once.root(), twice.root());
}

#[test]
fn identity_shaped_projects_retain_distinct_relation_facts() {
    let source = origin(FILE, 1, 20, Vec::new());
    let base = query(&[3, 8], &["left", "right"], source.clone());
    let fact_origin = origin(FILE, 21, 24, Vec::new());
    let facts = RelationFacts::new(
        Knowledge::proven(
            vec![CandidateKey::new(vec![base.output().columns()[0].id()])],
            fact_origin.clone(),
        ),
        Knowledge::proven(RowSemantics::Set, fact_origin),
    );
    let projected = RelationalQuery::new(identity_project_with_metadata(
        base.root().clone(),
        source,
        facts.clone(),
        Knowledge::unknown(),
    ));

    let normalized = normalized(&projected);
    assert!(matches!(
        normalized.root().operator(),
        RelationOperator::Project { .. }
    ));
    assert_eq!(normalized.root().facts(), &facts);
    assert_ne!(normalized.equivalence_key(), &key(&base));
}

#[test]
fn identity_shaped_projects_retain_direct_read_totality_evidence() {
    let source = origin(FILE, 1, 20, Vec::new());
    let base = query(&[3, 8], &["left", "right"], source.clone());
    let totality = Knowledge::proven(Totality::Total, origin(FILE, 21, 24, Vec::new()));
    let projected = RelationalQuery::new(identity_project_with_metadata(
        base.root().clone(),
        source,
        RelationFacts::unknown(),
        totality.clone(),
    ));

    let normalized = normalized(&projected);
    let RelationOperator::Project { projections, .. } = normalized.root().operator() else {
        panic!("direct-read totality evidence must retain the project");
    };
    assert!(
        projections
            .iter()
            .all(|projection| projection.expression().totality() == &totality)
    );
    assert_ne!(normalized.equivalence_key(), &key(&base));
}

#[test]
fn literal_true_filters_are_eliminated_and_keep_audit_provenance() {
    let source = origin(FILE, 1, 20, Vec::new());
    let base = query(&[3, 8], &["left", "right"], source.clone());
    let inner = filter(
        base.root().clone(),
        true_predicate(origin(FILE, 21, 25, Vec::new())),
        RelationFacts::unknown(),
        origin(FILE, 20, 26, Vec::new()),
    );
    let filtered = RelationalQuery::new(filter(
        inner,
        true_predicate(origin(FILE, 27, 31, Vec::new())),
        RelationFacts::unknown(),
        origin(FILE, 26, 32, Vec::new()),
    ));

    let once = normalized(&filtered);
    assert!(matches!(once.root().operator(), RelationOperator::Scan(_)));
    assert_eq!(once.equivalence_key(), &key(&base));
    assert_ne!(once.structural_key(), normalized(&base).structural_key());

    let twice = normalized(&RelationalQuery::new(once.root().clone()));
    assert_eq!(once.equivalence_key(), twice.equivalence_key());
    assert_eq!(once.root(), twice.root());
}

#[test]
fn literal_true_filter_eliminates_with_matching_proven_relation_facts() {
    let source = origin(FILE, 1, 20, Vec::new());
    let (class, _) = classes();
    let output = RelationSchema::new(vec![column(3, "value", source.clone())])
        .expect("fixture schema is valid");
    let facts = RelationFacts::new(
        Knowledge::proven(
            vec![CandidateKey::new(vec![output.columns()[0].id()])],
            origin(FILE, 21, 24, Vec::new()),
        ),
        Knowledge::proven(RowSemantics::Bag, origin(FILE, 21, 24, Vec::new())),
    );
    let input = RelationExpression::new(
        RelationOperator::Scan(RelationSource::Class(class)),
        output,
        facts.clone(),
        source.clone(),
    )
    .expect("fixture scan is valid");
    let filtered = RelationalQuery::new(filter(
        input.clone(),
        true_predicate(origin(FILE, 25, 29, Vec::new())),
        facts.clone(),
        origin(FILE, 24, 30, Vec::new()),
    ));

    let normalized = normalized(&filtered);
    assert!(matches!(
        normalized.root().operator(),
        RelationOperator::Scan(_)
    ));
    assert_eq!(normalized.root().facts(), &facts);
    assert_eq!(
        normalized.equivalence_key(),
        &key(&RelationalQuery::new(input))
    );
}

#[test]
fn literal_true_filter_guards_retain_forged_facts_and_totality_evidence() {
    let source = origin(FILE, 1, 20, Vec::new());
    let base = query(&[3, 8], &["left", "right"], source.clone());
    let facts = RelationFacts::new(
        Knowledge::proven(
            vec![CandidateKey::new(vec![base.output().columns()[0].id()])],
            origin(FILE, 21, 24, Vec::new()),
        ),
        Knowledge::proven(RowSemantics::Set, origin(FILE, 21, 24, Vec::new())),
    );
    let facts_changed = RelationalQuery::new(filter(
        base.root().clone(),
        true_predicate(origin(FILE, 25, 29, Vec::new())),
        facts.clone(),
        origin(FILE, 24, 30, Vec::new()),
    ));
    let normalized_facts = normalized(&facts_changed);
    assert!(matches!(
        normalized_facts.root().operator(),
        RelationOperator::Filter { .. }
    ));
    assert_eq!(normalized_facts.root().facts(), &facts);

    let totality = Knowledge::proven(Totality::Total, origin(FILE, 31, 35, Vec::new()));
    let totality_changed = RelationalQuery::new(filter(
        base.root().clone(),
        boolean_predicate(true, origin(FILE, 36, 40, Vec::new()), totality.clone()),
        RelationFacts::unknown(),
        origin(FILE, 35, 41, Vec::new()),
    ));
    let normalized_totality = normalized(&totality_changed);
    let RelationOperator::Filter { predicate, .. } = normalized_totality.root().operator() else {
        panic!("literal-true totality evidence must retain the filter");
    };
    assert_eq!(predicate.totality(), &totality);
}

#[test]
fn literal_false_filters_remain_frozen() {
    let source = origin(FILE, 1, 20, Vec::new());
    let base = query(&[3, 8], &["left", "right"], source.clone());
    let filtered = RelationalQuery::new(filter(
        base.root().clone(),
        boolean_predicate(
            false,
            origin(FILE, 21, 26, Vec::new()),
            Knowledge::unknown(),
        ),
        RelationFacts::unknown(),
        origin(FILE, 20, 27, Vec::new()),
    ));

    let normalized = normalized(&filtered);
    assert!(matches!(
        normalized.root().operator(),
        RelationOperator::Filter { predicate, .. }
            if matches!(predicate.operator(), ScalarOperator::Literal(ScalarLiteral::Boolean(false)))
    ));
}

#[test]
fn aliases_and_output_column_order_are_not_identity_projects() {
    let source = origin(FILE, 1, 20, Vec::new());
    let base = query(&[3, 8], &["left", "right"], source.clone());
    let input = base.root().clone();
    let first = input.schema().columns()[0].clone();
    let second = input.schema().columns()[1].clone();

    let alias = Column::new(
        ColumnId::new(100),
        "renamed".parse().expect("fixture name is valid"),
        first.type_ref().clone(),
        first.multiplicity(),
        first.nullability(),
        source.clone(),
    );
    let alias_project = RelationExpression::new(
        RelationOperator::Project {
            input: Box::new(input.clone()),
            projections: vec![Projection::new(
                alias.id(),
                scalar_column(&first, source.clone()),
            )],
        },
        RelationSchema::new(vec![alias]).expect("fixture schema is valid"),
        RelationFacts::unknown(),
        source.clone(),
    )
    .expect("alias project is valid");
    let alias_query = RelationalQuery::new(alias_project);
    assert!(matches!(
        normalized(&alias_query).root().operator(),
        RelationOperator::Project { .. }
    ));
    assert_ne!(key(&base), key(&alias_query));

    let reordered_schema =
        RelationSchema::new(vec![second.clone(), first.clone()]).expect("fixture schema is valid");
    let reordered_project = RelationExpression::new(
        RelationOperator::Project {
            input: Box::new(input),
            projections: vec![
                Projection::new(second.id(), scalar_column(&second, source.clone())),
                Projection::new(first.id(), scalar_column(&first, source.clone())),
            ],
        },
        reordered_schema,
        RelationFacts::unknown(),
        source,
    )
    .expect("reordered project is valid");
    let reordered_query = RelationalQuery::new(reordered_project);
    assert!(matches!(
        normalized(&reordered_query).root().operator(),
        RelationOperator::Project { .. }
    ));
    assert_ne!(key(&base), key(&reordered_query));
}

#[test]
fn pinned_repeated_distinct_witness_drives_normalization() {
    let witness = SEMANTIC_CORPUS_CASES
        .lines()
        .map(|line| {
            serde_json::from_str::<Value>(line)
                .unwrap_or_else(|error| panic!("semantic corpus row is invalid: {error}"))
        })
        .find(|case| {
            case.get("id").and_then(Value::as_str) == Some("repeated-distinct-is-idempotent")
        })
        .unwrap_or_else(|| panic!("semantic corpus lacks repeated distinct witness"));
    assert_eq!(
        witness.get("candidate").and_then(Value::as_str),
        Some("collapse-repeated-distinct")
    );
    assert_eq!(
        witness.get("outcome").and_then(Value::as_str),
        Some("equal")
    );
    assert_eq!(
        witness.pointer("/left/lambda").and_then(Value::as_str),
        Some("|[1, 1, 2]->removeDuplicates()->removeDuplicates()")
    );
    assert_eq!(
        witness.pointer("/right/lambda").and_then(Value::as_str),
        Some("|[1, 1, 2]->removeDuplicates()")
    );
    assert_eq!(
        witness.pointer("/left/result"),
        witness.pointer("/right/result")
    );

    let source = origin(FILE, 1, 20, Vec::new());
    let base = query(&[3, 8], &["left", "right"], source.clone());
    let single = distinct(
        base.root().clone(),
        RelationFacts::unknown(),
        origin(FILE, 21, 25, Vec::new()),
    );
    let nested = RelationalQuery::new(distinct(
        single.clone(),
        RelationFacts::unknown(),
        origin(FILE, 26, 30, Vec::new()),
    ));

    let normalized = normalized(&nested);
    assert!(matches!(
        normalized.root().operator(),
        RelationOperator::Distinct { input } if matches!(input.operator(), RelationOperator::Scan(_))
    ));
    assert_eq!(
        normalized.equivalence_key(),
        &key(&RelationalQuery::new(single))
    );
}

#[test]
fn repeated_distinct_preserves_matching_facts_and_full_input_provenance() {
    let source = origin(FILE, 1, 20, Vec::new());
    let base = query(&[3, 8], &["left", "right"], source.clone());
    let fact_origin = origin(FILE, 21, 24, Vec::new());
    let facts = RelationFacts::new(
        Knowledge::proven(
            vec![CandidateKey::new(vec![base.output().columns()[0].id()])],
            fact_origin.clone(),
        ),
        Knowledge::proven(RowSemantics::Set, fact_origin),
    );
    let inner = distinct(
        base.root().clone(),
        facts.clone(),
        origin(FILE, 25, 29, Vec::new()),
    );
    let first = RelationalQuery::new(distinct(
        inner.clone(),
        facts.clone(),
        origin(FILE, 30, 34, Vec::new()),
    ));
    let second = RelationalQuery::new(distinct(
        inner,
        facts.clone(),
        origin(FILE, 35, 39, Vec::new()),
    ));

    let first_normalized = normalized(&first);
    let second_normalized = normalized(&second);
    assert!(matches!(
        first_normalized.root().operator(),
        RelationOperator::Distinct { input } if matches!(input.operator(), RelationOperator::Scan(_))
    ));
    assert_eq!(first_normalized.root().schema(), first.output());
    assert_eq!(first_normalized.root().facts(), &facts);
    assert_eq!(
        first_normalized.equivalence_key(),
        second_normalized.equivalence_key()
    );
    assert_ne!(
        first_normalized.structural_key(),
        second_normalized.structural_key(),
        "the structural key must retain the eliminated outer distinct provenance"
    );

    let twice = normalized(&RelationalQuery::new(first_normalized.root().clone()));
    assert_eq!(first_normalized.equivalence_key(), twice.equivalence_key());
    assert_eq!(first_normalized.root(), twice.root());
}

#[test]
fn repeated_distinct_requires_exact_relation_facts() {
    let source = origin(FILE, 1, 20, Vec::new());
    let base = query(&[3, 8], &["left", "right"], source.clone());
    let inner_facts = RelationFacts::new(
        Knowledge::unknown(),
        Knowledge::proven(RowSemantics::Set, origin(FILE, 21, 25, Vec::new())),
    );
    let outer_facts = RelationFacts::new(
        Knowledge::unknown(),
        Knowledge::proven(RowSemantics::Set, origin(FILE, 26, 30, Vec::new())),
    );
    let inner = distinct(
        base.root().clone(),
        inner_facts,
        origin(FILE, 31, 35, Vec::new()),
    );
    let guarded = RelationalQuery::new(distinct(
        inner,
        outer_facts.clone(),
        origin(FILE, 36, 40, Vec::new()),
    ));

    let normalized = normalized(&guarded);
    assert!(matches!(
        normalized.root().operator(),
        RelationOperator::Distinct { input }
            if matches!(input.operator(), RelationOperator::Distinct { .. })
    ));
    assert_eq!(normalized.root().facts(), &outer_facts);
}

#[test]
fn single_and_nonconsecutive_distinct_forms_remain_frozen() {
    let source = origin(FILE, 1, 20, Vec::new());
    let base = query(&[3, 8], &["left", "right"], source.clone());
    let single = distinct(
        base.root().clone(),
        RelationFacts::unknown(),
        origin(FILE, 21, 25, Vec::new()),
    );
    assert!(matches!(
        normalized(&RelationalQuery::new(single)).root().operator(),
        RelationOperator::Distinct { input } if matches!(input.operator(), RelationOperator::Scan(_))
    ));

    let sort = RelationExpression::new(
        RelationOperator::Sort {
            input: Box::new(base.root().clone()),
            keys: vec![SortKey::new(
                base.output().columns()[0].id(),
                SortDirection::Descending,
                source.clone(),
            )],
        },
        base.output().clone(),
        RelationFacts::unknown(),
        source.clone(),
    )
    .expect("sort is valid");
    assert!(matches!(
        normalized(&RelationalQuery::new(sort)).root().operator(),
        RelationOperator::Sort { .. }
    ));

    let inner = distinct(
        base.root().clone(),
        RelationFacts::unknown(),
        origin(FILE, 26, 30, Vec::new()),
    );
    let sorted = RelationExpression::new(
        RelationOperator::Sort {
            input: Box::new(inner),
            keys: vec![SortKey::new(
                base.output().columns()[0].id(),
                SortDirection::Descending,
                origin(FILE, 31, 35, Vec::new()),
            )],
        },
        base.output().clone(),
        RelationFacts::unknown(),
        origin(FILE, 30, 36, Vec::new()),
    )
    .expect("sorted distinct fixture is valid");
    let normalized = normalized(&RelationalQuery::new(distinct(
        sorted,
        RelationFacts::unknown(),
        origin(FILE, 36, 40, Vec::new()),
    )));
    assert!(matches!(
        normalized.root().operator(),
        RelationOperator::Distinct { input }
            if matches!(
                input.operator(),
                RelationOperator::Sort { input: sorted_input, .. }
                    if matches!(sorted_input.operator(), RelationOperator::Distinct { .. })
            )
    ));
}

#[test]
fn selected_distinct_and_join_schema_order_remain_observable() {
    let source = origin(FILE, 1, 20, Vec::new());
    let base = query(&[3, 8], &["left", "right"], source.clone());
    let selected = RelationExpression::new(
        RelationOperator::DistinctOn {
            input: Box::new(base.root().clone()),
            columns: vec![base.output().columns()[1].id()],
        },
        RelationSchema::new(vec![base.output().columns()[1].clone()])
            .expect("fixture selected schema is valid"),
        RelationFacts::unknown(),
        source.clone(),
    )
    .expect("selected distinct is valid");
    let selected = RelationalQuery::new(selected);
    assert!(matches!(
        normalized(&selected).root().operator(),
        RelationOperator::DistinctOn { .. }
    ));
    let neighboring = RelationalQuery::new(distinct(
        selected.root().clone(),
        RelationFacts::unknown(),
        origin(FILE, 21, 25, Vec::new()),
    ));
    assert!(matches!(
        normalized(&neighboring).root().operator(),
        RelationOperator::Distinct { input }
            if matches!(input.operator(), RelationOperator::DistinctOn { .. })
    ));

    let (left_class, right_class) = classes();
    let left = scan(&[10], &["left"], source.clone(), left_class);
    let right = scan(&[30], &["right"], source.clone(), right_class);
    let schema = RelationSchema::new(
        left.schema()
            .columns()
            .iter()
            .chain(right.schema().columns())
            .cloned()
            .collect(),
    )
    .expect("join schema is valid");
    let join = RelationExpression::new(
        RelationOperator::Join {
            kind: pure_analyzer_analysis::JoinKind::Inner,
            left: Box::new(left),
            right: Box::new(right),
            condition: true_predicate(source.clone()),
        },
        schema,
        RelationFacts::unknown(),
        source,
    )
    .expect("join is valid");
    assert!(matches!(
        normalized(&RelationalQuery::new(join)).root().operator(),
        RelationOperator::Join { .. }
    ));
}

#[test]
fn exhausted_budget_is_typed_ind_missing_rewrite() {
    let query = query(&[3], &["value"], origin(FILE, 1, 4, Vec::new()));
    let outcome = normalize_relational_query_with_budget(&query, NormalizationBudget::new(0));
    let Some(failure) = outcome.failure() else {
        panic!("zero budget must stop normalization");
    };
    assert_eq!(failure.reason(), ReasonCode::IndMissingRewrite);

    let project = RelationalQuery::new(identity_project(
        query.root().clone(),
        origin(FILE, 5, 9, Vec::new()),
    ));
    let outcome = normalize_relational_query_with_budget(&project, NormalizationBudget::new(1));
    assert!(matches!(
        outcome,
        NormalizationOutcome::Indecisive(ref failure)
            if failure.reason() == ReasonCode::IndMissingRewrite
    ));
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(32))]

    #[test]
    fn arbitrary_repeated_distinct_column_allocations_alpha_normalize(
        first in any::<u16>(),
        second in any::<u16>(),
        other_first in any::<u16>(),
        other_second in any::<u16>(),
    ) {
        prop_assume!(first != second);
        prop_assume!(other_first != other_second);
        let left = query(
            &[u32::from(first), u32::from(second)],
            &["left", "right"],
            origin(FILE, 1, 9, Vec::new()),
        );
        let left = RelationalQuery::new(repeated_distinct(
            left.root().clone(),
            RelationFacts::unknown(),
            origin(FILE, 10, 14, Vec::new()),
            origin(FILE, 15, 19, Vec::new()),
        ));
        let right = query(
            &[u32::from(other_first), u32::from(other_second)],
            &["left", "right"],
            origin(FILE + 1, 20, 29, Vec::new()),
        );
        let right = RelationalQuery::new(repeated_distinct(
            right.root().clone(),
            RelationFacts::unknown(),
            origin(FILE + 1, 30, 34, Vec::new()),
            origin(FILE + 1, 35, 39, Vec::new()),
        ));
        prop_assert_eq!(key(&left), key(&right));
    }

    #[test]
    fn nested_identity_projects_have_one_confluent_normal_form(
        layers in 0usize..8,
    ) {
        let base = query(&[3, 8], &["left", "right"], origin(FILE, 1, 9, Vec::new()));
        let mut root = base.root().clone();
        for layer in 0..layers {
            root = identity_project(
                root,
                origin(FILE, u32::try_from(layer).expect("small layer"), 99, Vec::new()),
            );
        }
        let normalized = normalized(&RelationalQuery::new(root));
        prop_assert_eq!(normalized.equivalence_key(), &key(&base));
        prop_assert!(matches!(normalized.root().operator(), RelationOperator::Scan(_)));
    }
}
