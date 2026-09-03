//! Recovery and arbitrary-input contracts for the M3 parser.

#[path = "support/lexeme_strategy.rs"]
mod lexeme_strategy;

use std::{ops::Range, panic};

use lexeme_strategy::arbitrary_source;
use proptest::prelude::*;
use pure_analyzer_diagnostics::{DiagCode, FileId};
use pure_analyzer_lexer::lex;
use pure_analyzer_parser::parse_query;
use pure_analyzer_syntax::{GreenElement, GreenNode, SyntaxKind, TextRange};

fn test_file() -> FileId {
    FileId::new(73)
}

const EXCESSIVE_NESTING: usize = 1_024;
const LONG_BINARY_CHAIN: usize = 4_096;
const LONG_MIXED_BINARY_CHAIN: usize = 512;
const EXCESSIVE_POSTFIX_CHAIN: usize = 320;
const MANY_SIBLING_FUNCTION_CALLS: usize = 300;

fn parse(source: &str) -> pure_analyzer_parser::Parse {
    parse_query(source, test_file()).expect("small test sources must build a tree")
}

fn assert_ranges_are_valid(source: &str, parsed: &pure_analyzer_parser::Parse) {
    for diagnostic in &parsed.diagnostics {
        assert_eq!(diagnostic.primary.file, test_file());
        assert_range_is_valid(source, diagnostic.primary.span);
    }
    for token in parsed.green.tokens() {
        assert_range_is_valid(source, token.text_range());
    }
}

fn assert_range_is_valid(source: &str, range: TextRange) {
    let start = usize::from(range.start());
    let end = usize::from(range.end());
    assert!(start <= end);
    assert!(end <= source.len());
    assert!(source.is_char_boundary(start));
    assert!(source.is_char_boundary(end));
}

fn count_kind(node: &GreenNode, kind: SyntaxKind) -> usize {
    usize::from(node.kind() == kind)
        + node
            .children()
            .iter()
            .map(|element| match element {
                GreenElement::Node(child) => count_kind(child, kind),
                GreenElement::Token(_) => 0,
            })
            .sum::<usize>()
}

fn max_kind_depth(node: &GreenNode, kind: SyntaxKind) -> usize {
    let descendant_depth = node
        .children()
        .iter()
        .filter_map(GreenElement::as_node)
        .map(|child| max_kind_depth(child, kind))
        .max()
        .unwrap_or(0);
    descendant_depth + usize::from(node.kind() == kind)
}

fn diagnostic_codes(parsed: &pure_analyzer_parser::Parse) -> Vec<DiagCode> {
    parsed
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code)
        .collect()
}

fn diagnostic_details(
    parsed: &pure_analyzer_parser::Parse,
) -> Vec<(DiagCode, String, Range<usize>)> {
    parsed
        .diagnostics
        .iter()
        .map(|diagnostic| {
            (
                diagnostic.code,
                diagnostic.message.clone(),
                usize::from(diagnostic.primary.span.start())
                    ..usize::from(diagnostic.primary.span.end()),
            )
        })
        .collect()
}

fn nodes_with_kind<'tree>(
    node: &'tree GreenNode,
    kind: SyntaxKind,
    nodes: &mut Vec<&'tree GreenNode>,
) {
    if node.kind() == kind {
        nodes.push(node);
    }
    for child in node.children().iter().filter_map(GreenElement::as_node) {
        nodes_with_kind(child, kind, nodes);
    }
}

fn only_node_of_kind(node: &GreenNode, kind: SyntaxKind) -> &GreenNode {
    let mut nodes = Vec::new();
    nodes_with_kind(node, kind, &mut nodes);
    assert_eq!(nodes.len(), 1, "expected one {kind:?}, got {nodes:#?}");
    nodes[0]
}

fn syntax_error_count(parsed: &pure_analyzer_parser::Parse) -> usize {
    parsed
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code == DiagCode::MalformedSyntax)
        .count()
}

#[test]
fn lexical_errors_are_recovered_without_losing_later_source() {
    let source = "$x \u{0} ->filter(y|$y.name); model::Person.all()";
    let parsed = parse(source);

    assert_eq!(parsed.green.text(), source);
    assert!(
        parsed
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == DiagCode::BadToken)
    );
    assert_ranges_are_valid(source, &parsed);
}

#[test]
fn incomplete_delimiters_return_a_tree_and_structured_diagnostics() {
    for source in [
        "$",
        "model::Person.all(",
        "[",
        "[a,",
        "[a,]",
        "[[1]",
        "{x| let y =",
        "#>{db::Model.table",
        "#{ TDS",
    ] {
        let result = panic::catch_unwind(|| parse(source));
        let parsed = result.expect("parser must not panic for incomplete input");

        assert_eq!(parsed.green.text(), source);
        assert!(!parsed.diagnostics.is_empty(), "{source}");
        assert_ranges_are_valid(source, &parsed);
    }
}

#[test]
fn incomplete_braced_lambda_retains_the_recovery_boundary() {
    let source = "{x| $x";
    let parsed = parse(source);
    let syntax_errors = syntax_error_count(&parsed);

    assert_eq!(parsed.green.text(), source);
    assert_eq!(syntax_errors, 3, "{:#?}", parsed.diagnostics);
    assert!(count_kind(&parsed.green, SyntaxKind::ERROR_NODE) > 0);
    assert_ranges_are_valid(source, &parsed);
}

#[test]
fn source_separator_recovery_keeps_the_next_query_boundary() {
    let source = "first second; model::Person.all()";
    let parsed = parse(source);

    assert_eq!(parsed.green.text(), source);
    assert_eq!(diagnostic_codes(&parsed), [DiagCode::MalformedSyntax]);
    assert_eq!(count_kind(&parsed.green, SyntaxKind::QUERY_EXPR), 2);
    assert!(count_kind(&parsed.green, SyntaxKind::ERROR_NODE) > 0);
    assert_ranges_are_valid(source, &parsed);
}

#[test]
fn bad_tokens_remain_lexical_errors_inside_explicit_error_nodes() {
    let source = "\0";
    let parsed = parse(source);

    assert_eq!(parsed.green.text(), source);
    assert_eq!(diagnostic_codes(&parsed), [DiagCode::BadToken]);
    assert_eq!(count_kind(&parsed.green, SyntaxKind::ERROR_NODE), 1);
    assert_ranges_are_valid(source, &parsed);
}

#[test]
fn malformed_primary_does_not_become_a_qualified_name() {
    let source = ")";
    let parsed = parse(source);

    assert_eq!(parsed.green.text(), source);
    assert_eq!(
        diagnostic_codes(&parsed),
        [DiagCode::MalformedSyntax, DiagCode::MalformedSyntax]
    );
    assert_eq!(count_kind(&parsed.green, SyntaxKind::QUALIFIED_NAME), 0);
    assert!(count_kind(&parsed.green, SyntaxKind::ERROR_NODE) > 0);
    assert_ranges_are_valid(source, &parsed);
}

#[test]
fn malformed_collection_literal_preserves_the_next_top_level_query() {
    let source = "f([a, ); model::Person.all()";
    let parsed = parse(source);

    assert_eq!(parsed.green.text(), source);
    assert!(
        parsed
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == DiagCode::MalformedSyntax)
    );
    assert!(count_kind(&parsed.green, SyntaxKind::COLLECTION_LITERAL) > 0);
    assert_eq!(count_kind(&parsed.green, SyntaxKind::QUERY_EXPR), 2);
    assert_ranges_are_valid(source, &parsed);
}

#[test]
fn empty_collection_stops_at_the_closing_delimiter() {
    let source = "[]";
    let parsed = parse(source);
    let collection = only_node_of_kind(&parsed.green, SyntaxKind::COLLECTION_LITERAL);

    assert_eq!(parsed.green.text(), source);
    assert_eq!(collection.text(), source);
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    assert_eq!(count_kind(collection, SyntaxKind::ERROR_NODE), 0);
    assert_ranges_are_valid(source, &parsed);
}

#[test]
fn malformed_collection_item_recovers_at_a_comma_and_keeps_later_items() {
    let source = "[a, ), c]";
    let parsed = parse(source);
    let collection = only_node_of_kind(&parsed.green, SyntaxKind::COLLECTION_LITERAL);

    assert_eq!(parsed.green.text(), source);
    assert_eq!(collection.text(), source);
    assert!(diagnostic_codes(&parsed).contains(&DiagCode::MalformedSyntax));
    assert_eq!(count_kind(collection, SyntaxKind::QUALIFIED_NAME), 2);
    assert_eq!(count_kind(collection, SyntaxKind::ERROR_NODE), 1);
    assert_ranges_are_valid(source, &parsed);
}

#[test]
fn malformed_collection_item_recovers_at_the_closing_delimiter() {
    let source = "[a, )]";
    let parsed = parse(source);
    let collection = only_node_of_kind(&parsed.green, SyntaxKind::COLLECTION_LITERAL);

    assert_eq!(parsed.green.text(), source);
    assert_eq!(collection.text(), source);
    assert_eq!(syntax_error_count(&parsed), 2, "{:#?}", parsed.diagnostics);
    assert_eq!(count_kind(collection, SyntaxKind::QUALIFIED_NAME), 1);
    assert_eq!(count_kind(collection, SyntaxKind::ERROR_NODE), 1);
    assert_ranges_are_valid(source, &parsed);
}

#[test]
fn collection_trailing_comma_stops_before_the_closing_delimiter() {
    let source = "[a,]";
    let parsed = parse(source);
    let collection = only_node_of_kind(&parsed.green, SyntaxKind::COLLECTION_LITERAL);

    assert_eq!(parsed.green.text(), source);
    assert_eq!(collection.text(), source);
    assert_eq!(count_kind(collection, SyntaxKind::QUALIFIED_NAME), 1);
    assert_eq!(count_kind(collection, SyntaxKind::ERROR_NODE), 0);
    assert_eq!(
        diagnostic_details(&parsed),
        vec![(
            DiagCode::MalformedSyntax,
            "expected an expression after `,`".to_owned(),
            3..4,
        )]
    );
    assert_ranges_are_valid(source, &parsed);
}

#[test]
fn malformed_column_array_member_recovers_to_later_quoted_aliases() {
    let source = "~['first': x| $x 'discarded': y| $y, 'later': z| $z]";
    let parsed = parse(source);
    let columns = only_node_of_kind(&parsed.green, SyntaxKind::COLUMN_SPEC_ARRAY);

    assert_eq!(parsed.green.text(), source);
    assert_eq!(columns.text(), source);
    assert!(diagnostic_codes(&parsed).contains(&DiagCode::MalformedSyntax));
    assert_eq!(count_kind(columns, SyntaxKind::COLUMN_NAME), 2);
    assert_eq!(count_kind(columns, SyntaxKind::ERROR_NODE), 1);
    assert_ranges_are_valid(source, &parsed);
}

#[test]
fn invalid_column_array_member_makes_progress_to_later_quoted_aliases() {
    let source = "~['first': x| $x, ), 'later': y| $y]";
    let parsed = parse(source);
    let columns = only_node_of_kind(&parsed.green, SyntaxKind::COLUMN_SPEC_ARRAY);

    assert_eq!(parsed.green.text(), source);
    assert_eq!(columns.text(), source);
    assert!(diagnostic_codes(&parsed).contains(&DiagCode::MalformedSyntax));
    assert_eq!(count_kind(columns, SyntaxKind::COLUMN_NAME), 2);
    assert!(count_kind(columns, SyntaxKind::ERROR_NODE) > 0);
    assert_ranges_are_valid(source, &parsed);
}

#[test]
fn empty_column_array_member_makes_progress_to_later_quoted_aliases() {
    let source = "~['first': x| $x, , 'later': y| $y]";
    let parsed = parse(source);
    let columns = only_node_of_kind(&parsed.green, SyntaxKind::COLUMN_SPEC_ARRAY);

    assert_eq!(parsed.green.text(), source);
    assert_eq!(columns.text(), source);
    assert!(diagnostic_codes(&parsed).contains(&DiagCode::MalformedSyntax));
    assert_eq!(count_kind(columns, SyntaxKind::COLUMN_NAME), 2);
    assert_ranges_are_valid(source, &parsed);
}

#[test]
fn invalid_column_array_members_stop_cleanly_at_closing_bracket_and_eof() {
    for (source, expected_syntax_errors, expected_error_nodes) in [("~[)]", 1, 1), ("~[)", 4, 2)] {
        let parsed = parse(source);
        let columns = only_node_of_kind(&parsed.green, SyntaxKind::COLUMN_SPEC_ARRAY);

        assert_eq!(parsed.green.text(), source);
        assert_eq!(columns.text(), source);
        assert_eq!(
            syntax_error_count(&parsed),
            expected_syntax_errors,
            "{source}"
        );
        assert_eq!(count_kind(columns, SyntaxKind::COLUMN_NAME), 0, "{source}");
        assert_eq!(
            count_kind(&parsed.green, SyntaxKind::ERROR_NODE),
            expected_error_nodes,
            "{source}"
        );
        assert_ranges_are_valid(source, &parsed);
    }
}

#[test]
fn trailing_column_array_commas_distinguish_closing_bracket_from_eof() {
    for (source, expected_syntax_errors, expected_error_nodes) in
        [("~[first:String[1],]", 1, 0), ("~[first:String[1],", 4, 1)]
    {
        let parsed = parse(source);
        let columns = only_node_of_kind(&parsed.green, SyntaxKind::COLUMN_SPEC_ARRAY);

        assert_eq!(parsed.green.text(), source);
        assert_eq!(columns.text(), source);
        assert_eq!(
            syntax_error_count(&parsed),
            expected_syntax_errors,
            "{source}"
        );
        assert_eq!(count_kind(columns, SyntaxKind::COLUMN_NAME), 1, "{source}");
        assert_eq!(
            count_kind(&parsed.green, SyntaxKind::ERROR_NODE),
            expected_error_nodes,
            "{source}"
        );
        assert_ranges_are_valid(source, &parsed);
    }
}

#[test]
fn missing_column_array_separators_stop_cleanly_at_closing_bracket_and_eof() {
    for (source, expected_syntax_errors, expected_error_nodes) in [
        ("~[first:String[1] )]", 1, 1),
        ("~[first:String[1] )", 4, 2),
    ] {
        let parsed = parse(source);
        let columns = only_node_of_kind(&parsed.green, SyntaxKind::COLUMN_SPEC_ARRAY);

        assert_eq!(parsed.green.text(), source);
        assert_eq!(columns.text(), source);
        assert_eq!(count_kind(columns, SyntaxKind::COLUMN_NAME), 1, "{source}");
        assert_eq!(
            syntax_error_count(&parsed),
            expected_syntax_errors,
            "{source}"
        );
        assert_eq!(
            count_kind(&parsed.green, SyntaxKind::ERROR_NODE),
            expected_error_nodes,
            "{source}"
        );
        assert_ranges_are_valid(source, &parsed);
    }
}

#[test]
fn recovered_column_array_commas_report_closing_bracket_and_eof_boundaries() {
    for (source, expected_columns, expected_syntax_errors, expected_error_nodes) in [
        ("~[),]", 0, 2, 1),
        ("~[),", 0, 5, 2),
        ("~[first:String[1] ),]", 1, 2, 1),
        ("~[first:String[1] ),", 1, 5, 2),
    ] {
        let parsed = parse(source);
        let columns = only_node_of_kind(&parsed.green, SyntaxKind::COLUMN_SPEC_ARRAY);

        assert_eq!(parsed.green.text(), source);
        assert_eq!(columns.text(), source);
        assert_eq!(
            count_kind(columns, SyntaxKind::COLUMN_NAME),
            expected_columns,
            "{source}"
        );
        assert_eq!(
            syntax_error_count(&parsed),
            expected_syntax_errors,
            "{source}"
        );
        assert_eq!(
            count_kind(&parsed.green, SyntaxKind::ERROR_NODE),
            expected_error_nodes,
            "{source}"
        );
        assert_ranges_are_valid(source, &parsed);
    }
}

#[test]
fn malformed_column_arrays_stop_at_source_separators_and_keep_later_queries() {
    let source = "~[); model::Person.all()";
    let parsed = parse(source);
    let columns = only_node_of_kind(&parsed.green, SyntaxKind::COLUMN_SPEC_ARRAY);

    assert_eq!(parsed.green.text(), source);
    assert_eq!(columns.text(), "~[)");
    assert!(diagnostic_codes(&parsed).contains(&DiagCode::MalformedSyntax));
    assert_eq!(syntax_error_count(&parsed), 2, "{:#?}", parsed.diagnostics);
    assert_eq!(count_kind(&parsed.green, SyntaxKind::QUERY_EXPR), 2);
    assert_ranges_are_valid(source, &parsed);
}

#[test]
fn incomplete_variables_and_parentheses_propagate_to_outer_recovery() {
    for (source, expected_queries) in [("$); model::Person.all()", 2), ("(a", 1)] {
        let parsed = parse(source);

        assert_eq!(parsed.green.text(), source);
        assert_eq!(
            syntax_error_count(&parsed),
            3,
            "{source}: {:#?}",
            parsed.diagnostics
        );
        assert_eq!(
            count_kind(&parsed.green, SyntaxKind::QUERY_EXPR),
            expected_queries,
            "{source}"
        );
        assert!(
            count_kind(&parsed.green, SyntaxKind::ERROR_NODE) > 0,
            "{source}"
        );
        assert_ranges_are_valid(source, &parsed);
    }
}

#[test]
fn unterminated_islands_are_distinct_from_syntax_recovery() {
    for source in ["#>{db::Model.table", "#{ TDS", "#unterminated"] {
        let parsed = parse(source);

        assert_eq!(parsed.green.text(), source);
        assert!(
            diagnostic_codes(&parsed).contains(&DiagCode::UnterminatedIsland),
            "{source}: {:#?}",
            parsed.diagnostics
        );
        assert!(
            count_kind(&parsed.green, SyntaxKind::ERROR_NODE) > 0,
            "{source}"
        );
        assert_ranges_are_valid(source, &parsed);
    }
}

#[test]
fn malformed_input_recovers_at_a_top_level_semicolon() {
    let source = ") ; model::Person.all()";
    let parsed = parse(source);
    let tree_tokens = parsed.green.tokens().collect::<Vec<_>>();
    let lexer_tokens = lex(source);

    assert_eq!(parsed.green.text(), source);
    assert_eq!(tree_tokens.len(), lexer_tokens.len());
    assert!(
        parsed
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == DiagCode::MalformedSyntax)
    );
    assert_ranges_are_valid(source, &parsed);
}

#[test]
fn malformed_argument_recovers_to_the_next_comma_without_losing_it() {
    let source = "f(] , a)";
    let parsed = parse(source);

    assert_eq!(parsed.green.text(), source);
    assert_eq!(
        diagnostic_codes(&parsed),
        [DiagCode::MalformedSyntax, DiagCode::MalformedSyntax]
    );
    assert_eq!(count_kind(&parsed.green, SyntaxKind::FUNCTION_CALL), 1);
    assert_eq!(count_kind(&parsed.green, SyntaxKind::CALL_ARGS), 1);
    assert_eq!(count_kind(&parsed.green, SyntaxKind::ERROR_NODE), 1);
    assert_ranges_are_valid(source, &parsed);
}

#[test]
fn missing_argument_comma_recovers_inside_the_same_call() {
    let source = "f(a b, c)";
    let parsed = parse(source);
    let arguments = only_node_of_kind(&parsed.green, SyntaxKind::CALL_ARGS);

    assert_eq!(parsed.green.text(), source);
    assert_eq!(arguments.text(), "(a b, c)");
    assert_eq!(
        diagnostic_details(&parsed),
        vec![
            (
                DiagCode::MalformedSyntax,
                "expected `,` or `)` after an argument".to_owned(),
                4..5,
            ),
            (
                DiagCode::MalformedSyntax,
                "expected an operand after a unary operator".to_owned(),
                5..6,
            ),
            (
                DiagCode::MalformedSyntax,
                "expected an argument expression".to_owned(),
                5..6,
            ),
        ]
    );
    assert_eq!(count_kind(arguments, SyntaxKind::ERROR_NODE), 2);
    assert_ranges_are_valid(source, &parsed);
}

#[test]
fn malformed_delimiters_preserve_the_next_top_level_query() {
    let source = "foo(] ; model::Person.all()";
    let parsed = parse(source);

    assert_eq!(parsed.green.text(), source);
    assert!(
        parsed
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == DiagCode::MalformedSyntax)
    );
    assert_eq!(count_kind(&parsed.green, SyntaxKind::QUERY_EXPR), 2);
    assert_ranges_are_valid(source, &parsed);
}

#[test]
fn malformed_let_binding_preserves_the_next_code_block_statement() {
    let source = "{x| let y = ; $x.name()}";
    let parsed = parse(source);

    assert_eq!(parsed.green.text(), source);
    assert!(
        parsed
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == DiagCode::MalformedSyntax)
    );
    assert_eq!(count_kind(&parsed.green, SyntaxKind::LET_STMT), 1);
    assert_eq!(count_kind(&parsed.green, SyntaxKind::QUERY_EXPR), 2);
    assert_ranges_are_valid(source, &parsed);
}

#[test]
fn excessive_nesting_is_recovered_without_a_stack_overflow() {
    let source = format!(
        "{}value{}",
        "(".repeat(EXCESSIVE_NESTING),
        ")".repeat(EXCESSIVE_NESTING)
    );
    let result = panic::catch_unwind(|| parse(&source));
    let parsed = result.expect("deep input must not panic");

    assert_eq!(parsed.green.text(), source);
    assert!(
        parsed
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == DiagCode::MalformedSyntax)
    );
    assert_ranges_are_valid(&source, &parsed);
}

#[test]
fn long_left_associative_binary_chain_is_flat_and_lossless() {
    let source = format!("value{}", " + value".repeat(LONG_BINARY_CHAIN));
    let result = panic::catch_unwind(|| parse(&source));
    let parsed = result.expect("long binary chains must not panic");

    assert_eq!(parsed.green.text(), source);
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    assert_eq!(count_kind(&parsed.green, SyntaxKind::BINARY_EXPR), 1);
    assert_ranges_are_valid(&source, &parsed);
}

#[test]
fn binary_cst_keeps_precedence_without_repeated_associative_nesting() {
    let parsed = parse("a * b + c + d");

    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    assert_eq!(count_kind(&parsed.green, SyntaxKind::BINARY_EXPR), 2);
    assert_eq!(max_kind_depth(&parsed.green, SyntaxKind::BINARY_EXPR), 2);
}

#[test]
fn long_mixed_precedence_chain_remains_valid_and_lossless() {
    let source = format!(
        "value{}",
        " * value + value".repeat(LONG_MIXED_BINARY_CHAIN)
    );
    let parsed = parse(&source);

    assert_eq!(parsed.green.text(), source);
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    assert_ranges_are_valid(&source, &parsed);
}

#[test]
fn excessive_postfix_nesting_is_recovered_without_a_stack_overflow() {
    let source = format!("f{}", "()".repeat(EXCESSIVE_POSTFIX_CHAIN));
    let result = panic::catch_unwind(|| parse(&source));
    let parsed = result.expect("long postfix chains must not panic");

    assert_eq!(parsed.green.text(), source);
    assert!(
        parsed
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == DiagCode::MalformedSyntax)
    );
    assert_ranges_are_valid(&source, &parsed);
}

#[test]
fn sibling_function_calls_do_not_consume_the_nesting_budget() {
    let items = (0..MANY_SIBLING_FUNCTION_CALLS)
        .map(|index| format!("f({index})"))
        .collect::<Vec<_>>()
        .join(", ");
    let source = format!("[{items}];");
    let parsed = parse(&source);

    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    assert_eq!(
        count_kind(&parsed.green, SyntaxKind::FUNCTION_CALL),
        MANY_SIBLING_FUNCTION_CALLS
    );
    assert_eq!(max_kind_depth(&parsed.green, SyntaxKind::FUNCTION_CALL), 1);
    assert_ranges_are_valid(&source, &parsed);
}

proptest! {
    // `source in any::<String>()` used to draw from the full Unicode
    // codepoint space, so the odds of it ever emitting `let`, a balanced
    // `(`/`)` pair, or an island marker were effectively zero — none of the
    // recovery machinery this test exists for was ever exercised (issue
    // #299). `arbitrary_source()` instead samples a weighted sequence over
    // the parser's real lexeme alphabet, so recovery paths are reached at a
    // meaningful rate; see `tests/support/lexeme_strategy.rs`.
    //
    // The original test also asserted `prop_assert_eq!(&first, &second)`
    // across two calls to `parse` on the same input — unfalsifiable, since
    // `parse_query` is a pure function over a `Vec<(TokenKind, TextRange)>`
    // with no `HashMap` iteration, no threads, and no clock; nothing in the
    // parser could make two such calls diverge. Deleted rather than kept as
    // dead weight (issue #299); a real nondeterminism source, if one is ever
    // introduced, needs its own test built against that source, not a
    // standing assertion no code path can fail.
    #[test]
    fn arbitrary_token_sequence_is_lossless_and_recovery_safe(source in arbitrary_source()) {
        let result = panic::catch_unwind(|| parse(&source));
        let parsed = result.expect("parser must not panic");

        prop_assert_eq!(parsed.green.text(), source.as_str());
        assert_ranges_are_valid(&source, &parsed);

        // Recovery-safety: every `ERROR_NODE` the parser builds is reached
        // only through a call path that first pushes a diagnostic explaining
        // why (`Parser::error_current`/`unterminated_island`/the automatic
        // `BadToken` push in `Parser::bump` — see `src/m3.rs`), so recovery
        // never silently swallows an error.
        if count_kind(&parsed.green, SyntaxKind::ERROR_NODE) > 0 {
            prop_assert!(
                !parsed.diagnostics.is_empty(),
                "an ERROR_NODE was built without any diagnostic explaining it: {:#?}",
                parsed
            );
        }
    }
}
