//! End-to-end contracts for fail-closed structural relational comparison.

use pure_analyzer_analysis::{
    AnalysisInput, Column, ColumnId, ComparisonOutcome, IrOrigin, Knowledge, ModelOrigin,
    NormalizationBudget, NormalizationOutcome, Nullability, OpaqueOutcome, RelationExpression,
    RelationFacts, RelationOperator, RelationSchema, RelationSource, RelationalOutcome,
    RelationalQuery, ScalarExpression, ScalarLiteral, ScalarOperator, SourceSpan,
    StructuralDifferenceKind, Totality, compare_lowered_queries, compare_relational_queries,
    compare_relational_queries_with_budget, lower_m3_query, normalize_relational_query,
};
use pure_analyzer_diagnostics::{FileId, ReasonCode, TextRange, TextSize};
use pure_analyzer_model::{
    ModelGraph, Multiplicity, PmcdDocument, QName, TypeRef, load_pmcd_documents,
};
use pure_analyzer_parser::parse_query;
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
fn unknown_nullability_never_refutes_equivalence() {
    let class = class();
    let source = origin(FIRST_FILE, 1, 10);
    let left = query_with_schema(
        RelationSchema::new(vec![Column::new(
            ColumnId::new(7),
            "name".parse().expect("fixture column name is valid"),
            type_ref("String"),
            one(),
            Nullability::Unknown,
            source.clone(),
        )])
        .expect("fixture schema is valid"),
        source.clone(),
        class.clone(),
    );
    let known_sides = [Nullability::NonNullable, Nullability::Nullable];

    for known in known_sides {
        let right = query_with_schema(
            RelationSchema::new(vec![Column::new(
                ColumnId::new(90),
                "name".parse().expect("fixture column name is valid"),
                type_ref("String"),
                one(),
                known,
                origin(FIRST_FILE + 1, 101, 120),
            )])
            .expect("fixture schema is valid"),
            origin(FIRST_FILE + 1, 101, 120),
            class.clone(),
        );

        let forward = compare_relational_queries(&left, &right);
        let reverse = compare_relational_queries(&right, &left);
        assert_eq!(
            forward, reverse,
            "indecision selection must be argument-order independent for {known:?}"
        );
        assert!(
            forward.difference().is_none(),
            "an unknown nullability fact must never be treated as a proven \
             contradiction for {known:?}, got {forward:?}"
        );
        let ComparisonOutcome::Indecisive(indecision) = forward else {
            panic!("unknown nullability vs {known:?} must stay indecisive, got {forward:?}")
        };
        assert_eq!(indecision.reason(), ReasonCode::IndMissingRewrite);
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

const SOURCE_LOWERING_TEST_FILE: u32 = 91;

fn person_model_with_name() -> ModelGraph {
    let source = r#"{
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
                    }
                ],
                "qualifiedProperties": []
            }
        ]
    }"#;
    load_pmcd_documents(&[PmcdDocument::new("comparison-source-fixture", source)])
        .expect("fixture model must load")
}

fn lower_source(source: &str, model: &ModelGraph, file: u32) -> RelationalQuery {
    let parsed =
        parse_query(source, FileId::new(file)).expect("regression fixture source must parse");
    let outcome = lower_m3_query(AnalysisInput::new(
        FileId::new(file),
        source,
        &parsed.green,
        &parsed.diagnostics,
        Some(model),
    ));
    let RelationalOutcome::Supported(query) = outcome else {
        panic!("regression fixture must lower through the supported core: {outcome:#?}");
    };
    *query
}

/// Regression fixture for
/// https://github.com/tsouza/pure-analyzer/issues/263: `->map(f)` and
/// `->project(~[value: f])` lower to structurally identical schemas (both
/// produce one column literally named `value`, the internal name lowering
/// assigns a `->map` result), but the former is Legend's `String[*]` and the
/// latter is a `Relation<(value:String)>` — genuinely different result
/// types. The comparator must never call these `Equivalent`.
#[test]
fn map_and_a_project_named_value_are_never_equivalent() {
    let model = person_model_with_name();
    let map_query = lower_source(
        "model::Person.all()->map(p| $p.name)",
        &model,
        SOURCE_LOWERING_TEST_FILE,
    );
    let project_query = lower_source(
        "model::Person.all()->project(~[value: p | $p.name])",
        &model,
        SOURCE_LOWERING_TEST_FILE + 1,
    );

    let forward = compare_relational_queries(&map_query, &project_query);
    let reverse = compare_relational_queries(&project_query, &map_query);
    assert_eq!(
        forward, reverse,
        "the map/project construct refutation must not depend on argument order"
    );
    assert_ne!(
        forward,
        ComparisonOutcome::Equivalent,
        "a scalar collection and a relation must never compare as equivalent, got {forward:#?}"
    );
}

/// Companion regression fixture: `.property` navigation is exactly as
/// scalar-shaped as `->map`, and must not compare equivalent to an explicit
/// `->project(~[...])` that merely happens to share its output column name.
#[test]
fn property_navigation_and_a_project_sharing_its_column_name_are_never_equivalent() {
    let model = person_model_with_name();
    let navigation_query = lower_source(
        "model::Person.all().name",
        &model,
        SOURCE_LOWERING_TEST_FILE + 2,
    );
    let project_query = lower_source(
        "model::Person.all()->project(~[name: p | $p.name])",
        &model,
        SOURCE_LOWERING_TEST_FILE + 3,
    );

    let forward = compare_relational_queries(&navigation_query, &project_query);
    let reverse = compare_relational_queries(&project_query, &navigation_query);
    assert_eq!(
        forward, reverse,
        "the navigation/project construct refutation must not depend on argument order"
    );
    assert_ne!(
        forward,
        ComparisonOutcome::Equivalent,
        "a scalar collection and a relation must never compare as equivalent, got {forward:#?}"
    );
}

/// Production-path regression for
/// https://github.com/tsouza/pure-analyzer/issues/281: two real, stacked
/// `->distinct()` calls each lower their own `RelationFacts` from their own
/// call-site span (`lowering.rs`'s `lower_bare_distinct`), so before the fix
/// `is_repeated_distinct`'s `facts == input.facts()` guard — which also
/// compared the `IrOrigin` each fact was proved from — could never match
/// real lowered IR and `->distinct()->distinct()` never collapsed.
#[test]
fn repeated_distinct_collapses_from_real_lowered_source() {
    let model = person_model_with_name();
    let repeated = lower_source(
        "model::Person.all()->distinct()->distinct()",
        &model,
        SOURCE_LOWERING_TEST_FILE + 4,
    );
    let single = lower_source(
        "model::Person.all()->distinct()",
        &model,
        SOURCE_LOWERING_TEST_FILE + 5,
    );

    let normalized = match normalize_relational_query(&repeated) {
        NormalizationOutcome::Normalized(normalized) => *normalized,
        NormalizationOutcome::Indecisive(failure) => {
            panic!("repeated ->distinct() must normalize, got {failure:?}")
        }
    };
    assert!(
        matches!(
            normalized.root().operator(),
            RelationOperator::Distinct { input }
                if matches!(input.operator(), RelationOperator::Scan(_))
        ),
        "->distinct()->distinct() must collapse to a single Distinct(Scan), got {:#?}",
        normalized.root().operator()
    );

    let forward = compare_relational_queries(&repeated, &single);
    let reverse = compare_relational_queries(&single, &repeated);
    assert_eq!(
        forward, reverse,
        "the repeated-distinct collapse must not depend on argument order"
    );
    assert_eq!(forward, ComparisonOutcome::Equivalent);
}

/// Companion pin for issue #281/#410: `is_identity_project` stays frozen.
/// This projection is shape-identical to its input at the JSON/text level
/// (same single column, same name, same read), but real lowering always
/// mints a fresh `ColumnId` for a projected column — even a bare
/// pass-through read — so it never shares the input column's own id, and
/// must not collapse. If this assertion starts failing because normalization
/// silently collapses it, that change needs issue #410's full lowering-level
/// `ColumnId`-reuse design, not an accidental relaxation of the guard.
#[test]
fn identity_shaped_project_stays_frozen() {
    let model = person_model_with_name();
    let identity_query = lower_source(
        "model::Person.all()->project(~[name: p | $p.name])",
        &model,
        SOURCE_LOWERING_TEST_FILE + 6,
    );

    let normalized = match normalize_relational_query(&identity_query) {
        NormalizationOutcome::Normalized(normalized) => *normalized,
        NormalizationOutcome::Indecisive(failure) => {
            panic!("identity-shaped project must normalize, got {failure:?}")
        }
    };
    assert!(
        matches!(
            normalized.root().operator(),
            RelationOperator::Project { .. }
        ),
        "an identity-shaped project must stay frozen, not collapse to its input, got {:#?}",
        normalized.root().operator()
    );
}

fn structural_key_of(query: &RelationalQuery) -> pure_analyzer_analysis::StructuralKey {
    match normalize_relational_query(query) {
        NormalizationOutcome::Normalized(normalized) => normalized.structural_key().clone(),
        NormalizationOutcome::Indecisive(failure) => {
            panic!("fixture must normalize: {failure:?}")
        }
    }
}

/// Regression for a `canonical_normalized_origins` `<=` -> `>` mutant. The
/// existing `ordered_schema_mismatch_is_a_symmetric_span_anchored_refutation`
/// test only checks `compare(a, b) == compare(b, a)`, which a flip from `<=`
/// to `>` still satisfies (both branches stay self-consistent under
/// argument-swap, just consistently picking the *maximum* structural key
/// instead of the minimum). This pins the actual selection against an
/// independent oracle: `StructuralKey`'s own, unrelated `Ord` impl.
#[test]
fn output_column_mismatch_selects_the_minimum_structural_key_as_primary() {
    let class = class();
    let left = query(
        &[(7, "name", "String")],
        origin(FIRST_FILE, 1, 10),
        class.clone(),
    );
    let right = query(
        &[(90, "name", "Integer")],
        origin(FIRST_FILE + 1, 101, 120),
        class,
    );

    let left_structural_key = structural_key_of(&left);
    let right_structural_key = structural_key_of(&right);
    assert_ne!(
        left_structural_key, right_structural_key,
        "fixture must have a strict structural-key order for this regression \
         to be meaningful"
    );
    let expected_primary_file = if left_structural_key < right_structural_key {
        FIRST_FILE
    } else {
        FIRST_FILE + 1
    };

    let ComparisonOutcome::NotEquivalent(difference) = compare_relational_queries(&left, &right)
    else {
        panic!("a Type field mismatch must be refuted")
    };
    assert_eq!(
        difference.primary_origin().source().file(),
        FileId::new(expected_primary_file),
        "the primary origin must belong to the query with the strictly \
         smaller structural key, not an argument-order-dependent selection"
    );
}

/// Companion regression for the single-origin `canonical_normalized_origin`
/// `<=` -> `>` mutant, exercised through the unproven-normal-form-difference
/// indecision path. Same rationale as the test above: symmetry alone does
/// not distinguish "always pick the min" from "always pick the max".
#[test]
fn unproven_difference_indecision_selects_the_minimum_structural_key_origin() {
    let class = class();
    let base = query(&[(7, "name", "String")], origin(FIRST_FILE, 1, 10), class);
    let literal_false = filtered(&base, false, origin(FIRST_FILE + 1, 101, 120));

    let base_structural_key = structural_key_of(&base);
    let literal_false_structural_key = structural_key_of(&literal_false);
    assert_ne!(
        base_structural_key, literal_false_structural_key,
        "fixture must have a strict structural-key order for this regression \
         to be meaningful"
    );
    let expected_file = if base_structural_key < literal_false_structural_key {
        FIRST_FILE
    } else {
        FIRST_FILE + 1
    };

    let ComparisonOutcome::Indecisive(indecision) =
        compare_relational_queries(&base, &literal_false)
    else {
        panic!("an unproven normal-form difference must stay indecisive")
    };
    assert_eq!(
        indecision.origin().source().file(),
        FileId::new(expected_file),
        "the selected origin must belong to the query with the strictly \
         smaller structural key"
    );
}

fn shared_document_classes() -> (ResolvedClass, ResolvedClass) {
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
    let graph = load_pmcd_documents(&[PmcdDocument::new("comparison-tie-break-fixture", source)])
        .expect("fixture model loads");
    let resolver = Resolver::new(&graph);
    let resolve = |name: &str| match resolver
        .resolve_class(&QName::new(name).expect("fixture path is valid"))
    {
        Resolution::Found(class) => class,
        outcome => panic!("fixture class resolves: {outcome:?}"),
    };
    (resolve("model::Left"), resolve("model::Right"))
}

/// Regression for a `model_origin_keys -> vec![]` mutant. PMCD definitions
/// from the same document share one document-level [`DefinitionAnchor`] with
/// no element span, so two failures that also share the same query source
/// span can *only* be told apart by their model-origin identity. Without
/// `model_origin_keys` actually comparing that identity, the tie-break
/// degenerates to "always equal", which breaks the documented
/// argument-order-independence contract: swapping inputs then always selects
/// whichever failure happens to land in the *first* comparator position,
/// rather than the query's own content.
#[test]
fn two_sided_failure_with_a_tied_span_breaks_ties_on_model_origin_identity() {
    let (left_model, right_model) = shared_document_classes();
    assert_eq!(
        left_model.definition(),
        right_model.definition(),
        "fixture classes must share one document-level anchor for this \
         tie-break to be meaningful"
    );

    let tied_span = FileId::new(FIRST_FILE);
    let tied_range = TextRange::new(TextSize::from(1), TextSize::from(10));
    let origin_with =
        |models: Vec<ModelOrigin>| IrOrigin::new(SourceSpan::new(tied_span, tied_range), models);
    let class = class();
    let left = query(
        &[(7, "name", "String")],
        origin_with(vec![ModelOrigin::from_class(&left_model)]),
        class.clone(),
    );
    let right = query(
        &[(90, "name", "String")],
        origin_with(vec![ModelOrigin::from_class(&right_model)]),
        class,
    );

    let budget = NormalizationBudget::new(0);
    let forward = compare_relational_queries_with_budget(&left, &right, budget);
    let reverse = compare_relational_queries_with_budget(&right, &left, budget);
    assert_eq!(
        forward, reverse,
        "failure selection must be argument-order independent even when the \
         two failures tie on reason and source span, and can only be broken \
         by comparing model-origin identity"
    );
}
