//! Reviewed CST goldens for representative M3 source.

use std::fmt::Write;

use pure_analyzer_diagnostics::FileId;
use pure_analyzer_parser::parse_query;
use pure_analyzer_syntax::{GreenElement, GreenNode};

const INDENT: &str = "  ";
const COLLECTION_LITERAL_CST: &str =
    include_str!("snapshots/m3_golden__collection_literal_cst.snap");
const RELATION_PIPELINE_CST: &str = include_str!("snapshots/m3_golden__relation_pipeline_cst.snap");

fn test_file() -> FileId {
    FileId::new(97)
}

fn render_tree(node: &GreenNode, depth: usize, output: &mut String) {
    let indent = INDENT.repeat(depth);
    let _ = writeln!(output, "{indent}{:?} {:?}", node.kind(), node.text_range());
    for element in node.children() {
        match element {
            GreenElement::Node(child) => render_tree(child, depth.saturating_add(1), output),
            GreenElement::Token(token) => {
                let _ = writeln!(
                    output,
                    "{indent}{INDENT}{:?} {:?} {:?}",
                    token.kind(),
                    token.text_range(),
                    token.text()
                );
            }
        }
    }
}

fn golden_body(snapshot: &str) -> &str {
    match snapshot.rsplit_once("---\n") {
        Some((_, body)) => body,
        None => snapshot,
    }
}

#[test]
fn relation_pipeline_cst_is_stable() {
    let source = "#>{db::testDB.personTable}#\n  \
                  ->join(#>{db::testDB.groupMembershipTable}#, JoinKind.INNER, {x,y| $x.ID == $y.PERSONID})\n  \
                  ->extend(over(~GROUPID, ~SALARY->ascending()), ~[RANK:{p,w,r| $p->rank($w, $r)}]);";
    let parsed = parse_query(source, test_file()).expect("fixture must build a tree");
    let mut rendered = String::new();

    render_tree(&parsed.green, 0, &mut rendered);
    assert_eq!(rendered, golden_body(RELATION_PIPELINE_CST));
}

#[test]
fn collection_literal_cst_is_stable() {
    let source = "[a, [b]][0]";
    let parsed = parse_query(source, test_file()).expect("fixture must build a tree");
    let mut rendered = String::new();

    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    render_tree(&parsed.green, 0, &mut rendered);
    assert_eq!(rendered, golden_body(COLLECTION_LITERAL_CST));
}
