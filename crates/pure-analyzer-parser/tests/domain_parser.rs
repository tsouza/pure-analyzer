//! Contracts for the resilient Pure Domain parser.

use std::panic;

use proptest::prelude::*;
use pure_analyzer_diagnostics::{DiagCode, FileId, TextRange};
use pure_analyzer_lexer::lex;
use pure_analyzer_parser::{DomainCoverageGapKind, DomainParse, parse_domain};
use pure_analyzer_syntax::{
    AstNode, DomainClassDeclaration, GreenElement, GreenNode, QueryExpression, SyntaxKind,
};

const TEST_FILE: u32 = 118;

fn file() -> FileId {
    FileId::new(TEST_FILE)
}

fn parse(source: &str) -> DomainParse {
    parse_domain(source, file()).expect("representable test input must build a tree")
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

fn assert_range_is_valid(source: &str, range: TextRange) {
    let start = usize::from(range.start());
    let end = usize::from(range.end());
    assert!(start <= end);
    assert!(end <= source.len());
    assert!(source.is_char_boundary(start));
    assert!(source.is_char_boundary(end));
}

fn assert_lossless(source: &str, parsed: &DomainParse) {
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
fn parses_model_facts_with_domain_specific_ast_contracts() {
    let source = r#"
Class {meta::pure::profiles::doc.doc = 'generated'} demo::Person extends demo::Named, demo::Stamped
{
  id: Integer[1];
  manager: demo::Person[0..1];
  display(prefix: String[1], labels: List<String>[*]): String[1] { $this.name; };
}

Association demo::Employment
{
  employer: demo::Company[1];
  employee: demo::Person[*];
}

Profile demo::DataProfile
{
  stereotypes: [sensitive, pii];
  tags: [owner:String[1]];
}
"#;
    let parsed = parse(source);

    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    assert!(
        parsed.coverage_gaps.is_empty(),
        "{:#?}",
        parsed.coverage_gaps
    );
    for kind in [
        SyntaxKind::DOMAIN_FILE,
        SyntaxKind::DOMAIN_CLASS_DECL,
        SyntaxKind::DOMAIN_ASSOCIATION_DECL,
        SyntaxKind::DOMAIN_PROFILE_DECL,
        SyntaxKind::DOMAIN_STEREOTYPE_DECL,
        SyntaxKind::DOMAIN_STEREOTYPE_APPLICATIONS,
        SyntaxKind::DOMAIN_EXTENDS_CLAUSE,
        SyntaxKind::DOMAIN_PROPERTY_DECL,
        SyntaxKind::DOMAIN_QUALIFIED_PROPERTY_DECL,
        SyntaxKind::DOMAIN_PARAMETER_DECL,
        SyntaxKind::DOMAIN_TYPE_REF,
        SyntaxKind::DOMAIN_MULTIPLICITY,
        SyntaxKind::DOMAIN_QUALIFIED_NAME,
        SyntaxKind::DOMAIN_OPAQUE_BODY,
        SyntaxKind::DOMAIN_PROFILE_SECTION,
    ] {
        assert!(contains_kind(&parsed.green, kind), "missing {kind:?}");
    }

    let class = parsed
        .green
        .children()
        .iter()
        .filter_map(GreenElement::as_node)
        .flat_map(GreenNode::children)
        .filter_map(GreenElement::as_node)
        .find(|node| node.kind() == SyntaxKind::DOMAIN_CLASS_DECL)
        .expect("Domain file must contain the class declaration")
        .clone();
    let class = DomainClassDeclaration::cast(class).expect("class node has Domain AST wrapper");
    assert_eq!(class.text_range(), class.syntax().text_range());
    assert!(QueryExpression::cast(class.syntax().clone()).is_none());
    assert_lossless(source, &parsed);
}

#[test]
fn exotic_constructs_are_opaque_and_mark_precise_coverage_gaps() {
    let source = r#"
Enum demo::Status { OPEN, CLOSED }
Class demo::Known
{
  id: Integer[1];
  nativeThing foo;
  name: String[1];
}
"#;
    let parsed = parse(source);

    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    assert_eq!(
        parsed
            .coverage_gaps
            .iter()
            .map(|gap| gap.kind)
            .collect::<Vec<_>>(),
        vec![
            DomainCoverageGapKind::UnsupportedTopLevel,
            DomainCoverageGapKind::UnsupportedMember,
        ]
    );
    for gap in &parsed.coverage_gaps {
        assert_range_is_valid(source, gap.span);
        assert!(!source[usize::from(gap.span.start())..usize::from(gap.span.end())].is_empty());
    }
    assert!(contains_kind(&parsed.green, SyntaxKind::DOMAIN_OPAQUE_NODE));
    assert_eq!(count_kind(&parsed.green, SyntaxKind::DOMAIN_CLASS_DECL), 1);
    assert_eq!(
        count_kind(&parsed.green, SyntaxKind::DOMAIN_PROPERTY_DECL),
        2
    );
    assert_lossless(source, &parsed);
}

#[test]
fn malformed_declarations_keep_later_model_facts_and_stable_spans() {
    let source = r#"
Class broken::First
{
  bad: Integer[;
}
Class valid::After
{
  good: String[1];
}
"#;
    let parsed = panic::catch_unwind(|| parse(source)).expect("Domain parser must not panic");

    assert_eq!(count_kind(&parsed.green, SyntaxKind::DOMAIN_CLASS_DECL), 2);
    assert!(
        parsed
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == DiagCode::MalformedSyntax),
        "{:#?}",
        parsed.diagnostics
    );
    assert!(
        parsed
            .coverage_gaps
            .iter()
            .any(|gap| gap.kind == DomainCoverageGapKind::MalformedDeclaration)
    );
    for diagnostic in &parsed.diagnostics {
        assert_eq!(diagnostic.primary.file, file());
        assert_range_is_valid(source, diagnostic.primary.span);
    }
    for token in parsed.green.tokens() {
        assert_range_is_valid(source, token.text_range());
    }
    assert_lossless(source, &parsed);
}

#[test]
fn malformed_extends_clause_marks_the_class_without_losing_its_properties() {
    let source = r#"
Class demo::Broken extends
{
  first: String[1];
  second: Integer[1];
}
"#;
    let parsed = parse(source);

    assert_eq!(count_kind(&parsed.green, SyntaxKind::DOMAIN_CLASS_DECL), 1);
    assert_eq!(
        count_kind(&parsed.green, SyntaxKind::DOMAIN_PROPERTY_DECL),
        2,
        "a malformed extends clause must not swallow later class properties"
    );
    assert!(
        parsed
            .coverage_gaps
            .iter()
            .any(|gap| gap.kind == DomainCoverageGapKind::MalformedDeclaration),
        "the class needs conservative coverage when its inheritance is malformed: {:#?}",
        parsed.coverage_gaps
    );
    assert_lossless(source, &parsed);
}

#[test]
fn malformed_member_tail_preserves_following_property_and_qualified_property() {
    let source = r#"
Class demo::BrokenMembers
{
  bad: Foo good: String[1];
  alsoBad: Foo derived(): String[1] { $this.good; };
}
"#;
    let parsed = panic::catch_unwind(|| parse(source)).expect("Domain parser must not panic");

    assert_eq!(
        count_kind(&parsed.green, SyntaxKind::DOMAIN_PROPERTY_DECL),
        3,
        "the later property must be parsed instead of becoming recovery tail"
    );
    assert_eq!(
        count_kind(&parsed.green, SyntaxKind::DOMAIN_QUALIFIED_PROPERTY_DECL),
        1,
        "the later qualified property must be parsed instead of becoming recovery tail"
    );
    assert!(
        parsed
            .coverage_gaps
            .iter()
            .any(|gap| gap.kind == DomainCoverageGapKind::MalformedDeclaration)
    );
    assert_lossless(source, &parsed);
}

proptest! {
    #[test]
    fn arbitrary_domain_source_is_lossless_and_recovery_safe(source in ".{0,2048}") {
        let result = panic::catch_unwind(|| parse_domain(&source, file()));
        prop_assert!(result.is_ok());
        let parsed = result.expect("no panic").expect("small source must build a tree");
        prop_assert_eq!(parsed.green.text(), source.as_str());
        for token in parsed.green.tokens() {
            let start = usize::from(token.text_range().start());
            let end = usize::from(token.text_range().end());
            prop_assert!(start <= end && end <= source.len());
            prop_assert!(source.is_char_boundary(start) && source.is_char_boundary(end));
        }
        for diagnostic in &parsed.diagnostics {
            let start = usize::from(diagnostic.primary.span.start());
            let end = usize::from(diagnostic.primary.span.end());
            prop_assert_eq!(diagnostic.primary.file, file());
            prop_assert!(start <= end && end <= source.len());
            prop_assert!(source.is_char_boundary(start) && source.is_char_boundary(end));
        }
        for gap in &parsed.coverage_gaps {
            let start = usize::from(gap.span.start());
            let end = usize::from(gap.span.end());
            prop_assert!(start <= end && end <= source.len());
            prop_assert!(source.is_char_boundary(start) && source.is_char_boundary(end));
        }
    }
}
