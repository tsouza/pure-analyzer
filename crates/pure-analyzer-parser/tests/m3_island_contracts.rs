//! Focused contracts for M3 island parsing and recovery.

use std::ops::Range;

use pure_analyzer_diagnostics::{DiagCode, FileId, Severity};
use pure_analyzer_lexer::lex;
use pure_analyzer_parser::{Parse, parse_query};
use pure_analyzer_syntax::{GreenElement, GreenNode, SyntaxKind};

fn test_file() -> FileId {
    FileId::new(91)
}

fn parse(source: &str) -> Parse {
    parse_query(source, test_file()).expect("test source must build a lossless tree")
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

fn nodes_of_kind(node: &GreenNode, kind: SyntaxKind) -> Vec<&GreenNode> {
    let mut nodes = Vec::new();
    nodes_with_kind(node, kind, &mut nodes);
    nodes
}

fn only_node_of_kind(node: &GreenNode, kind: SyntaxKind) -> &GreenNode {
    let nodes = nodes_of_kind(node, kind);
    assert_eq!(nodes.len(), 1, "expected one {kind:?}, got {nodes:#?}");
    nodes[0]
}

fn direct_child_node_texts(node: &GreenNode, kind: SyntaxKind) -> Vec<String> {
    node.children()
        .iter()
        .filter_map(GreenElement::as_node)
        .filter(|child| child.kind() == kind)
        .map(GreenNode::text)
        .collect()
}

fn assert_lossless(source: &str, parsed: &Parse) {
    assert_eq!(parsed.green.text(), source);
    assert_eq!(parsed.green.to_string(), source);

    let tree_tokens = parsed.green.tokens().collect::<Vec<_>>();
    let lexer_tokens = lex(source);
    assert_eq!(tree_tokens.len(), lexer_tokens.len());
    for ((kind, range), token) in lexer_tokens.iter().zip(tree_tokens) {
        assert_eq!(token.kind(), (*kind).into());
        assert_eq!(token.text_range(), *range);
    }
}

fn diagnostic_ranges(parsed: &Parse) -> Vec<(DiagCode, Range<usize>)> {
    parsed
        .diagnostics
        .iter()
        .map(|diagnostic| {
            assert_eq!(diagnostic.severity, Severity::Error);
            assert_eq!(diagnostic.primary.file, test_file());
            (
                diagnostic.code,
                usize::from(diagnostic.primary.span.start())
                    ..usize::from(diagnostic.primary.span.end()),
            )
        })
        .collect()
}

#[test]
fn store_pointer_preserves_multitoken_trivia_and_token_spans() {
    let source = concat!(
        "#>{ // before database\n",
        " /* still before database */ db::schema /* before dot */\n",
        " /* still before dot */ . /* before table */\n",
        " /* still before table */ personTable /* before end */\n",
        " /* still before end */ }#",
    );
    let parsed = parse(source);

    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    assert_lossless(source, &parsed);
    assert_eq!(
        only_node_of_kind(&parsed.green, SyntaxKind::ISLAND).text(),
        source
    );
    assert_eq!(
        only_node_of_kind(&parsed.green, SyntaxKind::STORE_TABLE_POINTER).text(),
        source
    );
    assert!(
        parsed
            .green
            .tokens()
            .any(|token| token.kind() == SyntaxKind::LINE_COMMENT)
    );
    assert!(
        parsed
            .green
            .tokens()
            .any(|token| token.kind() == SyntaxKind::BLOCK_COMMENT)
    );
}

#[test]
fn store_pointer_component_failures_keep_local_and_outer_diagnostics_distinct() {
    for (source, first_range, first_message) in [
        ("#>{.table}#", 3..4, "expected a name"),
        (
            "#>{db::schema table}#",
            14..19,
            "expected `.` between a database and table name",
        ),
        ("#>{db::schema.}#", 14..16, "expected a table name"),
    ] {
        let parsed = parse(source);
        let eof = source.len()..source.len();

        assert_lossless(source, &parsed);
        assert_eq!(
            diagnostic_ranges(&parsed),
            vec![
                (DiagCode::MalformedSyntax, first_range),
                (DiagCode::MalformedSyntax, eof),
            ],
            "{source}",
        );
        assert_eq!(parsed.diagnostics[0].message, first_message, "{source}");
        assert_eq!(
            nodes_of_kind(&parsed.green, SyntaxKind::ERROR_NODE).len(),
            1
        );
        assert_eq!(
            only_node_of_kind(&parsed.green, SyntaxKind::STORE_TABLE_POINTER).text(),
            source,
        );
    }

    let source = "#>{db::schema.table";
    let parsed = parse(source);
    let eof = source.len()..source.len();

    assert_lossless(source, &parsed);
    assert_eq!(
        diagnostic_ranges(&parsed),
        vec![
            (DiagCode::UnterminatedIsland, eof.clone()),
            (DiagCode::MalformedSyntax, eof),
        ],
    );
    assert_eq!(parsed.diagnostics[0].message, "unterminated island");
    assert_eq!(
        nodes_of_kind(&parsed.green, SyntaxKind::ERROR_NODE).len(),
        3
    );
}

#[test]
fn braced_opaque_islands_respect_the_delimiter_stack() {
    for source in [
        "#{ { first }# second }# third } }#",
        "#{ stray } tail }#",
        "#{ outer #{ nested }# #>{db::schema.table}# tail }#",
    ] {
        let parsed = parse(source);

        assert!(
            parsed.diagnostics.is_empty(),
            "{source}: {:#?}",
            parsed.diagnostics
        );
        assert_lossless(source, &parsed);
        assert_eq!(nodes_of_kind(&parsed.green, SyntaxKind::ISLAND).len(), 1);
        assert_eq!(
            nodes_of_kind(&parsed.green, SyntaxKind::OPAQUE_ISLAND).len(),
            1
        );
        assert_eq!(
            only_node_of_kind(&parsed.green, SyntaxKind::OPAQUE_ISLAND).text(),
            source,
        );
        assert_eq!(
            nodes_of_kind(&parsed.green, SyntaxKind::ERROR_NODE).len(),
            0
        );
    }
}

#[test]
fn opaque_island_terminators_and_eof_diagnostics_are_distinct() {
    for source in ["#TDS body#", "#TDS body}#"] {
        let parsed = parse(source);

        assert!(
            parsed.diagnostics.is_empty(),
            "{source}: {:#?}",
            parsed.diagnostics
        );
        assert_lossless(source, &parsed);
        assert_eq!(
            only_node_of_kind(&parsed.green, SyntaxKind::OPAQUE_ISLAND).text(),
            source,
        );
    }

    for source in ["#{ still open", "#still open"] {
        let parsed = parse(source);
        let eof = source.len()..source.len();

        assert_lossless(source, &parsed);
        assert_eq!(
            diagnostic_ranges(&parsed),
            vec![
                (DiagCode::UnterminatedIsland, eof.clone()),
                (DiagCode::MalformedSyntax, eof),
            ],
            "{source}",
        );
        assert_eq!(parsed.diagnostics[0].message, "unterminated island");
        assert_eq!(
            nodes_of_kind(&parsed.green, SyntaxKind::ERROR_NODE).len(),
            2
        );
    }
}

#[test]
fn store_recovery_stops_at_semicolon_without_absorbing_it() {
    let source = "#>{db::schema.table trailing more; ignored";
    let parsed = parse(source);

    assert_lossless(source, &parsed);
    assert_eq!(
        diagnostic_ranges(&parsed),
        vec![
            (DiagCode::UnterminatedIsland, 20..28),
            (DiagCode::MalformedSyntax, 33..34),
            (DiagCode::MalformedSyntax, 35..42),
        ],
    );
    let store = only_node_of_kind(&parsed.green, SyntaxKind::STORE_TABLE_POINTER);
    assert_eq!(store.text(), "#>{db::schema.table trailing more");
    assert_eq!(
        direct_child_node_texts(store, SyntaxKind::ERROR_NODE),
        vec![String::new(), String::from("trailing more")],
    );
    assert!(!store.tokens().any(|token| token.text() == ";"));
    assert_eq!(
        nodes_of_kind(&parsed.green, SyntaxKind::ERROR_NODE).len(),
        4
    );
}

#[test]
fn source_recovery_stops_before_semicolon_and_keeps_the_next_query() {
    let source = "first second third; model::Person.all()";
    let parsed = parse(source);

    assert_lossless(source, &parsed);
    assert_eq!(
        diagnostic_ranges(&parsed),
        vec![(DiagCode::MalformedSyntax, 6..12)],
    );
    assert_eq!(
        direct_child_node_texts(&parsed.green, SyntaxKind::ERROR_NODE),
        vec![String::from("second third")],
    );
    assert_eq!(
        nodes_of_kind(&parsed.green, SyntaxKind::QUERY_EXPR).len(),
        2
    );
}
