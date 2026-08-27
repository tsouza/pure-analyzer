//! Lossless and typed-CST contracts for the M3 parser.

use pure_analyzer_diagnostics::FileId;
use pure_analyzer_lexer::lex;
use pure_analyzer_parser::parse_query;
use pure_analyzer_syntax::{AstNode, GreenElement, GreenNode, QueryExpression, SyntaxKind};

fn test_file() -> FileId {
    FileId::new(41)
}

fn parse(source: &str) -> pure_analyzer_parser::Parse {
    parse_query(source, test_file()).expect("representable source must build a tree")
}

fn contains_kind(node: &GreenNode, kind: SyntaxKind) -> bool {
    node.kind() == kind
        || node.children().iter().any(|element| match element {
            GreenElement::Node(child) => contains_kind(child, kind),
            GreenElement::Token(_) => false,
        })
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
