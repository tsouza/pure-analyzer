//! Contract tests for the immutable, validated green-tree foundation.

use std::{panic, thread};

use proptest::prelude::*;
use pure_analyzer_lexer::{SyntaxKind as LexerSyntaxKind, lex};
use pure_analyzer_syntax::{
    AstNode, BinaryExpression, BuildError, GreenElement, GreenNode, GreenNodeBuilder,
    RawSyntaxKind, Root, SyntaxKind, TextRange, TextSize,
};

fn flat_tree(source: &str) -> GreenNode {
    let tokens = lex(source);
    let mut builder = GreenNodeBuilder::new(source, &tokens);
    builder.open(SyntaxKind::ROOT);
    for _ in &tokens {
        builder.advance();
    }
    builder.close();
    builder.finish().expect("flat tree should be valid")
}

fn finish_with_events(
    source: &str,
    tokens: &[(LexerSyntaxKind, TextRange)],
    events: impl IntoIterator<Item = pure_analyzer_syntax::Event>,
) -> Result<GreenNode, BuildError> {
    let mut builder = GreenNodeBuilder::new(source, tokens);
    for event in events {
        builder.push(event);
    }
    builder.finish()
}

#[test]
fn round_trips_trivia_comments_and_unicode_bytes_exactly() {
    let source = "let value = 'λ' /* 🦀 */ // τέλος\n";
    let tree = flat_tree(source);

    assert_eq!(tree.text(), source);
    assert_eq!(tree.to_string(), source);
    assert_eq!(
        tree.text_range(),
        TextRange::new(
            TextSize::from(0),
            TextSize::try_from(source.len()).expect("fixture length should fit TextSize"),
        )
    );
}

#[test]
fn preserves_lexer_ranges_without_conversion() {
    let source = "let x = 1 + 2";
    let lexer_tokens = lex(source);
    let tree = flat_tree(source);

    let tree_tokens = tree.tokens().collect::<Vec<_>>();
    assert_eq!(tree_tokens.len(), lexer_tokens.len());
    for ((lexer_kind, lexer_range), tree_token) in lexer_tokens.iter().zip(tree_tokens) {
        assert_eq!(tree_token.kind(), (*lexer_kind).into());
        assert_eq!(tree_token.text_range(), *lexer_range);
    }
}

#[test]
fn token_elements_expose_their_token_and_range() {
    let source = "value";
    let tree = flat_tree(source);
    let element = &tree.children()[0];
    let token = element.as_token().expect("root child should be a token");

    assert_eq!(element.kind(), SyntaxKind::IDENT);
    assert_eq!(element.text_range(), token.text_range());
    assert!(element.as_node().is_none());
}

#[test]
fn checkpoint_retroactively_wraps_a_binary_expression() {
    let source = "1 + 2";
    let tokens = lex(source);
    let mut builder = GreenNodeBuilder::new(source, &tokens);
    builder.open(SyntaxKind::ROOT);
    let expression_start = builder.checkpoint();
    for _ in &tokens {
        builder.advance();
    }
    builder
        .open_at(&expression_start, SyntaxKind::BINARY_EXPR)
        .expect("checkpoint belongs to this builder");
    builder.close();
    builder.close();

    let tree = builder.finish().expect("events should be balanced");
    let expression = tree.children()[0]
        .as_node()
        .expect("root child should be a node");
    assert_eq!(expression.kind(), SyntaxKind::BINARY_EXPR);
    assert_eq!(expression.text(), source);
    assert!(BinaryExpression::cast(tree.clone()).is_none());
    assert!(Root::cast(expression.clone()).is_none());
    assert!(BinaryExpression::cast(expression.clone()).is_some());
    assert!(Root::cast(tree).is_some());
}

#[test]
fn typed_ast_text_range_preserves_a_nondefault_subtree_range() {
    const PREFIX: &str = "prefix ";
    const SOURCE: &str = "prefix 1 + 2";

    let tokens = lex(SOURCE);
    let prefix_end = TextSize::try_from(PREFIX.len()).expect("fixture prefix should fit TextSize");
    let expression_range = TextRange::new(
        prefix_end,
        TextSize::try_from(SOURCE.len()).expect("fixture length should fit TextSize"),
    );
    let prefix_tokens = tokens.partition_point(|(_, range)| range.end() <= prefix_end);
    let mut builder = GreenNodeBuilder::new(SOURCE, &tokens);
    builder.open(SyntaxKind::ROOT);
    for _ in &tokens[..prefix_tokens] {
        builder.advance();
    }
    let expression_start = builder.checkpoint();
    for _ in &tokens[prefix_tokens..] {
        builder.advance();
    }
    builder
        .open_at(&expression_start, SyntaxKind::BINARY_EXPR)
        .expect("checkpoint should wrap the expression suffix");
    builder.close();
    builder.close();

    let tree = builder.finish().expect("events should be balanced");
    let expression = tree
        .children()
        .iter()
        .find_map(GreenElement::as_node)
        .and_then(|node| BinaryExpression::cast(node.clone()))
        .expect("root should contain a typed binary expression");

    assert!(!expression_range.is_empty());
    assert_ne!(expression_range, TextRange::default());
    assert_eq!(expression.text_range(), expression_range);
}

#[test]
fn checkpoints_remain_stable_after_earlier_insertions() {
    let source = "1 + 2 * 3";
    let tokens = lex(source);
    let split = tokens
        .iter()
        .position(|(kind, _)| *kind == LexerSyntaxKind::STAR)
        .expect("fixture should contain a star token");
    let mut builder = GreenNodeBuilder::new(source, &tokens);
    builder.open(SyntaxKind::ROOT);
    let expression_start = builder.checkpoint();
    for _ in &tokens[..split] {
        builder.advance();
    }
    let multiplication_start = builder.checkpoint();
    for _ in &tokens[split..] {
        builder.advance();
    }

    builder
        .open_at(&expression_start, SyntaxKind::BINARY_EXPR)
        .expect("first checkpoint should remain valid");
    builder
        .open_at(&multiplication_start, SyntaxKind::BINARY_EXPR)
        .expect("later checkpoint should shift with the insertion");
    builder.close();
    builder.close();
    builder.close();

    let tree = builder.finish().expect("events should be balanced");
    let outer = tree.children()[0]
        .as_node()
        .expect("root child should be an expression");
    let inner = outer
        .children()
        .iter()
        .find_map(GreenElement::as_node)
        .expect("outer expression should contain the later expression");
    assert_eq!(outer.text(), source);
    assert_eq!(inner.text(), "* 3");
    assert_eq!(inner.text_range().start(), tokens[split].1.start());
}

#[test]
fn checkpoint_markers_do_not_change_public_event_indices() {
    let source = "x";
    let tokens = lex(source);
    let mut builder = GreenNodeBuilder::new(source, &tokens);
    builder.open(SyntaxKind::ROOT);
    let earlier = builder.checkpoint();
    builder.advance();
    let later = builder.checkpoint();
    builder
        .open_at(&earlier, SyntaxKind::ERROR_NODE)
        .expect("earlier checkpoint should be valid");

    assert_eq!(
        builder.open_at(&later, SyntaxKind::IDENT),
        Err(BuildError::ExpectedNodeKind {
            event_index: 3,
            kind: SyntaxKind::IDENT,
        })
    );
}

#[test]
fn raw_kind_ids_are_pinned_and_reject_unassigned_values() {
    let expected = [
        (SyntaxKind::DATE_TIME, 0x0000),
        (SyntaxKind::STRICT_DATE, 0x0001),
        (SyntaxKind::LATEST_DATE, 0x0002),
        (SyntaxKind::PERCENT, 0x0003),
        (SyntaxKind::TILDE, 0x0004),
        (SyntaxKind::DOLLAR, 0x0005),
        (SyntaxKind::ARROW, 0x0006),
        (SyntaxKind::PIPE, 0x0007),
        (SyntaxKind::AT, 0x0008),
        (SyntaxKind::NEW_SYMBOL, 0x0009),
        (SyntaxKind::DOT, 0x000a),
        (SyntaxKind::COMMA, 0x000b),
        (SyntaxKind::PATH_SEPARATOR, 0x000c),
        (SyntaxKind::COLON, 0x000d),
        (SyntaxKind::PAREN_OPEN, 0x000e),
        (SyntaxKind::PAREN_CLOSE, 0x000f),
        (SyntaxKind::BRACKET_OPEN, 0x0010),
        (SyntaxKind::BRACKET_CLOSE, 0x0011),
        (SyntaxKind::EQ, 0x0012),
        (SyntaxKind::NEQ, 0x0013),
        (SyntaxKind::PLUS, 0x0014),
        (SyntaxKind::MINUS, 0x0015),
        (SyntaxKind::STAR, 0x0016),
        (SyntaxKind::SLASH, 0x0017),
        (SyntaxKind::LE, 0x0018),
        (SyntaxKind::LT, 0x0019),
        (SyntaxKind::GE, 0x001a),
        (SyntaxKind::GT, 0x001b),
        (SyntaxKind::SEMICOLON, 0x001c),
        (SyntaxKind::BRACE_OPEN, 0x001d),
        (SyntaxKind::BRACE_CLOSE, 0x001e),
        (SyntaxKind::ALL_KW, 0x001f),
        (SyntaxKind::LET_KW, 0x0020),
        (SyntaxKind::ALL_VERSIONS_KW, 0x0021),
        (SyntaxKind::ALL_VERSIONS_IN_RANGE_KW, 0x0022),
        (SyntaxKind::TO_BYTES_KW, 0x0023),
        (SyntaxKind::IDENT, 0x0024),
        (SyntaxKind::INTEGER, 0x0025),
        (SyntaxKind::BOOLEAN, 0x0026),
        (SyntaxKind::STRING, 0x0027),
        (SyntaxKind::HASH_STORE_OPEN, 0x0028),
        (SyntaxKind::HASH_ISLAND_OPEN, 0x0029),
        (SyntaxKind::NAV_PATH_BLOCK, 0x002a),
        (SyntaxKind::ISLAND_END, 0x002b),
        (SyntaxKind::HASH, 0x002c),
        (SyntaxKind::WHITESPACE, 0x002d),
        (SyntaxKind::LINE_COMMENT, 0x002e),
        (SyntaxKind::BLOCK_COMMENT, 0x002f),
        (SyntaxKind::ERROR, 0x0030),
        (SyntaxKind::ASSIGN, 0x0031),
        (SyntaxKind::ROOT, 0x8000),
        (SyntaxKind::ERROR_NODE, 0x8001),
        (SyntaxKind::BINARY_EXPR, 0x8002),
        (SyntaxKind::QUERY_EXPR, 0x8003),
        (SyntaxKind::ALL_EXPR, 0x8004),
        (SyntaxKind::QUALIFIED_NAME, 0x8005),
        (SyntaxKind::VARIABLE_EXPR, 0x8006),
        (SyntaxKind::LITERAL_EXPR, 0x8007),
        (SyntaxKind::PAREN_EXPR, 0x8008),
        (SyntaxKind::UNARY_EXPR, 0x8009),
        (SyntaxKind::ARROW_CALL, 0x800a),
        (SyntaxKind::PROPERTY_NAV, 0x800b),
        (SyntaxKind::BRACKET_INDEX, 0x800c),
        (SyntaxKind::CALL_ARGS, 0x800d),
        (SyntaxKind::LAMBDA_EXPR, 0x800e),
        (SyntaxKind::LAMBDA_PARAMS, 0x800f),
        (SyntaxKind::CODE_BLOCK, 0x8010),
        (SyntaxKind::LET_STMT, 0x8011),
        (SyntaxKind::COLUMN_SPEC, 0x8012),
        (SyntaxKind::COLUMN_SPEC_ARRAY, 0x8013),
        (SyntaxKind::NEW_INSTANCE_EXPR, 0x8014),
        (SyntaxKind::CAST_EXPR, 0x8015),
        (SyntaxKind::RELATION_TYPE, 0x8016),
        (SyntaxKind::COLUMN_INFO, 0x8017),
        (SyntaxKind::ISLAND, 0x8018),
        (SyntaxKind::STORE_TABLE_POINTER, 0x8019),
        (SyntaxKind::NAV_PATH_ISLAND, 0x801a),
        (SyntaxKind::OPAQUE_ISLAND, 0x801b),
        (SyntaxKind::TYPE_REF, 0x801c),
        (SyntaxKind::MULTIPLICITY, 0x801d),
        (SyntaxKind::FUNCTION_CALL, 0x801e),
        (SyntaxKind::COLLECTION_LITERAL, 0x801f),
        (SyntaxKind::COLUMN_NAME, 0x8020),
    ];

    assert_eq!(SyntaxKind::all().len(), expected.len());
    for (actual, (kind, value)) in SyntaxKind::all().iter().copied().zip(expected) {
        assert_eq!(actual, kind);
        let raw = RawSyntaxKind::from(kind);
        assert_eq!(raw.get(), value);
        assert_eq!(u16::from(raw), value);
        assert_eq!(SyntaxKind::try_from(RawSyntaxKind::new(value)), Ok(kind));
    }

    for value in [0x0032, 0x7fff, 0x8021, u16::MAX] {
        let error = SyntaxKind::try_from(RawSyntaxKind::new(value))
            .expect_err("unassigned raw kind must be rejected");
        assert_eq!(error.value(), value);
    }
}

#[test]
fn rejects_noncontiguous_and_non_utf8_token_ranges_without_panicking() {
    let gap_tokens = [
        (
            LexerSyntaxKind::IDENT,
            TextRange::new(TextSize::from(0), TextSize::from(1)),
        ),
        (
            LexerSyntaxKind::IDENT,
            TextRange::new(TextSize::from(2), TextSize::from(3)),
        ),
    ];
    let gap = panic::catch_unwind(|| finish_with_events("abc", &gap_tokens, []));
    assert!(matches!(
        gap.expect("builder must not panic"),
        Err(BuildError::NonContiguousTokenRange { token_index: 1, .. })
    ));

    let split_code_point = [(
        LexerSyntaxKind::ERROR,
        TextRange::new(TextSize::from(0), TextSize::from(1)),
    )];
    let unicode = panic::catch_unwind(|| finish_with_events("λ", &split_code_point, []));
    assert!(matches!(
        unicode.expect("builder must not panic"),
        Err(BuildError::InvalidUtf8Boundary { token_index: 0, .. })
    ));
}

#[test]
fn rejects_overlap_empty_out_of_source_and_incomplete_ranges() {
    let overlap = [
        (
            LexerSyntaxKind::IDENT,
            TextRange::new(TextSize::from(0), TextSize::from(2)),
        ),
        (
            LexerSyntaxKind::IDENT,
            TextRange::new(TextSize::from(1), TextSize::from(3)),
        ),
    ];
    assert!(matches!(
        finish_with_events("abc", &overlap, []),
        Err(BuildError::NonContiguousTokenRange {
            token_index: 1,
            expected,
            actual,
        }) if expected == TextSize::from(2) && actual == TextSize::from(1)
    ));

    let empty = [(
        LexerSyntaxKind::IDENT,
        TextRange::new(TextSize::from(0), TextSize::from(0)),
    )];
    assert!(matches!(
        finish_with_events("", &empty, []),
        Err(BuildError::EmptyTokenRange { token_index: 0, .. })
    ));

    let outside = [(
        LexerSyntaxKind::IDENT,
        TextRange::new(TextSize::from(0), TextSize::from(2)),
    )];
    assert!(matches!(
        finish_with_events("a", &outside, []),
        Err(BuildError::TokenRangeOutsideSource { token_index: 0, .. })
    ));

    let incomplete = [(
        LexerSyntaxKind::IDENT,
        TextRange::new(TextSize::from(0), TextSize::from(1)),
    )];
    assert_eq!(
        finish_with_events("ab", &incomplete, []),
        Err(BuildError::IncompleteTokenCoverage {
            expected: TextSize::from(2),
            actual: TextSize::from(1),
        })
    );
}

#[test]
fn rejects_maximum_text_range_without_overflowing_or_panicking() {
    let tokens = [(
        LexerSyntaxKind::IDENT,
        TextRange::new(TextSize::from(0), TextSize::from(u32::MAX)),
    )];
    let result = panic::catch_unwind(|| finish_with_events("x", &tokens, []));

    assert!(matches!(
        result.expect("maximum TextSize range must not panic"),
        Err(BuildError::TokenRangeOutsideSource { token_index: 0, .. })
    ));
}

#[test]
fn rejects_malformed_event_streams_without_panicking() {
    use pure_analyzer_syntax::Event::{Advance, Close, Open};

    let cases = [
        (vec![Advance], "advance outside a node"),
        (vec![Close], "close without open"),
        (vec![Open(SyntaxKind::ROOT)], "unclosed root"),
        (
            vec![Open(SyntaxKind::IDENT), Close],
            "terminal opened as a node",
        ),
        (
            vec![Open(SyntaxKind::ROOT), Close, Open(SyntaxKind::ROOT), Close],
            "two roots",
        ),
        (vec![Open(SyntaxKind::ERROR_NODE), Close], "wrong root kind"),
    ];

    for (events, context) in cases {
        let result = panic::catch_unwind(|| finish_with_events("", &[], events));
        assert!(result.is_ok(), "{context} panicked");
        assert!(
            result
                .expect("malformed event stream must not panic")
                .is_err(),
            "{context} was accepted"
        );
    }
}

#[test]
fn rejects_unconsumed_and_overconsumed_tokens() {
    use pure_analyzer_syntax::Event::{Advance, Close, Open};

    let tokens = lex("x");
    assert!(matches!(
        finish_with_events("x", &tokens, [Open(SyntaxKind::ROOT), Close]),
        Err(BuildError::UnconsumedTokens {
            consumed: 0,
            total: 1
        })
    ));
    assert!(matches!(
        finish_with_events(
            "x",
            &tokens,
            [Open(SyntaxKind::ROOT), Advance, Advance, Close]
        ),
        Err(BuildError::AdvancePastTokens { token_count: 1, .. })
    ));
}

#[test]
fn rejects_a_root_nested_inside_another_root() {
    use pure_analyzer_syntax::Event::{Advance, Close, Open};

    let tokens = lex("x");
    assert_eq!(
        finish_with_events(
            "x",
            &tokens,
            [
                Open(SyntaxKind::ROOT),
                Open(SyntaxKind::ROOT),
                Advance,
                Close,
                Close,
            ]
        ),
        Err(BuildError::NestedRoot { event_index: 1 })
    );
}

#[test]
fn rejects_a_checkpoint_from_another_builder() {
    let tokens = lex("");
    let mut first = GreenNodeBuilder::new("", &tokens);
    let checkpoint = first.checkpoint();
    let mut second = GreenNodeBuilder::new("", &tokens);

    assert_eq!(
        second.open_at(&checkpoint, SyntaxKind::ROOT),
        Err(BuildError::ForeignCheckpoint)
    );
}

#[test]
fn accepts_an_empty_source_root() {
    let tree = flat_tree("");

    assert_eq!(tree.kind(), SyntaxKind::ROOT);
    assert_eq!(tree.text(), "");
    assert_eq!(
        tree.text_range(),
        TextRange::new(TextSize::from(0), TextSize::from(0))
    );
    assert!(tree.children().is_empty());
    assert!(tree.tokens().next().is_none());
}

#[test]
fn green_tree_clone_and_rebuild_are_structurally_equal() {
    let source = "let value = 'same'";
    let tree = flat_tree(source);

    assert_eq!(tree.clone(), tree);
    assert_eq!(flat_tree(source), tree);
    assert_ne!(flat_tree("let value = 'different'"), tree);
}

#[test]
fn green_values_are_send_sync_and_support_threaded_traversal() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<GreenNode>();
    assert_send_sync::<GreenElement>();
    assert_send_sync::<pure_analyzer_syntax::GreenToken>();

    let source = "let x = 1";
    let tree = flat_tree(source);
    let cloned = tree.clone();
    let owned = thread::spawn(move || cloned.text());
    assert_eq!(
        owned.join().expect("owned traversal thread panicked"),
        source
    );

    thread::scope(|scope| {
        let borrowed = scope.spawn(|| tree.tokens().map(|token| token.text()).collect::<String>());
        assert_eq!(
            borrowed.join().expect("borrowed traversal thread panicked"),
            source
        );
    });
}

const PROPTEST_CASES: u32 = 256;

proptest! {
    #![proptest_config(ProptestConfig {
        cases: PROPTEST_CASES,
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    #[test]
    fn arbitrary_lexer_output_round_trips_through_the_tree(source in any::<String>()) {
        let lexer_tokens = lex(&source);
        let tree = flat_tree(&source);
        let tree_tokens = tree.tokens().collect::<Vec<_>>();

        prop_assert_eq!(tree.text(), source.as_str());
        prop_assert_eq!(tree.to_string(), source.as_str());
        prop_assert_eq!(tree_tokens.len(), lexer_tokens.len());
        for ((lexer_kind, lexer_range), tree_token) in lexer_tokens.iter().zip(tree_tokens) {
            prop_assert_eq!(tree_token.kind(), (*lexer_kind).into());
            prop_assert_eq!(tree_token.text_range(), *lexer_range);
        }
    }
}
