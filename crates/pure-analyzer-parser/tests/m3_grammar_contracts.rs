//! Grammar-boundary contracts for the M3 parser.

use pure_analyzer_diagnostics::{DiagCode, FileId};
use pure_analyzer_parser::parse_query;
use pure_analyzer_syntax::{GreenElement, GreenNode, SyntaxKind};

const TEST_FILE_ID: u32 = 91;
const OUTER_EXPRESSION_FAILURE_DIAGNOSTICS: usize = 3;
const OUTER_EXPRESSION_FAILURE_NODES: usize = 1;
const GENERIC_TYPE_REFERENCE_COUNT: usize = 3;

fn test_file() -> FileId {
    FileId::new(TEST_FILE_ID)
}

fn parse(source: &str) -> pure_analyzer_parser::Parse {
    parse_query(source, test_file()).expect("small test sources must build a tree")
}

fn contains_kind(node: &GreenNode, kind: SyntaxKind) -> bool {
    node.kind() == kind
        || node.children().iter().any(|element| match element {
            GreenElement::Node(child) => contains_kind(child, kind),
            GreenElement::Token(_) => false,
        })
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

fn syntax_error_count(parsed: &pure_analyzer_parser::Parse) -> usize {
    parsed
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code == DiagCode::MalformedSyntax)
        .count()
}

fn assert_valid(source: &str, expected_kinds: &[SyntaxKind]) {
    let parsed = parse(source);

    assert_eq!(parsed.green.text(), source);
    assert!(
        parsed.diagnostics.is_empty(),
        "{source}: {:#?}",
        parsed.diagnostics
    );
    for kind in expected_kinds {
        assert!(
            contains_kind(&parsed.green, *kind),
            "{source}: missing {kind:?}"
        );
    }
}

fn assert_outer_expression_failure(source: &str, expected_kind: SyntaxKind) {
    let parsed = parse(source);

    assert_eq!(parsed.green.text(), source);
    assert_eq!(
        syntax_error_count(&parsed),
        OUTER_EXPRESSION_FAILURE_DIAGNOSTICS,
        "{source}: {:#?}",
        parsed.diagnostics
    );
    assert_eq!(
        count_kind(&parsed.green, SyntaxKind::ERROR_NODE),
        OUTER_EXPRESSION_FAILURE_NODES,
        "{source}"
    );
    assert!(contains_kind(&parsed.green, expected_kind), "{source}");
}

#[test]
fn typed_short_lambdas_keep_generic_types_and_star_multiplicity() {
    let source = "item: Map<String, Integer>[*]| $item";

    assert_valid(
        source,
        &[
            SyntaxKind::LAMBDA_EXPR,
            SyntaxKind::LAMBDA_PARAMS,
            SyntaxKind::TYPE_REF,
            SyntaxKind::MULTIPLICITY,
            SyntaxKind::VARIABLE_EXPR,
        ],
    );
    let parsed = parse(source);
    assert_eq!(
        count_kind(&parsed.green, SyntaxKind::TYPE_REF),
        GENERIC_TYPE_REFERENCE_COUNT
    );
}

#[test]
fn column_bodies_accept_each_lambda_form() {
    for source in [
        "~braced:{item| $item}",
        "~parameterless:| $item",
        "~short:item| $item",
    ] {
        assert_valid(
            source,
            &[
                SyntaxKind::COLUMN_SPEC,
                SyntaxKind::LAMBDA_EXPR,
                SyntaxKind::CODE_BLOCK,
            ],
        );
    }
}

#[test]
fn typed_columns_do_not_take_the_lambda_body_path() {
    let source = "~value:String[1]";
    let parsed = parse(source);

    assert_valid(
        source,
        &[
            SyntaxKind::COLUMN_SPEC,
            SyntaxKind::TYPE_REF,
            SyntaxKind::MULTIPLICITY,
        ],
    );
    assert!(!contains_kind(&parsed.green, SyntaxKind::LAMBDA_EXPR));
}

#[test]
fn incomplete_column_builders_fail_as_expressions() {
    for (source, expected_kind) in [
        ("~", SyntaxKind::COLUMN_SPEC),
        ("~[value:String", SyntaxKind::COLUMN_SPEC_ARRAY),
    ] {
        assert_outer_expression_failure(source, expected_kind);
    }
}

#[test]
fn incomplete_new_and_cast_expressions_fail_as_expressions() {
    for (source, expected_kind) in [
        ("^", SyntaxKind::NEW_INSTANCE_EXPR),
        ("@", SyntaxKind::CAST_EXPR),
        ("@<String>", SyntaxKind::CAST_EXPR),
    ] {
        assert_outer_expression_failure(source, expected_kind);
    }
}

#[test]
fn new_instance_calls_distinguish_positional_and_named_arguments() {
    let source = "^model::Person(friend, name = 'Ada')";

    assert_valid(
        source,
        &[
            SyntaxKind::NEW_INSTANCE_EXPR,
            SyntaxKind::CALL_ARGS,
            SyntaxKind::QUALIFIED_NAME,
            SyntaxKind::LITERAL_EXPR,
        ],
    );
}

#[test]
fn delimited_grammar_families_keep_each_member_and_terminator() {
    let top_level = parse("; first; second");

    assert_valid("(first, second)", &[SyntaxKind::PAREN_EXPR]);
    assert_valid(
        "{first, second| $first}",
        &[
            SyntaxKind::LAMBDA_EXPR,
            SyntaxKind::LAMBDA_PARAMS,
            SyntaxKind::CODE_BLOCK,
        ],
    );
    assert_valid(
        "{item| $item;}",
        &[
            SyntaxKind::LAMBDA_EXPR,
            SyntaxKind::LAMBDA_PARAMS,
            SyntaxKind::CODE_BLOCK,
        ],
    );
    assert_eq!(top_level.green.text(), "; first; second");
    assert!(
        top_level.diagnostics.is_empty(),
        "{:#?}",
        top_level.diagnostics
    );
    assert_eq!(count_kind(&top_level.green, SyntaxKind::QUERY_EXPR), 2);
}

#[test]
fn typed_short_lambda_lookahead_respects_nested_boundaries() {
    let valid = "item: Relation<(name:String[1])>| $item";

    assert_valid(
        valid,
        &[
            SyntaxKind::LAMBDA_EXPR,
            SyntaxKind::RELATION_TYPE,
            SyntaxKind::COLUMN_INFO,
        ],
    );
    for source in [
        "item: Type, other| $other",
        "item: Container<Inner|Other>",
        "item: Type)| $item",
        "item: Type]| $item",
    ] {
        let parsed = parse(source);

        assert_eq!(parsed.green.text(), source);
        assert!(
            !contains_kind(&parsed.green, SyntaxKind::LAMBDA_EXPR),
            "{source}"
        );
        assert!(
            syntax_error_count(&parsed) > 0,
            "{source}: {:#?}",
            parsed.diagnostics
        );
    }
}

#[test]
fn trivia_between_expression_terms_is_consumed_without_changing_the_tree() {
    assert_valid(
        "first /* inner */ + // line\n second",
        &[SyntaxKind::BINARY_EXPR, SyntaxKind::QUALIFIED_NAME],
    );
}
