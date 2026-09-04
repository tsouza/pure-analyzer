//! End-to-end proof contracts for conservative relational normalization.

use proptest::prelude::*;
use pure_analyzer_analysis::{
    CandidateKey, Column, ColumnId, EquivalenceKey, IrOrigin, Knowledge, ModelOrigin,
    NormalizationBudget, NormalizationOutcome, Nullability, Projection, ProjectionKind,
    RelationExpression, RelationExpressionError, RelationFacts, RelationOperator, RelationSchema,
    RelationSource, RelationalQuery, RowSemantics, ScalarExpression, ScalarLiteral, ScalarOperator,
    SortDirection, SortKey, SourceSpan, Totality, normalize_relational_query,
    normalize_relational_query_with_budget,
};
use pure_analyzer_diagnostics::{FileId, ReasonCode, TextRange, TextSize};
use pure_analyzer_model::{Multiplicity, PmcdDocument, QName, TypeRef, load_pmcd_documents};
use pure_analyzer_resolve::{Resolution, ResolvedClass, Resolver};
use serde_json::Value;

const FILE: u32 = 31;
const EXACTLY_ONE: u32 = 1;
const MIXED_REWRITE_NORMALIZATION_STEPS: usize = 8;
const SEMANTIC_CORPUS_CASES: &str = include_str!("../corpus/legend-4.113.0/cases.jsonl");

/// The three rewrites `normalizer.rs`'s guards recognize by IR shape.
///
/// All three are exercised here via hand-built IR (see `identity_project`,
/// `filter`, `repeated_distinct` below), which is a legitimate way to unit
/// test rewrite *logic* in isolation. It does not by itself mean lowering can
/// ever produce that shape: `IdentityProject` currently cannot — every real
/// `Project` output column gets a fresh `ColumnId` that never coincides with
/// the input column it reads, so `is_identity_project` is frozen pending
/// issue #410. `LiteralTrueFilter` and `RepeatedDistinct` are both reachable
/// from real lowered IR (the latter since issue #281); see
/// `tests/comparison.rs`'s production-path regressions for those two.
#[derive(Clone, Copy)]
enum IntrinsicRewrite {
    IdentityProject,
    LiteralTrueFilter,
    RepeatedDistinct,
}

const INTRINSIC_REWRITE_PERMUTATIONS: [[IntrinsicRewrite; 3]; 6] = [
    [
        IntrinsicRewrite::IdentityProject,
        IntrinsicRewrite::LiteralTrueFilter,
        IntrinsicRewrite::RepeatedDistinct,
    ],
    [
        IntrinsicRewrite::IdentityProject,
        IntrinsicRewrite::RepeatedDistinct,
        IntrinsicRewrite::LiteralTrueFilter,
    ],
    [
        IntrinsicRewrite::LiteralTrueFilter,
        IntrinsicRewrite::IdentityProject,
        IntrinsicRewrite::RepeatedDistinct,
    ],
    [
        IntrinsicRewrite::LiteralTrueFilter,
        IntrinsicRewrite::RepeatedDistinct,
        IntrinsicRewrite::IdentityProject,
    ],
    [
        IntrinsicRewrite::RepeatedDistinct,
        IntrinsicRewrite::IdentityProject,
        IntrinsicRewrite::LiteralTrueFilter,
    ],
    [
        IntrinsicRewrite::RepeatedDistinct,
        IntrinsicRewrite::LiteralTrueFilter,
        IntrinsicRewrite::IdentityProject,
    ],
];

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

fn zero_or_one() -> Multiplicity {
    Multiplicity::new(0, Some(EXACTLY_ONE)).expect("fixture multiplicity is valid")
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

fn nullable_scalar_input(name: &str, type_name: &str, source: IrOrigin) -> RelationExpression {
    let (class, _) = classes();
    let schema = RelationSchema::new(vec![Column::new(
        ColumnId::new(1),
        name.parse().expect("fixture column name is valid"),
        type_ref(type_name),
        zero_or_one(),
        Nullability::Nullable,
        source.clone(),
    )])
    .expect("fixture schema is valid");
    RelationExpression::new(
        RelationOperator::Scan(RelationSource::Class(class)),
        schema,
        RelationFacts::unknown(),
        source,
    )
    .expect("fixture scan is valid")
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
            kind: ProjectionKind::Relation,
        },
        schema,
        facts,
        source,
    )
    .expect("identity project is valid")
}

fn direct_read_subset_project(
    input: RelationExpression,
    selected: &[ColumnId],
    facts: RelationFacts,
    source: IrOrigin,
) -> RelationExpression {
    let schema = RelationSchema::new(
        selected
            .iter()
            .map(|column| {
                input
                    .schema()
                    .column(*column)
                    .unwrap_or_else(|| panic!("selected fixture column must exist"))
                    .clone()
            })
            .collect(),
    )
    .expect("subset schema is valid");
    let projections = schema
        .columns()
        .iter()
        .map(|column| Projection::new(column.id(), scalar_column(column, source.clone())))
        .collect();
    RelationExpression::new(
        RelationOperator::Project {
            input: Box::new(input),
            projections,
            kind: ProjectionKind::Relation,
        },
        schema,
        facts,
        source,
    )
    .expect("direct-read subset project is valid")
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

fn mixed_intrinsic_query(
    input: RelationExpression,
    permutation: &[IntrinsicRewrite],
    file: u32,
) -> RelationalQuery {
    let root = permutation
        .iter()
        .copied()
        .enumerate()
        .fold(input, |input, (index, rewrite)| {
            let start = 10 + u32::try_from(index).expect("small permutation index") * 10;
            match rewrite {
                IntrinsicRewrite::IdentityProject => {
                    identity_project(input, origin(file, start, start + 4, Vec::new()))
                }
                IntrinsicRewrite::LiteralTrueFilter => filter(
                    input,
                    true_predicate(origin(file, start, start + 2, Vec::new())),
                    RelationFacts::unknown(),
                    origin(file, start, start + 4, Vec::new()),
                ),
                IntrinsicRewrite::RepeatedDistinct => repeated_distinct(
                    input,
                    RelationFacts::unknown(),
                    origin(file, start, start + 2, Vec::new()),
                    origin(file, start + 2, start + 4, Vec::new()),
                ),
            }
        });
    RelationalQuery::new(root)
}

fn normalized(query: &RelationalQuery) -> pure_analyzer_analysis::NormalizedQuery {
    match normalize_relational_query(query) {
        NormalizationOutcome::Normalized(value) => *value,
        NormalizationOutcome::Indecisive(value) => {
            panic!("normalization unexpectedly stopped: {value:?}")
        }
    }
}

fn normalized_with_budget(
    query: &RelationalQuery,
    budget: NormalizationBudget,
) -> pure_analyzer_analysis::NormalizedQuery {
    match normalize_relational_query_with_budget(query, budget) {
        NormalizationOutcome::Normalized(value) => *value,
        NormalizationOutcome::Indecisive(value) => {
            panic!("normalization unexpectedly stopped: {value:?}")
        }
    }
}

fn key(query: &RelationalQuery) -> EquivalenceKey {
    normalized(query).equivalence_key().clone()
}

fn semantic_witness(id: &str) -> Value {
    SEMANTIC_CORPUS_CASES
        .lines()
        .map(|line| {
            serde_json::from_str::<Value>(line)
                .unwrap_or_else(|error| panic!("semantic corpus row is invalid: {error}"))
        })
        .find(|case| case.get("id").and_then(Value::as_str) == Some(id))
        .unwrap_or_else(|| panic!("semantic corpus lacks {id} witness"))
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
                    kind: ProjectionKind::Relation,
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
            kind: ProjectionKind::Relation,
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
            kind: ProjectionKind::Relation,
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
fn pinned_repeated_distinct_witness_collapses_across_distinct_origins() {
    let witness = semantic_witness("repeated-distinct-is-idempotent");
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

    // Each level below proves its own `RelationFacts` from its own distinct
    // source span, exactly as `lower_bare_distinct` does for a real
    // `->distinct()->distinct()` chain — never one `RelationFacts` value
    // shared across levels, which no real lowering call site produces. See
    // `tests/comparison.rs`'s `repeated_distinct_collapses_from_real_lowered_source`
    // for the same claim proven through real Pure source and lowering.
    let source = origin(FILE, 1, 20, Vec::new());
    let base = query(&[3, 8], &["left", "right"], source.clone());
    let bag_facts_at = |start: u32, end: u32| {
        RelationFacts::new(
            Knowledge::unknown(),
            Knowledge::proven(RowSemantics::Bag, origin(FILE, start, end, Vec::new())),
        )
    };
    let (class, _) = classes();
    let bag_input = RelationExpression::new(
        RelationOperator::Scan(RelationSource::Class(class)),
        base.output().clone(),
        bag_facts_at(21, 25),
        origin(FILE, 21, 25, Vec::new()),
    )
    .expect("bag fixture scan is valid");
    let single = distinct(
        bag_input,
        bag_facts_at(26, 30),
        origin(FILE, 26, 30, Vec::new()),
    );
    let nested = RelationalQuery::new(distinct(
        single.clone(),
        bag_facts_at(31, 35),
        origin(FILE, 31, 35, Vec::new()),
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
    let candidate_key = || vec![CandidateKey::new(vec![base.output().columns()[0].id()])];
    let facts_at = |start: u32, end: u32| {
        let fact_origin = origin(FILE, start, end, Vec::new());
        RelationFacts::new(
            Knowledge::proven(candidate_key(), fact_origin.clone()),
            Knowledge::proven(RowSemantics::Set, fact_origin),
        )
    };
    // The inner and both outer wrappers each prove the identical
    // (candidate-key, row-semantics) conclusion from their own distinct
    // origin, matching what real per-node lowering produces — collapsing
    // must retain the surviving inner node's own facts/provenance, not the
    // discarded outer node's, even though the two are `matches()`-equal.
    let inner_facts = facts_at(21, 24);
    let inner = distinct(
        base.root().clone(),
        inner_facts.clone(),
        origin(FILE, 25, 29, Vec::new()),
    );
    let first = RelationalQuery::new(distinct(
        inner.clone(),
        facts_at(30, 34),
        origin(FILE, 30, 34, Vec::new()),
    ));
    let second = RelationalQuery::new(distinct(
        inner,
        facts_at(35, 39),
        origin(FILE, 35, 39, Vec::new()),
    ));

    let first_normalized = normalized(&first);
    let second_normalized = normalized(&second);
    assert!(matches!(
        first_normalized.root().operator(),
        RelationOperator::Distinct { input } if matches!(input.operator(), RelationOperator::Scan(_))
    ));
    assert_eq!(first_normalized.root().schema(), first.output());
    assert_eq!(
        first_normalized.root().facts(),
        &inner_facts,
        "collapsing the redundant outer Distinct must retain the surviving \
         inner node's own facts and provenance, not the discarded outer \
         node's merely-matching facts"
    );
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
fn repeated_distinct_requires_matching_relation_facts_value() {
    // A same-VALUE, different-origin pair now collapses (issue #281); only a
    // genuine value mismatch — here Bag vs. Set row semantics — must still
    // block the rewrite. `RelationFacts::matches` is origin-insensitive, not
    // a rubber stamp.
    let source = origin(FILE, 1, 20, Vec::new());
    let base = query(&[3, 8], &["left", "right"], source.clone());
    let inner_facts = RelationFacts::new(
        Knowledge::unknown(),
        Knowledge::proven(RowSemantics::Bag, origin(FILE, 21, 25, Vec::new())),
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

fn assert_pinned_different_witness(id: &str, candidate: &str) {
    let witness = semantic_witness(id);
    assert_eq!(
        witness.get("candidate").and_then(Value::as_str),
        Some(candidate)
    );
    assert_eq!(
        witness.get("outcome").and_then(Value::as_str),
        Some("different")
    );
    assert_ne!(
        witness.pointer("/left/result"),
        witness.pointer("/right/result")
    );
}

fn assert_single_distinct_is_retained(base: &RelationalQuery) {
    let single = distinct(
        base.root().clone(),
        RelationFacts::unknown(),
        origin(FILE, 21, 25, Vec::new()),
    );
    assert!(matches!(
        normalized(&RelationalQuery::new(single)).root().operator(),
        RelationOperator::Distinct { input } if matches!(input.operator(), RelationOperator::Scan(_))
    ));
}

fn assert_nested_sort_is_retained(base: &RelationalQuery, source: IrOrigin) {
    let inner_sort = RelationExpression::new(
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
    .expect("inner sort is valid");
    let nested_sort = RelationExpression::new(
        RelationOperator::Sort {
            input: Box::new(inner_sort),
            keys: vec![SortKey::new(
                base.output().columns()[1].id(),
                SortDirection::Ascending,
                origin(FILE, 21, 25, Vec::new()),
            )],
        },
        base.output().clone(),
        RelationFacts::unknown(),
        origin(FILE, 20, 26, Vec::new()),
    )
    .expect("nested sort is valid");
    assert!(matches!(
        normalized(&RelationalQuery::new(nested_sort)).root().operator(),
        RelationOperator::Sort { input, .. }
            if matches!(input.operator(), RelationOperator::Sort { .. })
    ));
}

fn assert_nonconsecutive_distinct_is_retained(base: &RelationalQuery) {
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
fn pinned_order_and_bag_witnesses_keep_nested_sort_and_distinct_guards_frozen() {
    assert_pinned_different_witness("nested-sort-is-not-outer-sort", "collapse-nested-sort");
    assert_pinned_different_witness("distinct-is-not-an-identity-on-a-bag", "elide-distinct");

    let source = origin(FILE, 1, 20, Vec::new());
    let base = query(&[3, 8], &["left", "right"], source.clone());
    assert_single_distinct_is_retained(&base);
    assert_nested_sort_is_retained(&base, source);
    assert_nonconsecutive_distinct_is_retained(&base);
}

#[test]
fn pinned_three_valued_witnesses_keep_nullable_forms_frozen_and_indecisive() {
    let nullable_predicate_witness = semantic_witness("nullable-predicate-is-not-literal-true");
    assert_eq!(
        nullable_predicate_witness
            .get("candidate")
            .and_then(Value::as_str),
        Some("elide-nullable-predicate-as-true")
    );
    assert_eq!(
        nullable_predicate_witness
            .get("outcome")
            .and_then(Value::as_str),
        Some("different")
    );
    assert_ne!(
        nullable_predicate_witness.pointer("/left/result"),
        nullable_predicate_witness.pointer("/right/result")
    );

    let nullable_complement_witness =
        semantic_witness("nullable-complement-tautology-is-indecisive");
    assert_eq!(
        nullable_complement_witness
            .get("candidate")
            .and_then(Value::as_str),
        Some("simplify-complement-to-true")
    );
    assert_eq!(
        nullable_complement_witness
            .get("outcome")
            .and_then(Value::as_str),
        Some("indecisive")
    );
    assert!(
        nullable_complement_witness
            .get("reason")
            .and_then(Value::as_str)
            .is_some_and(|reason| !reason.is_empty())
    );
    assert!(
        nullable_complement_witness
            .pointer("/probe/left/lambda")
            .and_then(Value::as_str)
            .is_some_and(|lambda| lambda.contains("optional == 1 || $x.optional != 1"))
    );

    let nullable_boolean =
        nullable_scalar_input("predicate", "Boolean", origin(FILE, 1, 9, Vec::new()));
    let direct_nullable_predicate = scalar_column(
        &nullable_boolean.schema().columns()[0],
        origin(FILE, 10, 19, Vec::new()),
    );
    let rejected = RelationExpression::new(
        RelationOperator::Filter {
            input: Box::new(nullable_boolean.clone()),
            predicate: direct_nullable_predicate,
        },
        nullable_boolean.schema().clone(),
        RelationFacts::unknown(),
        origin(FILE, 20, 29, Vec::new()),
    );
    assert_eq!(rejected, Err(RelationExpressionError::NonBooleanPredicate));

    // The corpus witness above pins that a real Legend Pure engine keeps
    // `x == 1 || x != 1` indecisive against `true` under three-valued null
    // semantics. There is no in-process companion check for that scenario:
    // `||` has no lowering producer (see `ScalarOperator`'s rustdoc), so a
    // hand-built `Or` predicate would exercise IR the pipeline can never
    // actually construct.
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

/// A `->distinct()->distinct()->...` pipe of arbitrary length lowers cleanly
/// (nothing in the parser or lowerer bounds pipe length), so
/// `DEFAULT_NORMALIZATION_STEP_LIMIT`'s node-count budget alone does not stop
/// `Normalizer::relation` from recursing once per node: it is a work budget,
/// not a stack-depth budget, and https://github.com/tsouza/pure-analyzer/issues/266
/// reproduced a process-aborting stack overflow well inside it, at 450 nested
/// calls. `Normalizer::relation`/`scalar` now carry an explicit depth budget
/// (`MAX_RELATIONAL_RECURSION_DEPTH` in `relational.rs`) independent of the
/// node-count one, and `RelationExpression`'s `Drop` impl unwinds its
/// `Box` chain iteratively so an unnormalized deep pipe cannot abort on drop
/// either. This proves both fixes hold on a worker stack smaller than any
/// default in this workspace.
#[test]
fn a_pipe_deeper_than_the_depth_budget_is_a_typed_missing_rewrite_not_an_abort() {
    // Pinned exactly, in both directions: lowering the budget silently
    // rejects ordinary short pipes; raising it past the frames
    // `on_a_small_stack` affords turns the budget into a process abort
    // instead of a typed result. See `MAX_RELATIONAL_RECURSION_DEPTH`.
    const EXPECTED_DEPTH_BUDGET: usize = 32;
    const OVER_BUDGET_DEPTH: usize = EXPECTED_DEPTH_BUDGET * 2;
    // Comfortably past DEFAULT_NORMALIZATION_STEP_LIMIT (4_096) too, so this
    // also exercises the `RelationExpression` drop fix at a depth the node
    // count budget alone would never have reached.
    const FAR_OVER_BUDGET_DEPTH: usize = 50_000;

    let base = query(&[3], &["value"], origin(FILE, 1, 4, Vec::new()));

    let at_budget = pipe(base.root().clone(), EXPECTED_DEPTH_BUDGET - 1);
    let outcome = on_a_small_stack(at_budget, normalize_relational_query);
    assert!(
        outcome.normalized().is_some(),
        "a pipe exactly inside the depth budget must normalize, got {outcome:?}"
    );

    let over_budget = pipe(base.root().clone(), OVER_BUDGET_DEPTH);
    let outcome = on_a_small_stack(over_budget, normalize_relational_query);
    let Some(failure) = outcome.failure() else {
        panic!(
            "a pipe {OVER_BUDGET_DEPTH} deep must stop at the depth budget instead of aborting, \
             got {outcome:?}"
        );
    };
    assert_eq!(failure.reason(), ReasonCode::IndMissingRewrite);

    let far_over_budget = pipe(base.root().clone(), FAR_OVER_BUDGET_DEPTH);
    let outcome = on_a_small_stack(far_over_budget, normalize_relational_query);
    assert!(
        outcome.failure().is_some(),
        "a pipe far past the depth budget must still stop cleanly, got {outcome:?}"
    );
}

/// Wrap `root` in `depth` consecutive `->distinct()` layers, iteratively.
///
/// This mirrors how the lowerer actually builds a pipe chain (a loop that
/// wraps the previous relation in a new node, not per-layer recursion), so
/// building the fixture itself never risks a stack overflow independent of
/// the code under test.
fn pipe(mut root: RelationExpression, depth: usize) -> RelationalQuery {
    let layer_source = origin(FILE, 0, 1, Vec::new());
    for _ in 0..depth {
        root = distinct(root, RelationFacts::unknown(), layer_source.clone());
    }
    RelationalQuery::new(root)
}

/// Run `lookup` on a thread with a smaller stack than any default in this
/// workspace, so a walk (or a drop of its input) that outgrows its depth
/// budget aborts here instead of in production. `query` is moved into the
/// thread and dropped there too, before it joins, so this also covers the
/// `RelationExpression` drop chain on a worker-sized stack rather than only
/// on the (larger) test-harness main thread.
///
/// A debug build is the worst case for per-frame stack cost (see the module
/// doc on `MAX_RELATIONAL_RECURSION_DEPTH`), so this covers the release
/// profile a fortiori, mirroring
/// `pure-analyzer-resolve`'s `on_a_small_stack` precedent.
fn on_a_small_stack<T: Send + 'static>(
    query: RelationalQuery,
    lookup: impl FnOnce(&RelationalQuery) -> T + Send + 'static,
) -> T {
    const WORKER_STACK_BYTES: usize = 1024 * 1024;

    std::thread::Builder::new()
        .stack_size(WORKER_STACK_BYTES)
        .spawn(move || {
            let result = lookup(&query);
            drop(query);
            result
        })
        .expect("worker thread must spawn")
        .join()
        .expect("a bounded normalization and drop must not abort its thread")
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

    #[test]
    fn mixed_intrinsic_rewrite_permutations_converge_with_exact_fuel(
        first in any::<u16>(),
        second in any::<u16>(),
        other_first in any::<u16>(),
        other_second in any::<u16>(),
    ) {
        prop_assume!(first != second);
        prop_assume!(other_first != other_second);
        let left_base = query(
            &[u32::from(first), u32::from(second)],
            &["left", "right"],
            origin(FILE, 1, 9, Vec::new()),
        );
        let right_base = query(
            &[u32::from(other_first), u32::from(other_second)],
            &["left", "right"],
            origin(FILE + 1, 1, 9, Vec::new()),
        );
        let expected_key = key(&RelationalQuery::new(distinct(
            left_base.root().clone(),
            RelationFacts::unknown(),
            origin(FILE, 90, 94, Vec::new()),
        )));

        for permutation in INTRINSIC_REWRITE_PERMUTATIONS {
            let left = mixed_intrinsic_query(left_base.root().clone(), &permutation, FILE);
            let right = mixed_intrinsic_query(
                right_base.root().clone(),
                &permutation,
                FILE + 1,
            );
            let exhausted = normalize_relational_query_with_budget(
                &left,
                NormalizationBudget::new(MIXED_REWRITE_NORMALIZATION_STEPS - 1),
            );
            prop_assert!(matches!(
                exhausted,
                NormalizationOutcome::Indecisive(ref failure)
                    if failure.reason() == ReasonCode::IndMissingRewrite
            ));

            let left = normalized_with_budget(
                &left,
                NormalizationBudget::new(MIXED_REWRITE_NORMALIZATION_STEPS),
            );
            let right = normalized_with_budget(
                &right,
                NormalizationBudget::new(MIXED_REWRITE_NORMALIZATION_STEPS),
            );
            let left_is_single_distinct = matches!(
                left.root().operator(),
                RelationOperator::Distinct { input } if input.as_ref() == left_base.root()
            );
            let right_is_single_distinct = matches!(
                right.root().operator(),
                RelationOperator::Distinct { input } if input.as_ref() == right_base.root()
            );
            prop_assert!(left_is_single_distinct);
            prop_assert!(right_is_single_distinct);
            prop_assert_eq!(left.equivalence_key(), &expected_key);
            prop_assert_eq!(left.equivalence_key(), right.equivalence_key());

            let twice = normalized(&RelationalQuery::new(left.root().clone()));
            prop_assert_eq!(left.equivalence_key(), twice.equivalence_key());
            prop_assert_eq!(left.root(), twice.root());
        }
    }

    #[test]
    fn direct_read_subsets_and_candidate_key_collections_preserve_fact_meaning(
        first in any::<u16>(),
        second in any::<u16>(),
        third in any::<u16>(),
    ) {
        prop_assume!(first != second);
        prop_assume!(first != third);
        prop_assume!(second != third);
        let base = query(
            &[u32::from(first), u32::from(second), u32::from(third)],
            &["first", "second", "third"],
            origin(FILE, 1, 9, Vec::new()),
        );
        let selected = [
            base.output().columns()[0].id(),
            base.output().columns()[1].id(),
        ];
        let first_key = CandidateKey::new(vec![selected[0]]);
        let second_key = CandidateKey::new(vec![selected[1]]);
        let key_origin = origin(FILE, 10, 14, Vec::new());
        let row_origin = origin(FILE, 15, 19, Vec::new());
        let ordered_facts = RelationFacts::new(
            Knowledge::proven(
                vec![first_key.clone(), second_key.clone()],
                key_origin.clone(),
            ),
            Knowledge::proven(RowSemantics::Set, row_origin.clone()),
        );
        let reordered_facts = RelationFacts::new(
            Knowledge::proven(
                vec![second_key.clone(), first_key.clone()],
                key_origin.clone(),
            ),
            Knowledge::proven(RowSemantics::Set, row_origin.clone()),
        );
        let distinct_facts = RelationFacts::new(
            Knowledge::proven(vec![first_key], key_origin),
            Knowledge::proven(RowSemantics::Bag, row_origin),
        );
        prop_assert_eq!(&ordered_facts, &reordered_facts);
        prop_assert_ne!(&ordered_facts, &distinct_facts);

        let ordered = RelationalQuery::new(direct_read_subset_project(
            base.root().clone(),
            &selected,
            ordered_facts.clone(),
            origin(FILE, 20, 29, Vec::new()),
        ));
        let reordered = RelationalQuery::new(direct_read_subset_project(
            base.root().clone(),
            &selected,
            reordered_facts.clone(),
            origin(FILE, 20, 29, Vec::new()),
        ));
        let distinct = RelationalQuery::new(direct_read_subset_project(
            base.root().clone(),
            &selected,
            distinct_facts.clone(),
            origin(FILE, 20, 29, Vec::new()),
        ));

        let ordered = normalized(&ordered);
        let reordered = normalized(&reordered);
        let distinct = normalized(&distinct);
        let ordered_is_project = matches!(ordered.root().operator(), RelationOperator::Project { .. });
        prop_assert!(ordered_is_project);
        prop_assert_eq!(ordered.root().schema().columns().len(), selected.len());
        prop_assert_eq!(ordered.root().facts(), &ordered_facts);
        prop_assert_eq!(ordered.equivalence_key(), reordered.equivalence_key());
        prop_assert_eq!(ordered.structural_key(), reordered.structural_key());
        prop_assert_ne!(ordered.equivalence_key(), distinct.equivalence_key());
        prop_assert_ne!(ordered.structural_key(), distinct.structural_key());
        prop_assert_eq!(distinct.root().facts(), &distinct_facts);
    }
}

/// Regression for a `NormalizationOutcome::normalized -> None` mutant: the
/// accessor must actually expose the `Normalized` payload it was built from,
/// not silently discard it, and must stay `None` for a genuine failure.
#[test]
fn normalization_outcome_accessors_expose_the_correct_variant() {
    let base = query(&[3], &["value"], origin(FILE, 1, 4, Vec::new()));

    let succeeded = normalize_relational_query(&base);
    assert!(succeeded.normalized().is_some());
    assert!(succeeded.failure().is_none());

    let failed = normalize_relational_query_with_budget(&base, NormalizationBudget::new(0));
    assert!(failed.normalized().is_none());
    assert!(failed.failure().is_some());
}

/// Regression for `EquivalenceKey::as_str`/`StructuralKey::as_str` `-> ""`
/// and `-> "xyzzy"` mutants: both accessors must expose the real encoded
/// content, not a constant placeholder.
#[test]
fn equivalence_and_structural_key_as_str_expose_the_real_encoded_content() {
    let base = normalized(&query(
        &[3, 8],
        &["left", "right"],
        origin(FILE, 1, 9, Vec::new()),
    ));
    let other = normalized(&query(&[3], &["left"], origin(FILE, 1, 9, Vec::new())));

    for placeholder in ["", "xyzzy"] {
        assert_ne!(base.equivalence_key().as_str(), placeholder);
        assert_ne!(base.structural_key().as_str(), placeholder);
    }
    assert_ne!(
        base.equivalence_key().as_str(),
        other.equivalence_key().as_str()
    );
    assert_ne!(
        base.structural_key().as_str(),
        other.structural_key().as_str()
    );
}

/// Regression for a `KeyEncoder::column_id -> ()` mutant and for
/// `ColumnScope::position -> None`/`Some(0)`/`Some(1)` mutants: a sort key
/// referencing the first vs. the second output column — identical in every
/// other respect — must resolve to a different scope position and therefore
/// a different equivalence key. A stubbed-out column-id (or a constant
/// position) would make both sorts encode identically despite ordering by
/// different columns.
#[test]
fn sort_key_column_reference_is_position_sensitive() {
    let source = origin(FILE, 1, 20, Vec::new());
    let base = query(&[3, 8], &["first", "second"], source.clone());
    let sort_by = |sort_column: ColumnId| {
        RelationalQuery::new(
            RelationExpression::new(
                RelationOperator::Sort {
                    input: Box::new(base.root().clone()),
                    keys: vec![SortKey::new(
                        sort_column,
                        SortDirection::Ascending,
                        source.clone(),
                    )],
                },
                base.output().clone(),
                RelationFacts::unknown(),
                source.clone(),
            )
            .expect("sort fixture is valid"),
        )
    };

    let sorted_by_first = sort_by(base.output().columns()[0].id());
    let sorted_by_second = sort_by(base.output().columns()[1].id());

    assert_ne!(
        key(&sorted_by_first),
        key(&sorted_by_second),
        "sorting by the first vs. the second output column must not collapse \
         to the same equivalence key"
    );
}

/// Regression for `KeyEncoder::source -> ()` and `KeyEncoder::class -> ()`
/// mutants: scanning a different resolved class must change the equivalence
/// key even when the declared output schema and column ids are byte-for-byte
/// identical.
#[test]
fn scans_of_different_classes_never_share_an_equivalence_key() {
    let (left_class, right_class) = classes();
    let source = origin(FILE, 1, 9, Vec::new());
    let left = RelationalQuery::new(scan(&[3], &["value"], source.clone(), left_class));
    let right = RelationalQuery::new(scan(&[3], &["value"], source, right_class));

    assert_ne!(
        key(&left),
        key(&right),
        "scanning a different resolved class must change the equivalence key"
    );
}

/// Regression for a `KeyEncoder::keys -> ()` mutant: a proven candidate key
/// must change the equivalence key even when row semantics and everything
/// else about the scan is identical.
#[test]
fn candidate_key_facts_alone_change_the_equivalence_key() {
    let source = origin(FILE, 1, 9, Vec::new());
    let (class, _) = classes();
    let schema = RelationSchema::new(vec![column(3, "value", source.clone())])
        .expect("fixture schema is valid");
    let key_origin = origin(FILE, 10, 14, Vec::new());
    let scan_with = |facts: RelationFacts| {
        RelationalQuery::new(
            RelationExpression::new(
                RelationOperator::Scan(RelationSource::Class(class.clone())),
                schema.clone(),
                facts,
                source.clone(),
            )
            .expect("fixture scan is valid"),
        )
    };

    let with_key = scan_with(RelationFacts::new(
        Knowledge::proven(vec![CandidateKey::new(vec![ColumnId::new(3)])], key_origin),
        Knowledge::unknown(),
    ));
    let without_key = scan_with(RelationFacts::unknown());

    assert_ne!(
        key(&with_key),
        key(&without_key),
        "a proven candidate key must change the equivalence key on its own"
    );
}

/// Regression for a `KeyEncoder::row_semantics -> ()` mutant: row semantics
/// (Set vs. Bag) must change the equivalence key even when candidate-key
/// facts and everything else about the scan is identical.
#[test]
fn row_semantics_facts_alone_change_the_equivalence_key() {
    let source = origin(FILE, 1, 9, Vec::new());
    let (class, _) = classes();
    let schema = RelationSchema::new(vec![column(3, "value", source.clone())])
        .expect("fixture schema is valid");
    let semantics_origin = origin(FILE, 10, 14, Vec::new());
    let scan_with = |facts: RelationFacts| {
        RelationalQuery::new(
            RelationExpression::new(
                RelationOperator::Scan(RelationSource::Class(class.clone())),
                schema.clone(),
                facts,
                source.clone(),
            )
            .expect("fixture scan is valid"),
        )
    };

    let set_facts = scan_with(RelationFacts::new(
        Knowledge::unknown(),
        Knowledge::proven(RowSemantics::Set, semantics_origin.clone()),
    ));
    let bag_facts = scan_with(RelationFacts::new(
        Knowledge::unknown(),
        Knowledge::proven(RowSemantics::Bag, semantics_origin),
    ));

    assert_ne!(
        key(&set_facts),
        key(&bag_facts),
        "row semantics (Set vs. Bag) must change the equivalence key on its own"
    );
}

/// Regression for `KeyEncoder::scalar -> ()` and `KeyEncoder::literal -> ()`
/// mutants: the literal value projected into an output column must change
/// the equivalence key even when the schema, types, and everything else
/// about the projection is identical.
#[test]
fn projected_literal_value_changes_the_equivalence_key() {
    let literal_projection = |value: &str, source: IrOrigin| {
        let (class, _) = classes();
        let input = scan(&[1], &["seed"], source.clone(), class);
        let output = column(90, "computed", source.clone());
        let literal_scalar = ScalarExpression::new(
            ScalarOperator::Literal(ScalarLiteral::String(value.to_string())),
            type_ref("String"),
            one(),
            Nullability::NonNullable,
            Knowledge::unknown(),
            source.clone(),
        );
        RelationalQuery::new(
            RelationExpression::new(
                RelationOperator::Project {
                    input: Box::new(input),
                    projections: vec![Projection::new(output.id(), literal_scalar)],
                    kind: ProjectionKind::Relation,
                },
                RelationSchema::new(vec![output]).expect("fixture schema is valid"),
                RelationFacts::unknown(),
                source,
            )
            .expect("literal projection fixture is valid"),
        )
    };

    let first = literal_projection("alpha", origin(FILE, 1, 9, Vec::new()));
    let second = literal_projection("beta", origin(FILE, 1, 9, Vec::new()));

    assert_ne!(
        key(&first),
        key(&second),
        "the literal value projected into an output column must change the \
         equivalence key even when the schema and everything else about the \
         projection is identical"
    );
}

/// Regression for a `KeyEncoder::totality -> ()` mutant: proven totality
/// evidence (Total vs. Partial) must change the equivalence key even when
/// the projection is otherwise identity-shaped.
#[test]
fn projected_totality_evidence_changes_the_equivalence_key() {
    let source = origin(FILE, 1, 20, Vec::new());
    let base = query(&[3, 8], &["left", "right"], source.clone());
    let evidence_origin = origin(FILE, 21, 24, Vec::new());
    let total = Knowledge::proven(Totality::Total, evidence_origin.clone());
    let partial = Knowledge::proven(Totality::Partial, evidence_origin);

    let total_query = RelationalQuery::new(identity_project_with_metadata(
        base.root().clone(),
        source.clone(),
        RelationFacts::unknown(),
        total,
    ));
    let partial_query = RelationalQuery::new(identity_project_with_metadata(
        base.root().clone(),
        source,
        RelationFacts::unknown(),
        partial,
    ));

    assert_ne!(
        key(&total_query),
        key(&partial_query),
        "proven totality evidence (Total vs. Partial) must change the \
         equivalence key even when the projection is otherwise identity-shaped"
    );
}

/// Regression for `structural_identity_key -> String::new()`/`"xyzzy".into()`
/// (relational.rs) and `model_origin_key -> String::new()`/`"xyzzy".into()`
/// (this module): PMCD definitions from the same document share a
/// document-level [`DefinitionAnchor`] with no element span (see
/// `DefinitionAnchor`'s rustdoc), so `structural_identity_key` is the *only*
/// thing that can tell two same-document classes' model origins apart. A
/// single differing model origin — same source span, same document, only the
/// resolved class differs — must still change the structural key, even
/// though it never changes the (provenance-independent) equivalence key.
#[test]
fn distinct_singleton_model_origins_change_the_structural_key() {
    let (left, right) = classes();
    assert_eq!(
        left.definition(),
        right.definition(),
        "fixture classes must share one document-level anchor for this \
         regression to be meaningful"
    );

    // The *scanned* class is held fixed (always `left`) on both sides, so
    // the equivalence key's own class encoding cannot be what distinguishes
    // them. Only the extraneous `model_origins` bookkeeping on the query's
    // `IrOrigin` — a fact an `IrOrigin` can carry from a definition that
    // merely contributed to the node without being what it scans — differs.
    let scanned = left.clone();
    let left_origin = origin(FILE, 1, 4, vec![ModelOrigin::from_class(&left)]);
    let right_origin = origin(FILE, 1, 4, vec![ModelOrigin::from_class(&right)]);
    let with_left = RelationalQuery::new(scan(&[4], &["value"], left_origin, scanned.clone()));
    let with_right = RelationalQuery::new(scan(&[4], &["value"], right_origin, scanned));

    let with_left = normalized(&with_left);
    let with_right = normalized(&with_right);
    assert_eq!(
        with_left.equivalence_key(),
        with_right.equivalence_key(),
        "model provenance must never affect the allocation-independent \
         semantic identity"
    );
    assert_ne!(
        with_left.structural_key(),
        with_right.structural_key(),
        "two singleton model origins that differ only in which same-document \
         class they name must produce different structural keys"
    );
}
