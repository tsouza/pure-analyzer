//! End-to-end contracts for conservative relation-column selector resolution.

use proptest::prelude::*;
use pure_analyzer_analysis::{
    Column, ColumnId, ColumnSelectorName, ColumnSelectorOpaqueReason, ColumnSelectorOutcome,
    IrOrigin, Nullability, RelationSchema, SourceSpan, extract_relation_column_selectors,
    resolve_relation_column_selectors,
};
use pure_analyzer_diagnostics::{FileId, TextRange, TextSize};
use pure_analyzer_model::{Multiplicity, Name, QName, TypeRef};
use pure_analyzer_parser::parse_query;
use pure_analyzer_syntax::{GreenElement, GreenNode, SyntaxKind};

const FILE: u32 = 83;
const EXACTLY_ONE: u32 = 1;
const ORIGIN_OFFSET: u32 = 0;
const ID_STEP: u32 = 17;

fn selector_node(source: &str) -> GreenNode {
    let parsed = parse_query(source, FileId::new(FILE)).expect("fixture source must build a tree");
    find_selector(&parsed.green).expect("fixture must contain exactly one selector form")
}

fn find_selector(node: &GreenNode) -> Option<GreenNode> {
    if matches!(
        node.kind(),
        SyntaxKind::COLUMN_SPEC | SyntaxKind::COLUMN_SPEC_ARRAY
    ) {
        return Some(node.clone());
    }
    node.children()
        .iter()
        .filter_map(GreenElement::as_node)
        .find_map(find_selector)
}

fn origin() -> IrOrigin {
    IrOrigin::new(
        SourceSpan::new(
            FileId::new(FILE),
            TextRange::empty(TextSize::from(ORIGIN_OFFSET)),
        ),
        Vec::new(),
    )
}

fn column(id: u32, name: &str) -> Column {
    Column::new(
        ColumnId::new(id),
        Name::new(name).expect("fixture name must be valid"),
        TypeRef::new(
            QName::new("String").expect("fixture type must be valid"),
            Vec::new(),
        ),
        Multiplicity::new(EXACTLY_ONE, Some(EXACTLY_ONE))
            .expect("fixture multiplicity must be valid"),
        Nullability::Unknown,
        origin(),
    )
}

fn schema(columns: &[(u32, &str)]) -> RelationSchema {
    RelationSchema::new(columns.iter().map(|(id, name)| column(*id, name)).collect())
        .expect("fixture schema must be valid")
}

fn span_text(source: &str, span: SourceSpan) -> &str {
    &source[usize::from(span.range().start())..usize::from(span.range().end())]
}

#[test]
fn extracts_and_resolves_source_order_with_exact_spans_and_quoted_names() {
    let source = "~[  alpha /* keep */, 'Total Revenue'  ]";
    let node = selector_node(source);
    let schema = schema(&[(41, "zeta"), (7, "alpha"), (99, "Total Revenue")]);

    let extracted = extract_relation_column_selectors(FileId::new(FILE), &node)
        .expect("valid selector form must extract");
    assert_eq!(span_text(source, extracted.source()), source);
    assert_eq!(extracted.selectors().len(), 2);
    assert_eq!(
        span_text(source, extracted.selectors()[0].source()),
        "alpha"
    );
    assert_eq!(
        span_text(source, extracted.selectors()[0].name_source()),
        "alpha"
    );
    assert_eq!(
        span_text(source, extracted.selectors()[1].source()),
        "'Total Revenue'"
    );
    assert_eq!(
        span_text(source, extracted.selectors()[1].name_source()),
        "'Total Revenue'"
    );
    assert!(matches!(
        extracted.selectors()[0].name(),
        ColumnSelectorName::Bare(name) if name.as_str() == "alpha"
    ));
    assert!(matches!(
        extracted.selectors()[1].name(),
        ColumnSelectorName::Quoted(name) if name.as_str() == "Total Revenue"
    ));

    let outcome = resolve_relation_column_selectors(FileId::new(FILE), &node, &schema);
    let ColumnSelectorOutcome::Resolved(resolved) = outcome else {
        panic!("valid selector form must resolve");
    };
    assert_eq!(
        resolved
            .selectors()
            .iter()
            .map(|selector| selector.column())
            .collect::<Vec<_>>(),
        [ColumnId::new(7), ColumnId::new(99)]
    );
    assert_eq!(span_text(source, resolved.source()), source);
}

#[test]
fn resolves_column_names_case_sensitively() {
    let source = "~Alpha";
    let node = selector_node(source);
    let relation = schema(&[(11, "alpha"), (12, "Alpha")]);

    let outcome = resolve_relation_column_selectors(FileId::new(FILE), &node, &relation);
    let ColumnSelectorOutcome::Resolved(resolved) = outcome else {
        panic!("exactly cased selector must resolve");
    };
    assert_eq!(resolved.selectors()[0].column(), ColumnId::new(12));
}

#[test]
fn rejects_missing_column_names() {
    let relation = schema(&[(11, "alpha"), (12, "Alpha")]);
    let missing_source = "~missing";
    let missing_node = selector_node(missing_source);
    let missing = resolve_relation_column_selectors(FileId::new(FILE), &missing_node, &relation);
    assert!(matches!(
        missing,
        ColumnSelectorOutcome::Opaque(opaque)
            if matches!(opaque.reason(), ColumnSelectorOpaqueReason::Missing(name) if name.as_str() == "missing")
                && span_text(missing_source, opaque.source()) == "missing"
    ));
}

#[test]
fn rejects_duplicate_schema_names() {
    let duplicate_schema = schema(&[(4, "alpha"), (9, "alpha")]);
    let duplicate_schema_source = "~alpha";
    let duplicate_schema_node = selector_node(duplicate_schema_source);
    let ambiguous = resolve_relation_column_selectors(
        FileId::new(FILE),
        &duplicate_schema_node,
        &duplicate_schema,
    );
    assert!(matches!(
        ambiguous,
        ColumnSelectorOutcome::Opaque(opaque)
            if matches!(opaque.reason(), ColumnSelectorOpaqueReason::DuplicateSchemaName(name) if name.as_str() == "alpha")
                && span_text(duplicate_schema_source, opaque.source()) == "alpha"
    ));
}

#[test]
fn rejects_duplicate_semantic_selectors() {
    let relation = schema(&[(11, "alpha"), (12, "Alpha")]);
    let duplicate_selector_source = "~[alpha, 'alpha']";
    let duplicate_selector_node = selector_node(duplicate_selector_source);
    let duplicate_selector =
        resolve_relation_column_selectors(FileId::new(FILE), &duplicate_selector_node, &relation);
    assert!(matches!(
        duplicate_selector,
        ColumnSelectorOutcome::Opaque(opaque)
            if matches!(opaque.reason(), ColumnSelectorOpaqueReason::DuplicateSelector(name) if name.as_str() == "alpha")
                && span_text(duplicate_selector_source, opaque.source()) == "'alpha'"
    ));
}

#[test]
fn preserves_quoted_spelling_while_decoding_doubled_quote_escapes_for_lookup() {
    let source = "~['it''s', other]";
    let node = selector_node(source);
    let relation = schema(&[(18, "it's"), (37, "other")]);

    let extracted = extract_relation_column_selectors(FileId::new(FILE), &node)
        .expect("quoted selector form must extract");
    assert_eq!(
        span_text(source, extracted.selectors()[0].name_source()),
        "'it''s'"
    );
    assert!(matches!(
        extracted.selectors()[0].name(),
        ColumnSelectorName::Quoted(name) if name.as_str() == "it's"
    ));

    let outcome = resolve_relation_column_selectors(FileId::new(FILE), &node, &relation);
    let ColumnSelectorOutcome::Resolved(resolved) = outcome else {
        panic!("quoted selector form must resolve");
    };
    assert_eq!(
        resolved
            .selectors()
            .iter()
            .map(|selector| selector.column())
            .collect::<Vec<_>>(),
        [ColumnId::new(18), ColumnId::new(37)]
    );
}

#[test]
fn rejects_unsupported_bodies_and_recovery_without_partial_resolution() {
    let relation = schema(&[(1, "alpha"), (2, "beta")]);
    for (source, expected) in [
        (
            "~alpha:String[1]",
            ColumnSelectorOpaqueReason::UnsupportedBody,
        ),
        (
            "~[alpha, beta: String[1]]",
            ColumnSelectorOpaqueReason::UnsupportedBody,
        ),
        ("~[alpha,]", ColumnSelectorOpaqueReason::Malformed),
        ("~[]", ColumnSelectorOpaqueReason::Malformed),
        ("~", ColumnSelectorOpaqueReason::Malformed),
        ("~[alpha beta]", ColumnSelectorOpaqueReason::Malformed),
    ] {
        let node = selector_node(source);
        let outcome = resolve_relation_column_selectors(FileId::new(FILE), &node, &relation);
        assert!(
            matches!(
                &outcome,
                ColumnSelectorOutcome::Opaque(opaque) if opaque.reason() == &expected
            ),
            "{source}: {outcome:#?}"
        );
    }
}

proptest! {
    #[test]
    fn resolves_unique_nonsequential_schema_ids_in_selector_order(
        base in 1_u32..10_000,
        count in 1_usize..6,
    ) {
        let schema = RelationSchema::new(
            (0..count)
                .map(|index| {
                    let index = u32::try_from(index).expect("bounded fixture index must fit u32");
                    column(base + ID_STEP * index, &format!("column{index}"))
                })
                .collect(),
        )
        .expect("fixture schema must be valid");
        let source = format!(
            "~[{}]",
            (0..count)
                .rev()
                .map(|index| format!("column{index}"))
                .collect::<Vec<_>>()
                .join(", "),
        );
        let node = selector_node(&source);
        let first = resolve_relation_column_selectors(FileId::new(FILE), &node, &schema);
        let second = resolve_relation_column_selectors(FileId::new(FILE), &node, &schema);
        prop_assert_eq!(&first, &second);
        let ColumnSelectorOutcome::Resolved(resolved) = first else {
            prop_assert!(false, "unique selector form must resolve");
            return Ok(());
        };
        let expected = (0..count)
            .rev()
            .map(|index| {
                let index = u32::try_from(index).expect("bounded fixture index must fit u32");
                ColumnId::new(base + ID_STEP * index)
            })
            .collect::<Vec<_>>();
        prop_assert_eq!(
            resolved
                .selectors()
                .iter()
                .map(|selector| selector.column())
                .collect::<Vec<_>>(),
            expected,
        );
    }
}
