//! Lossless and typed-CST contracts for the M3 parser.

use pure_analyzer_diagnostics::FileId;
use pure_analyzer_lexer::lex;
use pure_analyzer_parser::parse_query;
use pure_analyzer_syntax::{
    AstNode, CollectionLiteral, ColumnName, GreenElement, GreenNode, QueryExpression, SyntaxKind,
};

fn test_file() -> FileId {
    FileId::new(41)
}

fn parse(source: &str) -> pure_analyzer_parser::Parse {
    parse_query(source, test_file()).expect("representable source must build a tree")
}

fn contains_kind(node: &GreenNode, kind: SyntaxKind) -> bool {
    find_node(node, kind).is_some()
}

fn find_node(node: &GreenNode, kind: SyntaxKind) -> Option<&GreenNode> {
    (node.kind() == kind).then_some(node).or_else(|| {
        node.children()
            .iter()
            .filter_map(GreenElement::as_node)
            .find_map(|child| find_node(child, kind))
    })
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

fn assert_lossless(source: &str) {
    let parsed = parse(source);
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

fn assert_valid_cst(source: &str, expected_kinds: &[SyntaxKind]) {
    let parsed = parse(source);

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
    assert_lossless(source);
}

#[test]
fn parses_the_relation_query_fixture_losslessly() {
    let source = "#>{db::testDB.personTable}#\n  \
                  ->join(#>{db::testDB.groupMembershipTable}#, JoinKind.INNER, {x,y| $x.ID == $y.PERSONID})\n  \
                  ->extend(over(~GROUPID, ~SALARY->ascending()), ~[RANK:{p,w,r| $p->rank($w, $r)}]);";
    let parsed = parse(source);

    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    assert_eq!(parsed.green.text(), source);
    assert!(contains_kind(
        &parsed.green,
        SyntaxKind::STORE_TABLE_POINTER
    ));
    assert!(contains_kind(&parsed.green, SyntaxKind::LAMBDA_EXPR));
    assert!(contains_kind(&parsed.green, SyntaxKind::COLUMN_SPEC_ARRAY));
    assert_lossless(source);
}

#[test]
fn parses_each_supported_primary_and_postfix_family() {
    let samples = [
        "model::Person.all()->filter(x|$x.name == 'Ada')",
        "model::Person.allVersions",
        "model::Person.allVersionsInRange(%2020-01-01, $date)",
        "model::Person.all()->filter(x: model::Person[1]| $x.name == 'Ada')",
        "model::Person.all()->filter(x: Relation<(name:String[1], rank:Integer[0..1])>| $x)",
        "|$x",
        "~out:|$x",
        "~[name:String[0..1], rank:{x| $x.rank()}]",
        "^model::Person(name='Ada')->toBytes()",
        "@meta::pure::metamodel::type::String",
        "#/model::Class/property#",
        "#{TDS data}#",
        "#{outer #{inner}# tail}#",
        "#{#>{db::testDB.personTable}#}#",
        "#{ { value } }#",
        "#TDS data#",
        "$record['name'][0] + 2 * 3",
    ];

    for source in samples {
        let parsed = parse(source);
        assert!(
            parsed.diagnostics.is_empty(),
            "{source}: {:#?}",
            parsed.diagnostics
        );
        assert_lossless(source);
    }
}

#[test]
fn distinguishes_all_and_parenthesized_expression_nodes() {
    for (source, expected) in [
        ("model::Person.all()", SyntaxKind::ALL_EXPR),
        ("(a + b)", SyntaxKind::PAREN_EXPR),
    ] {
        let parsed = parse(source);

        assert!(
            parsed.diagnostics.is_empty(),
            "{source}: {:#?}",
            parsed.diagnostics
        );
        assert!(contains_kind(&parsed.green, expected), "{source}");
        assert_lossless(source);
    }
}

#[test]
fn parses_collection_literals_in_relation_expressions() {
    for source in [
        "model::Person.all()->sort([~name->ascending(), ~age->descending()])",
        "[[], [$x, 2]][0]",
        "f([a, b], [])",
    ] {
        let parsed = parse(source);

        assert!(
            parsed.diagnostics.is_empty(),
            "{source}: {:#?}",
            parsed.diagnostics
        );
        assert!(contains_kind(&parsed.green, SyntaxKind::COLLECTION_LITERAL));
        assert_lossless(source);
    }

    let parsed = parse("f([a, b])");
    let collection = find_node(&parsed.green, SyntaxKind::COLLECTION_LITERAL)
        .and_then(|node| CollectionLiteral::cast(node.clone()))
        .expect("call argument should be a typed collection literal");

    assert_eq!(collection.syntax().text(), "[a, b]");
    assert_eq!(collection.text_range(), collection.syntax().text_range());
}

#[test]
fn quoted_column_aliases_are_typed_and_lossless() {
    let source = "~[ 'Total Revenue' : x | $x.amount, plain: String[1] ]";
    let parsed = parse(source);
    let mut names = Vec::new();

    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    nodes_with_kind(&parsed.green, SyntaxKind::COLUMN_NAME, &mut names);
    assert_eq!(names.len(), 2, "{names:#?}");

    let quoted = ColumnName::cast(names[0].clone()).expect("column name should have a typed view");
    assert_eq!(quoted.syntax().text(), "'Total Revenue'");
    assert_eq!(quoted.text_range(), quoted.syntax().text_range());
    assert_lossless(source);
}

#[test]
fn keeps_expression_grammar_families_structurally_distinct() {
    for (source, expected_kinds) in [
        (
            "model::Person.all()->filter(x| $x.name == 'Ada')",
            &[
                SyntaxKind::ALL_EXPR,
                SyntaxKind::ARROW_CALL,
                SyntaxKind::CALL_ARGS,
                SyntaxKind::LAMBDA_EXPR,
                SyntaxKind::LAMBDA_PARAMS,
                SyntaxKind::CODE_BLOCK,
                SyntaxKind::PROPERTY_NAV,
                SyntaxKind::BINARY_EXPR,
            ][..],
        ),
        (
            "model::Person.allVersionsInRange(%2020-01-01, $date)",
            &[
                SyntaxKind::ALL_EXPR,
                SyntaxKind::CALL_ARGS,
                SyntaxKind::LITERAL_EXPR,
                SyntaxKind::VARIABLE_EXPR,
            ][..],
        ),
        (
            "$record['name'][0] + 2 * 3",
            &[
                SyntaxKind::VARIABLE_EXPR,
                SyntaxKind::BRACKET_INDEX,
                SyntaxKind::LITERAL_EXPR,
                SyntaxKind::BINARY_EXPR,
            ][..],
        ),
        (
            "f((a + b), 2)",
            &[
                SyntaxKind::FUNCTION_CALL,
                SyntaxKind::CALL_ARGS,
                SyntaxKind::PAREN_EXPR,
                SyntaxKind::BINARY_EXPR,
                SyntaxKind::LITERAL_EXPR,
            ][..],
        ),
        (
            "(-value)",
            &[
                SyntaxKind::PAREN_EXPR,
                SyntaxKind::UNARY_EXPR,
                SyntaxKind::QUALIFIED_NAME,
            ][..],
        ),
    ] {
        assert_valid_cst(source, expected_kinds);
    }
}

#[test]
fn keeps_lambda_and_construct_grammar_families_structurally_distinct() {
    for (source, expected_kinds) in [
        (
            "{x,y| let z = $x; $z.name}",
            &[
                SyntaxKind::LAMBDA_EXPR,
                SyntaxKind::LAMBDA_PARAMS,
                SyntaxKind::LET_STMT,
                SyntaxKind::CODE_BLOCK,
                SyntaxKind::VARIABLE_EXPR,
                SyntaxKind::PROPERTY_NAV,
            ][..],
        ),
        (
            "x: Relation<(name:String[1], rank:Integer[0..1])>| $x",
            &[
                SyntaxKind::LAMBDA_EXPR,
                SyntaxKind::LAMBDA_PARAMS,
                SyntaxKind::TYPE_REF,
                SyntaxKind::RELATION_TYPE,
                SyntaxKind::COLUMN_INFO,
                SyntaxKind::MULTIPLICITY,
            ][..],
        ),
        (
            "~[rank:Integer[0..1], output:{x| $x}]",
            &[
                SyntaxKind::COLUMN_SPEC_ARRAY,
                SyntaxKind::COLUMN_SPEC,
                SyntaxKind::TYPE_REF,
                SyntaxKind::MULTIPLICITY,
                SyntaxKind::LAMBDA_EXPR,
            ][..],
        ),
        (
            "~out: x| $x",
            &[
                SyntaxKind::COLUMN_SPEC,
                SyntaxKind::LAMBDA_EXPR,
                SyntaxKind::LAMBDA_PARAMS,
                SyntaxKind::CODE_BLOCK,
            ][..],
        ),
        (
            "^model::Person('Ada')",
            &[
                SyntaxKind::NEW_INSTANCE_EXPR,
                SyntaxKind::QUALIFIED_NAME,
                SyntaxKind::CALL_ARGS,
                SyntaxKind::LITERAL_EXPR,
            ][..],
        ),
        (
            "^$person(name='Ada')",
            &[
                SyntaxKind::NEW_INSTANCE_EXPR,
                SyntaxKind::VARIABLE_EXPR,
                SyntaxKind::CALL_ARGS,
                SyntaxKind::LITERAL_EXPR,
            ][..],
        ),
        (
            "@meta::pure::String",
            &[
                SyntaxKind::CAST_EXPR,
                SyntaxKind::TYPE_REF,
                SyntaxKind::QUALIFIED_NAME,
            ][..],
        ),
    ] {
        assert_valid_cst(source, expected_kinds);
    }
}

#[test]
fn keeps_island_grammar_families_structurally_distinct() {
    for (source, expected_kinds) in [
        (
            "#>{db::testDB.personTable}#",
            &[SyntaxKind::ISLAND, SyntaxKind::STORE_TABLE_POINTER][..],
        ),
        (
            "#/model::Class/property#",
            &[SyntaxKind::ISLAND, SyntaxKind::NAV_PATH_ISLAND][..],
        ),
        (
            "#{outer #{inner}# tail}#",
            &[SyntaxKind::ISLAND, SyntaxKind::OPAQUE_ISLAND][..],
        ),
        (
            "#{ { value } }#",
            &[SyntaxKind::ISLAND, SyntaxKind::OPAQUE_ISLAND][..],
        ),
        (
            "#TDS data#",
            &[SyntaxKind::ISLAND, SyntaxKind::OPAQUE_ISLAND][..],
        ),
    ] {
        assert_valid_cst(source, expected_kinds);
    }
}

#[test]
fn preserves_binary_precedence_when_the_right_hand_side_is_stronger() {
    let parsed = parse("a == b + c");
    let mut binaries = Vec::new();

    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    nodes_with_kind(&parsed.green, SyntaxKind::BINARY_EXPR, &mut binaries);
    assert_eq!(binaries.len(), 2);
    assert_eq!(binaries[0].text(), "a == b + c");
    assert_eq!(binaries[1].text(), " b + c");
    assert_lossless("a == b + c");
}

#[test]
fn preserves_valid_property_call_surfaces_for_later_resolution() {
    for source in [
        "model::Person.all().p(%latest)",
        "model::Person.all().p($date)",
        "model::Person.all().p(%2020-01-01)",
        "model::Person.all().p(25)",
        "model::Person.all().p()",
    ] {
        let parsed = parse(source);
        assert!(
            parsed.diagnostics.is_empty(),
            "{source}: {:#?}",
            parsed.diagnostics
        );
        assert_lossless(source);
    }
}

#[test]
fn keeps_code_block_statements_and_typed_spans() {
    let source = "model::Person.all()->filter({x| let y = $x.manager; $y.name == 'Ada'})";
    let parsed = parse(source);
    let query = parsed
        .green
        .children()
        .iter()
        .find_map(GreenElement::as_node)
        .and_then(|node| QueryExpression::cast(node.clone()))
        .expect("root should contain a typed query expression");

    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    assert_eq!(query.text_range(), parsed.green.text_range());
    assert!(contains_kind(&parsed.green, SyntaxKind::LET_STMT));
    assert!(contains_kind(&parsed.green, SyntaxKind::CODE_BLOCK));
    assert_lossless(source);
}

#[test]
fn tree_owns_its_shared_source_after_input_is_dropped() {
    let tree = {
        let source = String::from("model::Person.all() // trailing");
        parse(&source).green
    };

    assert_eq!(tree.text(), "model::Person.all() // trailing");
}
