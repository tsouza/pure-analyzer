//! End-to-end contracts for fail-closed structural relational comparison.

use pure_analyzer_analysis::{
    Column, ColumnId, ComparisonOutcome, IrOrigin, Knowledge, NormalizationBudget, Nullability,
    OpaqueOutcome, RelationExpression, RelationFacts, RelationOperator, RelationSchema,
    RelationSource, RelationalOutcome, RelationalQuery, ScalarExpression, ScalarLiteral,
    ScalarOperator, SourceSpan, StructuralDifferenceKind, Totality, compare_lowered_queries,
    compare_relational_queries, compare_relational_queries_with_budget,
};
use pure_analyzer_diagnostics::{FileId, ReasonCode, TextRange, TextSize};
use pure_analyzer_model::{Multiplicity, PmcdDocument, QName, TypeRef, load_pmcd_documents};
use pure_analyzer_resolve::{Resolution, ResolvedClass, Resolver};

const FIRST_FILE: u32 = 71;
const EXACTLY_ONE: u32 = 1;

fn origin(file: u32, start: u32, end: u32) -> IrOrigin {
    IrOrigin::new(
        SourceSpan::new(
            FileId::new(file),
            TextRange::new(TextSize::from(start), TextSize::from(end)),
        ),
        Vec::new(),
    )
}

fn one() -> Multiplicity {
    Multiplicity::new(EXACTLY_ONE, Some(EXACTLY_ONE)).expect("fixture multiplicity is valid")
}

fn type_ref(name: &str) -> TypeRef {
    TypeRef::new(QName::new(name).expect("fixture type is valid"), Vec::new())
}

fn class() -> ResolvedClass {
    let source = r#"{
        "_type": "data",
        "elements": [
            {
                "_type": "class",
                "package": "model",
                "name": "Person",
                "stereotypes": [],
                "superTypes": [],
                "properties": [],
                "qualifiedProperties": []
            }
        ]
    }"#;
    let graph = load_pmcd_documents(&[PmcdDocument::new("comparison-fixture", source)])
        .expect("fixture model loads");
    let resolver = Resolver::new(&graph);
    match resolver.resolve_class(&QName::new("model::Person").expect("fixture path is valid")) {
        Resolution::Found(class) => class,
        outcome => panic!("fixture class resolves: {outcome:?}"),
    }
}

fn column(id: u32, name: &str, type_name: &str, source: IrOrigin) -> Column {
    Column::new(
        ColumnId::new(id),
        name.parse().expect("fixture column name is valid"),
        type_ref(type_name),
        one(),
        Nullability::NonNullable,
        source,
    )
}

fn query(columns: &[(u32, &str, &str)], source: IrOrigin, class: ResolvedClass) -> RelationalQuery {
    query_with_schema(
        RelationSchema::new(
            columns
                .iter()
                .map(|(id, name, type_name)| column(*id, name, type_name, source.clone()))
                .collect(),
        )
        .expect("fixture schema is valid"),
        source,
        class,
    )
}

fn query_with_schema(
    schema: RelationSchema,
    source: IrOrigin,
    class: ResolvedClass,
) -> RelationalQuery {
    let root = RelationExpression::new(
        RelationOperator::Scan(RelationSource::Class(class)),
        schema,
        RelationFacts::unknown(),
        source,
    )
    .expect("fixture scan is valid");
    RelationalQuery::new(root)
}

fn filtered(query: &RelationalQuery, value: bool, source: IrOrigin) -> RelationalQuery {
    let predicate = ScalarExpression::new(
        ScalarOperator::Literal(ScalarLiteral::Boolean(value)),
        type_ref("Boolean"),
        one(),
        Nullability::NonNullable,
        Knowledge::<Totality>::unknown(),
        source.clone(),
    );
    let root = RelationExpression::new(
        RelationOperator::Filter {
            input: Box::new(query.root().clone()),
            predicate,
        },
        query.output().clone(),
        RelationFacts::unknown(),
        source,
    )
    .expect("fixture filter is valid");
    RelationalQuery::new(root)
}

#[test]
fn proven_keys_make_reflexivity_and_literal_true_rewrites_equivalent() {
    let class = class();
    let base = query(&[(7, "name", "String")], origin(FIRST_FILE, 1, 10), class);
    let literal_true = filtered(&base, true, origin(FIRST_FILE, 12, 30));

    let reflexive = compare_relational_queries(&base, &base);
    assert_eq!(reflexive, ComparisonOutcome::Equivalent);
    assert!(reflexive.difference().is_none());
    assert!(reflexive.indecision().is_none());
    assert_eq!(
        compare_relational_queries(&base, &literal_true),
        ComparisonOutcome::Equivalent
    );
    assert_eq!(
        compare_relational_queries(&literal_true, &base),
        ComparisonOutcome::Equivalent
    );
}

#[test]
fn equivalent_normal_forms_ignore_allocation_and_query_spans() {
    let class = class();
    let first = query(
        &[(7, "name", "String"), (91, "email", "String")],
        origin(FIRST_FILE, 1, 10),
        class.clone(),
    );
    let second = query(
        &[(902, "name", "String"), (3, "email", "String")],
        origin(FIRST_FILE + 1, 101, 199),
        class,
    );

    assert_eq!(
        compare_relational_queries(&first, &second),
        ComparisonOutcome::Equivalent
    );
    assert_eq!(
        compare_relational_queries(&second, &first),
        ComparisonOutcome::Equivalent
    );
}

#[test]
fn ordered_schema_mismatch_is_a_symmetric_span_anchored_refutation() {
    let class = class();
    let left = query(
        &[(7, "name", "String"), (8, "email", "String")],
        origin(FIRST_FILE, 1, 10),
        class.clone(),
    );
    let right = query(
        &[(90, "email", "String"), (91, "name", "String")],
        origin(FIRST_FILE + 1, 101, 120),
        class,
    );

    let forward = compare_relational_queries(&left, &right);
    let reverse = compare_relational_queries(&right, &left);
    assert_eq!(
        forward, reverse,
        "comparison proof must not depend on argument order"
    );
    assert!(forward.difference().is_some());
    assert!(forward.indecision().is_none());

    let ComparisonOutcome::NotEquivalent(difference) = forward else {
        panic!("ordered output schemas must be refuted")
    };
    assert!(matches!(
        difference.kind(),
        StructuralDifferenceKind::OutputColumn {
            index: 0,
            field: pure_analyzer_analysis::OutputSchemaField::Name,
        }
    ));
    let origins = [
        difference.primary_origin().source(),
        difference.secondary_origin().source(),
    ];
    assert!(
        origins
            .iter()
            .any(|span| span.file() == FileId::new(FIRST_FILE))
    );
    assert!(
        origins
            .iter()
            .any(|span| span.file() == FileId::new(FIRST_FILE + 1))
    );
}

#[test]
fn output_column_count_is_a_symmetric_refutation() {
    let class = class();
    let one_column = query(
        &[(7, "name", "String")],
        origin(FIRST_FILE, 1, 10),
        class.clone(),
    );
    let two_columns = query(
        &[(90, "name", "String"), (91, "email", "String")],
        origin(FIRST_FILE + 1, 101, 120),
        class,
    );

    let forward = compare_relational_queries(&one_column, &two_columns);
    let reverse = compare_relational_queries(&two_columns, &one_column);
    assert_eq!(
        forward, reverse,
        "column-count proof must not depend on argument order"
    );

    let ComparisonOutcome::NotEquivalent(difference) = forward else {
        panic!("different output widths must be refuted")
    };
    let StructuralDifferenceKind::OutputColumnCount {
        primary_count,
        secondary_count,
    } = difference.kind()
    else {
        panic!("expected an output-column-count refutation")
    };
    let mut counts = [*primary_count, *secondary_count];
    counts.sort_unstable();
    assert_eq!(counts, [1, 2]);
}

#[test]
fn every_declared_output_column_metadata_mismatch_is_refuted() {
    let class = class();
    let source = origin(FIRST_FILE, 1, 10);
    let left = query_with_schema(
        RelationSchema::new(vec![column(7, "name", "String", source.clone())])
            .expect("fixture schema is valid"),
        source.clone(),
        class.clone(),
    );
    let nullable = Column::new(
        ColumnId::new(90),
        "name".parse().expect("fixture column name is valid"),
        type_ref("String"),
        one(),
        Nullability::Nullable,
        origin(FIRST_FILE + 1, 101, 120),
    );
    let optional = Column::new(
        ColumnId::new(91),
        "name".parse().expect("fixture column name is valid"),
        type_ref("String"),
        Multiplicity::new(0, Some(EXACTLY_ONE)).expect("fixture multiplicity is valid"),
        Nullability::NonNullable,
        origin(FIRST_FILE + 2, 201, 220),
    );
    let mismatches = [
        (
            query(
                &[(92, "name", "Integer")],
                origin(FIRST_FILE + 3, 301, 320),
                class.clone(),
            ),
            pure_analyzer_analysis::OutputSchemaField::Type,
        ),
        (
            query_with_schema(
                RelationSchema::new(vec![optional]).expect("fixture schema is valid"),
                origin(FIRST_FILE + 2, 201, 220),
                class.clone(),
            ),
            pure_analyzer_analysis::OutputSchemaField::Multiplicity,
        ),
        (
            query_with_schema(
                RelationSchema::new(vec![nullable]).expect("fixture schema is valid"),
                origin(FIRST_FILE + 1, 101, 120),
                class,
            ),
            pure_analyzer_analysis::OutputSchemaField::Nullability,
        ),
    ];

    for (right, expected_field) in mismatches {
        let ComparisonOutcome::NotEquivalent(difference) =
            compare_relational_queries(&left, &right)
        else {
            panic!("output metadata mismatch must be refuted")
        };
        assert!(matches!(
            difference.kind(),
            StructuralDifferenceKind::OutputColumn {
                index: 0,
                field,
            } if *field == expected_field
        ));
    }
}

#[test]
fn unproven_normal_form_differences_stay_indecisive() {
    let class = class();
    let base = query(&[(7, "name", "String")], origin(FIRST_FILE, 1, 10), class);
    let literal_false = filtered(&base, false, origin(FIRST_FILE + 1, 101, 120));

    let forward = compare_relational_queries(&base, &literal_false);
    let reverse = compare_relational_queries(&literal_false, &base);
    assert_eq!(
        forward, reverse,
        "indecision selection must be argument-order independent"
    );
    assert!(forward.difference().is_none());
    assert!(forward.indecision().is_some());
    let ComparisonOutcome::Indecisive(indecision) = forward else {
        panic!("an unproved normal-form difference must not be committed")
    };
    assert_eq!(indecision.reason(), ReasonCode::IndMissingRewrite);
}

#[test]
fn two_sided_normalization_failure_selects_one_deterministic_reason_and_origin() {
    let class = class();
    let left = query(
        &[(7, "name", "String")],
        origin(FIRST_FILE, 1, 10),
        class.clone(),
    );
    let right = query(
        &[(90, "name", "String")],
        origin(FIRST_FILE + 1, 101, 120),
        class,
    );

    let budget = NormalizationBudget::new(0);
    let forward = compare_relational_queries_with_budget(&left, &right, budget);
    let reverse = compare_relational_queries_with_budget(&right, &left, budget);
    assert_eq!(
        forward, reverse,
        "failure selection must be argument-order independent"
    );
    let ComparisonOutcome::Indecisive(indecision) = forward else {
        panic!("zero normalization budget must be indecisive")
    };
    assert_eq!(indecision.reason(), ReasonCode::IndMissingRewrite);
    assert_eq!(indecision.origin().source(), left.root().origin().source());
}

#[test]
fn one_sided_normalization_failure_never_uses_the_other_query_as_a_proof() {
    let class = class();
    let base = query(&[(7, "name", "String")], origin(FIRST_FILE, 1, 10), class);
    let complex = filtered(&base, false, origin(FIRST_FILE + 1, 101, 120));
    let budget = NormalizationBudget::new(1);

    let forward = compare_relational_queries_with_budget(&base, &complex, budget);
    let reverse = compare_relational_queries_with_budget(&complex, &base, budget);
    assert_eq!(
        forward, reverse,
        "one failed normalization must remain decisive about nothing"
    );
    let ComparisonOutcome::Indecisive(indecision) = forward else {
        panic!("one-sided normalization failure must be indecisive")
    };
    assert_eq!(indecision.reason(), ReasonCode::IndMissingRewrite);
}

#[test]
fn opaque_lowering_outcomes_remain_indecisive_before_normalization() {
    let supported = RelationalOutcome::supported(query(
        &[(7, "name", "String")],
        origin(FIRST_FILE, 1, 10),
        class(),
    ));
    let opaque = RelationalOutcome::opaque(OpaqueOutcome::new(
        ReasonCode::IndUnparseable,
        origin(FIRST_FILE + 1, 101, 120),
    ));

    let forward = compare_lowered_queries(&supported, &opaque);
    let reverse = compare_lowered_queries(&opaque, &supported);
    assert_eq!(
        forward, reverse,
        "opaque lowering must be input-order independent"
    );

    let ComparisonOutcome::Indecisive(indecision) = forward else {
        panic!("an opaque input must not produce a committed comparison")
    };
    assert_eq!(indecision.reason(), ReasonCode::IndUnparseable);
    assert_eq!(
        indecision.origin().source().file(),
        FileId::new(FIRST_FILE + 1)
    );
}
