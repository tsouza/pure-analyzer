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

fn assert_only_malformed_declaration(source: &str) {
    let parsed = parse(source);
    assert_eq!(
        parsed
            .coverage_gaps
            .iter()
            .map(|gap| gap.kind)
            .collect::<Vec<_>>(),
        vec![DomainCoverageGapKind::MalformedDeclaration],
        "malformed declaration must conservatively mark coverage: {:#?}",
        parsed.coverage_gaps
    );
    assert_lossless(source, &parsed);
}

fn gap_texts<'source>(source: &'source str, parsed: &DomainParse) -> Vec<&'source str> {
    parsed
        .coverage_gaps
        .iter()
        .map(|gap| &source[usize::from(gap.span.start())..usize::from(gap.span.end())])
        .collect()
}

#[test]
fn empty_domain_file_is_lossless_and_diagnostic_free() {
    let parsed = parse("");

    assert_lossless("", &parsed);
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    assert!(
        parsed.coverage_gaps.is_empty(),
        "{:#?}",
        parsed.coverage_gaps
    );
}

#[test]
fn nonempty_domain_file_requires_token_progress() {
    let source = "\nClass demo::C {}\n";
    let parsed = parse(source);

    assert_lossless(source, &parsed);
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    assert!(
        parsed.coverage_gaps.is_empty(),
        "{:#?}",
        parsed.coverage_gaps
    );
    assert_eq!(count_kind(&parsed.green, SyntaxKind::DOMAIN_CLASS_DECL), 1);
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
fn class_header_missing_its_name_marks_malformed_coverage() {
    assert_only_malformed_declaration(
        r#"
Class
{
  id: Integer[1];
}
"#,
    );
}

#[test]
fn association_header_missing_its_name_marks_malformed_coverage() {
    assert_only_malformed_declaration(
        r#"
Association
{
  left: demo::Left[1];
  right: demo::Right[1];
}
"#,
    );
}

#[test]
fn profile_header_missing_its_name_marks_malformed_coverage() {
    assert_only_malformed_declaration(
        r#"
Profile
{
  stereotypes: [sensitive];
}
"#,
    );
}

#[test]
fn malformed_profile_section_marks_conservative_coverage() {
    let source = r#"
Profile demo::P
{
  stereotypes: [one two];
}
"#;
    let parsed = parse(source);

    assert!(
        parsed
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == DiagCode::MalformedSyntax),
        "{:#?}",
        parsed.diagnostics
    );
    assert_eq!(
        parsed
            .coverage_gaps
            .iter()
            .map(|gap| gap.kind)
            .collect::<Vec<_>>(),
        vec![DomainCoverageGapKind::MalformedDeclaration],
        "a malformed profile section must conservatively mark coverage: {:#?}",
        parsed.coverage_gaps
    );
    assert_eq!(
        count_kind(&parsed.green, SyntaxKind::DOMAIN_PROFILE_SECTION),
        1
    );
    assert_lossless(source, &parsed);
}

#[test]
fn malformed_stereotype_and_tag_applications_mark_conservative_coverage() {
    for source in [
        r#"
Class {meta::pure::profiles::temporal.bitemporal} demo::Bad
{
  value: String[1];
}
"#,
        r#"
Class <<temporal.bitemporal unexpected>> demo::Bad
{
  value: String[1];
}
"#,
    ] {
        let parsed = parse(source);

        assert!(
            parsed
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == DiagCode::MalformedSyntax),
            "{:#?}",
            parsed.diagnostics
        );
        assert_eq!(
            parsed
                .coverage_gaps
                .iter()
                .map(|gap| gap.kind)
                .collect::<Vec<_>>(),
            vec![DomainCoverageGapKind::MalformedDeclaration],
            "malformed annotation must conservatively mark coverage: {:#?}",
            parsed.coverage_gaps
        );
        assert_eq!(
            count_kind(&parsed.green, SyntaxKind::DOMAIN_STEREOTYPE_APPLICATIONS),
            0
        );
        assert!(contains_kind(&parsed.green, SyntaxKind::ERROR_NODE));
        assert_lossless(source, &parsed);
    }
}

#[test]
fn qualified_property_missing_return_colon_marks_malformed_coverage() {
    assert_only_malformed_declaration(
        r#"
Class demo::Broken
{
  derived() String[1] { $this; }
}
"#,
    );
}

#[test]
fn qualified_property_missing_return_type_marks_malformed_coverage() {
    assert_only_malformed_declaration(
        r#"
Class demo::Broken
{
  derived(): [1] { $this; }
}
"#,
    );
}

#[test]
fn qualified_property_missing_return_multiplicity_marks_malformed_coverage() {
    assert_only_malformed_declaration(
        r#"
Class demo::Broken
{
  derived(): String { $this; }
}
"#,
    );
}

#[test]
fn qualified_property_missing_body_marks_malformed_coverage() {
    assert_only_malformed_declaration(
        r#"
Class demo::Broken
{
  derived(): String[1];
}
"#,
    );
}

#[test]
fn qualified_property_malformed_parameter_marks_malformed_coverage() {
    assert_only_malformed_declaration(
        r#"
Class demo::Broken
{
  derived(value String[1]): String[1] { $this; }
}
"#,
    );
}

#[test]
fn coverage_gaps_follow_source_order_when_malformed_extends_contains_an_opaque_member() {
    let source = r#"
Class demo::Broken extends
{
  nativeThing foo;
}
"#;
    let parsed = parse(source);

    assert_eq!(
        parsed
            .coverage_gaps
            .iter()
            .map(|gap| gap.kind)
            .collect::<Vec<_>>(),
        vec![
            DomainCoverageGapKind::MalformedDeclaration,
            DomainCoverageGapKind::UnsupportedMember,
        ],
        "outer malformed declarations must precede later body coverage gaps"
    );
    assert!(
        parsed
            .coverage_gaps
            .windows(2)
            .all(|gaps| gaps[0].span.start() <= gaps[1].span.start()),
        "coverage gaps must be ordered by source span: {:#?}",
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

#[test]
fn nested_opaque_regions_stop_at_their_real_declaration_and_member_boundaries() {
    let source = r#"
Enum demo::Ignored
{
  value: { nested: [one, two]; };
}
Class demo::Known
{
  nativeThing foo({ nested: [a, b] });
  kept: String[1];
}
"#;
    let parsed = parse(source);

    assert_eq!(
        parsed
            .coverage_gaps
            .iter()
            .map(|gap| gap.kind)
            .collect::<Vec<_>>(),
        vec![
            DomainCoverageGapKind::UnsupportedTopLevel,
            DomainCoverageGapKind::UnsupportedMember,
        ],
        "{:#?}",
        gap_texts(source, &parsed),
    );
    let gaps = gap_texts(source, &parsed);
    assert!(gaps[0].trim_end().ends_with('}'));
    assert!(gaps[0].contains("nested: [one, two]"));
    assert!(gaps[1].contains("foo({ nested: [a, b] })"));
    assert_eq!(count_kind(&parsed.green, SyntaxKind::DOMAIN_CLASS_DECL), 1);
    assert_eq!(
        count_kind(&parsed.green, SyntaxKind::DOMAIN_PROPERTY_DECL),
        1
    );
    assert_eq!(count_kind(&parsed.green, SyntaxKind::DOMAIN_OPAQUE_NODE), 2);
    assert_lossless(source, &parsed);
}

#[test]
fn generic_types_leading_paths_and_double_angle_stereotypes_are_distinct_contracts() {
    let source = r#"
Class <<meta::tag>> ::demo::model::Thing extends ::demo::Base , other::Stamped
{
  value: Map<::demo::Key, List<::demo::Value>>[0..*];
}
"#;
    let parsed = parse(source);

    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    assert!(
        parsed.coverage_gaps.is_empty(),
        "{:#?}",
        parsed.coverage_gaps
    );
    assert_eq!(
        count_kind(&parsed.green, SyntaxKind::DOMAIN_STEREOTYPE_APPLICATIONS),
        1
    );
    assert_eq!(
        count_kind(&parsed.green, SyntaxKind::DOMAIN_EXTENDS_CLAUSE),
        1
    );
    assert_eq!(count_kind(&parsed.green, SyntaxKind::DOMAIN_TYPE_REF), 4);
    assert_eq!(
        count_kind(&parsed.green, SyntaxKind::DOMAIN_MULTIPLICITY),
        1
    );
    assert_lossless(source, &parsed);
}

#[test]
fn profile_tag_lists_keep_nested_multiplicities_before_later_sections() {
    let source = r#"
Profile demo::Annotated
{
  tags: [owner: Map<String, List<demo::Owner>>[1]];
  stereotypes: [internal, pii];
}
"#;
    let parsed = parse(source);

    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    assert!(
        parsed.coverage_gaps.is_empty(),
        "{:#?}",
        parsed.coverage_gaps
    );
    assert_eq!(
        count_kind(&parsed.green, SyntaxKind::DOMAIN_PROFILE_SECTION),
        2
    );
    assert_eq!(
        count_kind(&parsed.green, SyntaxKind::DOMAIN_STEREOTYPE_DECL),
        2
    );
    assert_lossless(source, &parsed);
}

#[test]
fn opaque_initializers_with_nested_delimiters_leave_following_properties_intact() {
    let source = r#"
Class demo::Computed
{
  first: String[1] = call({ nested: [one, two] }, [left, right]);
  second: Integer[1];
}
"#;
    let parsed = parse(source);

    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    assert!(
        parsed.coverage_gaps.is_empty(),
        "{:#?}",
        parsed.coverage_gaps
    );
    assert_eq!(
        count_kind(&parsed.green, SyntaxKind::DOMAIN_PROPERTY_DECL),
        2
    );
    assert_eq!(count_kind(&parsed.green, SyntaxKind::DOMAIN_OPAQUE_BODY), 1);
    assert_lossless(source, &parsed);
}

#[test]
fn missing_declaration_body_recovers_at_the_next_declaration() {
    let source = r#"
Class demo::MissingBody;;
Class demo::After
{
  kept: String[1];
}
"#;
    let parsed = parse(source);

    assert_eq!(
        parsed.diagnostics.len(),
        1,
        "header recovery must not create a second no-progress diagnostic: {:#?}",
        parsed.diagnostics
    );
    assert_eq!(count_kind(&parsed.green, SyntaxKind::DOMAIN_CLASS_DECL), 2);
    assert_eq!(
        count_kind(&parsed.green, SyntaxKind::DOMAIN_PROPERTY_DECL),
        1
    );
    assert!(
        parsed
            .coverage_gaps
            .iter()
            .any(|gap| gap.kind == DomainCoverageGapKind::MalformedDeclaration),
        "{:#?}",
        parsed.coverage_gaps
    );
    assert_lossless(source, &parsed);
}

#[test]
fn eof_diagnostics_anchor_at_the_end_of_domain_source() {
    let source = "Class demo::Unfinished";
    let parsed = parse(source);
    let eof = source.len();

    assert!(
        parsed.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == DiagCode::MalformedSyntax
                && usize::from(diagnostic.primary.span.start()) == eof
                && usize::from(diagnostic.primary.span.end()) == eof
        }),
        "missing-body diagnostic must be anchored at EOF: {:#?}",
        parsed.diagnostics
    );
    assert_lossless(source, &parsed);
}

#[test]
fn token_diagnostics_anchor_at_the_current_domain_token() {
    let source = "Class : {}";
    let parsed = parse(source);
    let start = source.find(':').expect("unexpected token");
    let end = start + ':'.len_utf8();

    assert!(
        parsed.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == DiagCode::MalformedSyntax
                && usize::from(diagnostic.primary.span.start()) == start
                && usize::from(diagnostic.primary.span.end()) == end
        }),
        "missing-name diagnostic must be anchored at the unexpected token: {:#?}",
        parsed.diagnostics
    );
    assert_lossless(source, &parsed);
}

#[test]
fn malformed_parameter_tail_does_not_swallow_a_following_property() {
    let source = r#"
Class demo::BrokenParameters
{
  derived(value: String[1],): String[1] { $this; };
  kept: Integer[1];
}
"#;
    let parsed = parse(source);

    assert_eq!(
        count_kind(&parsed.green, SyntaxKind::DOMAIN_QUALIFIED_PROPERTY_DECL),
        1
    );
    assert_eq!(
        count_kind(&parsed.green, SyntaxKind::DOMAIN_PROPERTY_DECL),
        1
    );
    assert!(
        parsed
            .coverage_gaps
            .iter()
            .any(|gap| gap.kind == DomainCoverageGapKind::MalformedDeclaration),
        "{:#?}",
        parsed.coverage_gaps
    );
    assert_lossless(source, &parsed);
}

#[test]
fn lexer_errors_are_diagnosed_without_losing_later_supported_members() {
    let source = r#"
Class demo::BadToken
{
  before: String[1];
  \u{0}
  after: Integer[1];
}
"#;
    let parsed = parse(source);

    assert!(
        parsed
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == DiagCode::BadToken),
        "{:#?}",
        parsed.diagnostics
    );
    assert_eq!(
        count_kind(&parsed.green, SyntaxKind::DOMAIN_PROPERTY_DECL),
        2
    );
    assert_lossless(source, &parsed);
}

#[test]
fn type_nesting_limit_recovers_without_losing_the_input() {
    let nested = format!("{}String{}", "List<".repeat(257), ">".repeat(257));
    let source = format!("Class demo::Deep {{ value: {nested}[1]; }}");
    let parsed = panic::catch_unwind(|| parse(&source)).expect("depth recovery must not panic");

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
            .any(|gap| gap.kind == DomainCoverageGapKind::MalformedDeclaration),
        "{:#?}",
        parsed.coverage_gaps
    );
    assert_lossless(&source, &parsed);
}

#[test]
fn sequential_type_references_do_not_consume_the_nesting_budget() {
    let members = (0..257)
        .map(|index| format!("  value{index}: String[1];"))
        .collect::<Vec<_>>()
        .join("\n");
    let source = format!("Class demo::FlatTypes {{\n{members}\n}}");
    let parsed = parse(&source);

    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    assert!(
        parsed.coverage_gaps.is_empty(),
        "{:#?}",
        parsed.coverage_gaps
    );
    assert_eq!(
        count_kind(&parsed.green, SyntaxKind::DOMAIN_PROPERTY_DECL),
        257
    );
    assert_eq!(count_kind(&parsed.green, SyntaxKind::DOMAIN_TYPE_REF), 257);
    assert_lossless(&source, &parsed);
}

#[test]
fn dangling_qualified_name_marks_the_declaration_malformed() {
    let source = r#"
Class demo::
{
  kept: String[1];
}
"#;
    let parsed = parse(source);

    assert_eq!(count_kind(&parsed.green, SyntaxKind::DOMAIN_CLASS_DECL), 1);
    assert_eq!(
        count_kind(&parsed.green, SyntaxKind::DOMAIN_PROPERTY_DECL),
        1
    );
    assert_eq!(
        parsed
            .coverage_gaps
            .iter()
            .map(|gap| gap.kind)
            .collect::<Vec<_>>(),
        vec![DomainCoverageGapKind::MalformedDeclaration],
        "{:#?}",
        parsed.coverage_gaps
    );
    assert_lossless(source, &parsed);
}

#[test]
fn opaque_top_level_stops_before_a_supported_declaration_without_a_terminator() {
    let source = r#"
Enum demo::Skipped
Class demo::Kept
{
  value: String[1];
}
"#;
    let parsed = parse(source);

    assert_eq!(count_kind(&parsed.green, SyntaxKind::DOMAIN_CLASS_DECL), 1);
    assert_eq!(
        count_kind(&parsed.green, SyntaxKind::DOMAIN_PROPERTY_DECL),
        1
    );
    assert_eq!(
        parsed
            .coverage_gaps
            .iter()
            .map(|gap| gap.kind)
            .collect::<Vec<_>>(),
        vec![DomainCoverageGapKind::UnsupportedTopLevel],
        "{:#?}",
        gap_texts(source, &parsed)
    );
    assert_lossless(source, &parsed);
}

#[test]
fn double_angle_annotations_must_precede_the_declaration_name() {
    let source = r#"
Class Known <<meta::tag>>
{
  value: String[1];
}
"#;
    let parsed = parse(source);

    assert_eq!(
        count_kind(&parsed.green, SyntaxKind::DOMAIN_STEREOTYPE_APPLICATIONS),
        0,
        "a later `<<...>>` must not be reclassified as a declaration annotation"
    );
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
            .any(|gap| gap.kind == DomainCoverageGapKind::MalformedDeclaration),
        "{:#?}",
        parsed.coverage_gaps
    );
    assert_lossless(source, &parsed);
}

#[test]
fn opaque_member_without_a_terminator_stops_before_a_supported_property() {
    let source = r#"
Class demo::Known
{
  nativeThing foo
  kept: String[1];
}
"#;
    let parsed = parse(source);

    assert_eq!(
        count_kind(&parsed.green, SyntaxKind::DOMAIN_PROPERTY_DECL),
        1
    );
    assert_eq!(
        parsed
            .coverage_gaps
            .iter()
            .map(|gap| gap.kind)
            .collect::<Vec<_>>(),
        vec![DomainCoverageGapKind::UnsupportedMember],
        "{:#?}",
        gap_texts(source, &parsed)
    );
    assert_eq!(gap_texts(source, &parsed)[0].trim(), "nativeThing foo");
    assert_lossless(source, &parsed);
}

#[test]
fn opaque_member_with_a_top_level_colon_stops_before_a_qualified_property() {
    let source = r#"
Class demo::Known
{
  nativeThing [legacy]: String[1]
  derived(value: String[1]): String[1] { $this; };
  kept: Boolean[1];
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
        vec![DomainCoverageGapKind::UnsupportedMember],
        "the opaque prefix must not swallow a later qualified property: {:#?}",
        parsed.coverage_gaps
    );
    assert_eq!(
        count_kind(&parsed.green, SyntaxKind::DOMAIN_QUALIFIED_PROPERTY_DECL),
        1
    );
    assert_eq!(
        count_kind(&parsed.green, SyntaxKind::DOMAIN_PARAMETER_DECL),
        1
    );
    assert_eq!(count_kind(&parsed.green, SyntaxKind::DOMAIN_OPAQUE_BODY), 1);
    assert_eq!(
        count_kind(&parsed.green, SyntaxKind::DOMAIN_PROPERTY_DECL),
        1
    );
    assert_lossless(source, &parsed);
}

#[test]
fn opaque_member_does_not_consume_a_tightly_following_property() {
    let source = r#"
Class demo::Known
{
  nativeThing foo;kept: String[1];
}
"#;
    let parsed = parse(source);

    assert_eq!(
        count_kind(&parsed.green, SyntaxKind::DOMAIN_PROPERTY_DECL),
        1
    );
    assert_eq!(
        parsed
            .coverage_gaps
            .iter()
            .map(|gap| gap.kind)
            .collect::<Vec<_>>(),
        vec![DomainCoverageGapKind::UnsupportedMember],
        "{:#?}",
        gap_texts(source, &parsed)
    );
    assert_eq!(gap_texts(source, &parsed)[0].trim(), "nativeThing foo;");
    assert_lossless(source, &parsed);
}

#[test]
fn declaration_header_recovery_consumes_junk_before_the_next_declaration() {
    let source = r#"
Class demo::Broken trailing header tokens
Class demo::After
{
  kept: String[1];
}
"#;
    let parsed = parse(source);

    assert_eq!(
        parsed.diagnostics.len(),
        1,
        "header recovery must not create a spurious no-progress diagnostic: {:#?}",
        parsed.diagnostics
    );
    assert_eq!(count_kind(&parsed.green, SyntaxKind::DOMAIN_CLASS_DECL), 2);
    assert_eq!(
        count_kind(&parsed.green, SyntaxKind::DOMAIN_PROPERTY_DECL),
        1
    );
    assert_eq!(
        parsed
            .coverage_gaps
            .iter()
            .map(|gap| gap.kind)
            .collect::<Vec<_>>(),
        vec![DomainCoverageGapKind::MalformedDeclaration],
        "{:#?}",
        gap_texts(source, &parsed)
    );
    assert_lossless(source, &parsed);
}

#[test]
fn declaration_header_recovery_leaves_a_stray_brace_before_the_next_declaration() {
    let source = r#"
Class demo::Broken trailing header tokens
}
Class demo::After
{
  kept: String[1];
}
"#;
    let parsed = parse(source);

    assert_eq!(count_kind(&parsed.green, SyntaxKind::DOMAIN_CLASS_DECL), 2);
    assert_eq!(
        count_kind(&parsed.green, SyntaxKind::DOMAIN_PROPERTY_DECL),
        1
    );
    assert_eq!(count_kind(&parsed.green, SyntaxKind::DOMAIN_OPAQUE_NODE), 1);
    assert_eq!(
        parsed
            .coverage_gaps
            .iter()
            .map(|gap| gap.kind)
            .collect::<Vec<_>>(),
        vec![
            DomainCoverageGapKind::MalformedDeclaration,
            DomainCoverageGapKind::UnsupportedTopLevel,
        ],
        "the header recovery must leave the brace for top-level recovery: {:#?}",
        parsed.coverage_gaps
    );
    assert_lossless(source, &parsed);
}

#[test]
fn member_tail_recovery_consumes_noise_before_a_property_boundary() {
    let source = r#"
Class demo::Broken
{
  broken: Foo junk more kept: String[1];
}
"#;
    let parsed = parse(source);

    assert_eq!(
        count_kind(&parsed.green, SyntaxKind::DOMAIN_PROPERTY_DECL),
        2
    );
    assert_eq!(
        parsed
            .coverage_gaps
            .iter()
            .map(|gap| gap.kind)
            .collect::<Vec<_>>(),
        vec![DomainCoverageGapKind::MalformedDeclaration],
        "{:#?}",
        gap_texts(source, &parsed)
    );
    assert_lossless(source, &parsed);
}

#[test]
fn unterminated_body_trailing_trivia_does_not_invent_an_opaque_member() {
    let source = "Class demo::Broken {\n  ";
    let parsed = parse(source);

    assert!(
        parsed
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == DiagCode::MalformedSyntax),
        "{:#?}",
        parsed.diagnostics
    );
    assert_eq!(count_kind(&parsed.green, SyntaxKind::DOMAIN_OPAQUE_NODE), 0);
    assert_eq!(
        parsed
            .coverage_gaps
            .iter()
            .map(|gap| gap.kind)
            .collect::<Vec<_>>(),
        vec![DomainCoverageGapKind::MalformedDeclaration],
        "{:#?}",
        parsed.coverage_gaps
    );
    assert_lossless(source, &parsed);
}

#[test]
fn valid_extends_prefix_does_not_hide_a_dangling_supertype() {
    let source = r#"
Class demo::Broken extends demo::Base,
{
  kept: String[1];
}
"#;
    let parsed = parse(source);

    assert_eq!(
        count_kind(&parsed.green, SyntaxKind::DOMAIN_PROPERTY_DECL),
        1
    );
    assert_eq!(
        parsed
            .coverage_gaps
            .iter()
            .map(|gap| gap.kind)
            .collect::<Vec<_>>(),
        vec![DomainCoverageGapKind::MalformedDeclaration],
        "a later malformed supertype cannot be hidden by a valid prefix: {:#?}",
        parsed.coverage_gaps
    );
    assert_lossless(source, &parsed);
}

#[test]
fn association_with_a_missing_name_keeps_its_known_body() {
    let source = r#"
Association ::
{
  left: demo::Left[1];
  right: demo::Right[1];
}
"#;
    let parsed = parse(source);

    assert_eq!(
        count_kind(&parsed.green, SyntaxKind::DOMAIN_ASSOCIATION_DECL),
        1
    );
    assert_eq!(
        count_kind(&parsed.green, SyntaxKind::DOMAIN_PROPERTY_DECL),
        2
    );
    assert_eq!(
        parsed
            .coverage_gaps
            .iter()
            .map(|gap| gap.kind)
            .collect::<Vec<_>>(),
        vec![DomainCoverageGapKind::MalformedDeclaration],
        "a valid association body cannot hide its missing declaration name: {:#?}",
        parsed.coverage_gaps
    );
    assert_lossless(source, &parsed);
}

#[test]
fn profile_with_a_missing_name_keeps_its_known_sections() {
    let source = r#"
Profile ::
{
  stereotypes: [sensitive];
}
"#;
    let parsed = parse(source);

    assert_eq!(
        count_kind(&parsed.green, SyntaxKind::DOMAIN_PROFILE_DECL),
        1
    );
    assert_eq!(
        count_kind(&parsed.green, SyntaxKind::DOMAIN_PROFILE_SECTION),
        1
    );
    assert_eq!(
        count_kind(&parsed.green, SyntaxKind::DOMAIN_STEREOTYPE_DECL),
        1
    );
    assert_eq!(
        parsed
            .coverage_gaps
            .iter()
            .map(|gap| gap.kind)
            .collect::<Vec<_>>(),
        vec![DomainCoverageGapKind::MalformedDeclaration],
        "a valid profile body cannot hide its missing declaration name: {:#?}",
        parsed.coverage_gaps
    );
    assert_lossless(source, &parsed);
}

#[test]
fn profile_sections_are_complete_and_lossless() {
    let source = r#"
Profile demo::First
{
  stereotypes: [sensitive];
}
"#;
    let parsed = parse(source);

    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    assert!(
        parsed.coverage_gaps.is_empty(),
        "{:#?}",
        parsed.coverage_gaps
    );
    assert_eq!(
        count_kind(&parsed.green, SyntaxKind::DOMAIN_PROFILE_DECL),
        1
    );
    assert_eq!(
        count_kind(&parsed.green, SyntaxKind::DOMAIN_PROFILE_SECTION),
        1
    );
    assert_eq!(
        count_kind(&parsed.green, SyntaxKind::DOMAIN_STEREOTYPE_DECL),
        1
    );
    assert_lossless(source, &parsed);
}

#[test]
fn opaque_profile_members_leave_later_sections_intact() {
    let source = r#"
Profile demo::Known
{
  nativeThing custom;
  stereotypes: [sensitive];
}
"#;
    let parsed = parse(source);

    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    assert_eq!(
        count_kind(&parsed.green, SyntaxKind::DOMAIN_PROFILE_DECL),
        1
    );
    assert_eq!(count_kind(&parsed.green, SyntaxKind::DOMAIN_OPAQUE_NODE), 1);
    assert_eq!(
        count_kind(&parsed.green, SyntaxKind::DOMAIN_PROFILE_SECTION),
        1
    );
    assert_eq!(
        count_kind(&parsed.green, SyntaxKind::DOMAIN_STEREOTYPE_DECL),
        1
    );
    assert_eq!(
        parsed
            .coverage_gaps
            .iter()
            .map(|gap| gap.kind)
            .collect::<Vec<_>>(),
        vec![DomainCoverageGapKind::UnsupportedMember],
        "the unsupported member must not suppress later profile facts: {:#?}",
        parsed.coverage_gaps
    );
    assert_lossless(source, &parsed);
}

#[test]
fn a_missing_property_terminator_marks_that_property_malformed() {
    let source = r#"
Class demo::Broken
{
  value: String[1]
}
"#;
    let parsed = parse(source);

    assert_eq!(
        count_kind(&parsed.green, SyntaxKind::DOMAIN_PROPERTY_DECL),
        1
    );
    assert_eq!(
        parsed
            .coverage_gaps
            .iter()
            .map(|gap| gap.kind)
            .collect::<Vec<_>>(),
        vec![DomainCoverageGapKind::MalformedDeclaration],
        "a property is only a model fact when every required component is present: {:#?}",
        parsed.coverage_gaps
    );
    assert_lossless(source, &parsed);
}

#[test]
fn member_tail_recovery_leaves_the_closing_brace_for_the_class_body() {
    let source = r#"
Class demo::Broken
{
  broken: Foo junk;
}
"#;
    let parsed = parse(source);

    assert_eq!(
        count_kind(&parsed.green, SyntaxKind::DOMAIN_PROPERTY_DECL),
        1
    );
    assert_eq!(
        parsed
            .coverage_gaps
            .iter()
            .map(|gap| gap.kind)
            .collect::<Vec<_>>(),
        vec![DomainCoverageGapKind::MalformedDeclaration],
        "recovery must keep the malformed member scoped to its semicolon: {:#?}",
        parsed.coverage_gaps
    );
    assert_eq!(
        gap_texts(source, &parsed)
            .iter()
            .map(|gap| gap.trim())
            .collect::<Vec<_>>(),
        vec!["broken: Foo junk;"]
    );
    assert_lossless(source, &parsed);
}

#[test]
fn member_tail_recovery_keeps_a_closing_brace_before_the_next_declaration() {
    let source = r#"
Class demo::Broken
{
  broken: Foo junk
}
Class demo::After
{
  kept: String[1];
}
"#;
    let parsed = parse(source);

    assert_eq!(count_kind(&parsed.green, SyntaxKind::DOMAIN_CLASS_DECL), 2);
    assert_eq!(
        count_kind(&parsed.green, SyntaxKind::DOMAIN_PROPERTY_DECL),
        2
    );
    assert_eq!(
        parsed
            .coverage_gaps
            .iter()
            .map(|gap| gap.kind)
            .collect::<Vec<_>>(),
        vec![DomainCoverageGapKind::MalformedDeclaration],
        "member recovery must leave the closing brace for the class body: {:#?}",
        parsed.coverage_gaps
    );
    assert_lossless(source, &parsed);
}

#[test]
fn top_level_semicolons_are_consumed_without_creating_an_opaque_declaration() {
    let source = r#"
;;
Class demo::After
{
  kept: String[1];
}
"#;
    let parsed = parse(source);

    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    assert!(
        parsed.coverage_gaps.is_empty(),
        "{:#?}",
        parsed.coverage_gaps
    );
    assert_eq!(count_kind(&parsed.green, SyntaxKind::DOMAIN_OPAQUE_NODE), 0);
    assert_eq!(count_kind(&parsed.green, SyntaxKind::DOMAIN_CLASS_DECL), 1);
    assert_eq!(
        count_kind(&parsed.green, SyntaxKind::DOMAIN_PROPERTY_DECL),
        1
    );
    assert_lossless(source, &parsed);
}

#[test]
fn declaration_body_semicolons_are_consumed_without_creating_an_opaque_member() {
    let source = r#"
Class demo::Empty
{
  ;;
  kept: String[1];
}
"#;
    let parsed = parse(source);

    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    assert!(
        parsed.coverage_gaps.is_empty(),
        "{:#?}",
        parsed.coverage_gaps
    );
    assert_eq!(count_kind(&parsed.green, SyntaxKind::DOMAIN_OPAQUE_NODE), 0);
    assert_eq!(count_kind(&parsed.green, SyntaxKind::DOMAIN_CLASS_DECL), 1);
    assert_eq!(
        count_kind(&parsed.green, SyntaxKind::DOMAIN_PROPERTY_DECL),
        1
    );
    assert_lossless(source, &parsed);
}

#[test]
fn malformed_parameter_tails_keep_the_qualified_property_body() {
    let source = r#"
Class demo::BrokenParameters
{
  derived(value: String[1] noise): String[1] { $this; };
  kept: Integer[1];
}
"#;
    let parsed = parse(source);

    assert_eq!(
        count_kind(&parsed.green, SyntaxKind::DOMAIN_QUALIFIED_PROPERTY_DECL),
        1
    );
    assert_eq!(
        count_kind(&parsed.green, SyntaxKind::DOMAIN_PROPERTY_DECL),
        1
    );
    assert_eq!(count_kind(&parsed.green, SyntaxKind::DOMAIN_OPAQUE_BODY), 1);
    assert_eq!(
        parsed
            .coverage_gaps
            .iter()
            .map(|gap| gap.kind)
            .collect::<Vec<_>>(),
        vec![DomainCoverageGapKind::MalformedDeclaration],
        "parameter recovery must not discard the known qualified-property body: {:#?}",
        parsed.coverage_gaps
    );
    assert_lossless(source, &parsed);
}

#[test]
fn parameter_recovery_advances_to_later_parameters() {
    let source = r#"
Class demo::RecoveredParameters
{
  derived(first: String[1] junk, second: Integer[1]): String[1] { $this; };
  kept: Boolean[1];
}
"#;
    let parsed = parse(source);

    assert!(
        parsed
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == DiagCode::MalformedSyntax),
        "{:#?}",
        parsed.diagnostics
    );
    assert_eq!(
        parsed
            .coverage_gaps
            .iter()
            .map(|gap| gap.kind)
            .collect::<Vec<_>>(),
        vec![DomainCoverageGapKind::MalformedDeclaration],
        "parameter recovery must keep the qualified-property conservatively covered: {:#?}",
        parsed.coverage_gaps
    );
    assert_eq!(
        count_kind(&parsed.green, SyntaxKind::DOMAIN_QUALIFIED_PROPERTY_DECL),
        1
    );
    assert_eq!(
        count_kind(&parsed.green, SyntaxKind::DOMAIN_PARAMETER_DECL),
        2
    );
    assert_eq!(
        count_kind(&parsed.green, SyntaxKind::DOMAIN_PROPERTY_DECL),
        1
    );
    assert_eq!(count_kind(&parsed.green, SyntaxKind::DOMAIN_OPAQUE_BODY), 1);
    assert_lossless(source, &parsed);
}

#[test]
fn generic_type_arguments_require_every_argument() {
    let source = r#"
Class demo::Broken
{
  value: List<, String>[1];
}
"#;
    let parsed = parse(source);

    assert_eq!(
        count_kind(&parsed.green, SyntaxKind::DOMAIN_PROPERTY_DECL),
        1
    );
    assert_eq!(
        parsed
            .coverage_gaps
            .iter()
            .map(|gap| gap.kind)
            .collect::<Vec<_>>(),
        vec![DomainCoverageGapKind::MalformedDeclaration],
        "one invalid type argument invalidates the containing model fact: {:#?}",
        parsed.coverage_gaps
    );
    assert_lossless(source, &parsed);
}

#[test]
fn generic_type_recovery_consumes_a_separator_before_later_facts() {
    let source = r#"
Class demo::Broken
{
  broken: List<, String junk more noise, Integer>[1];
  kept: Boolean[1];
}
"#;
    let parsed = panic::catch_unwind(|| parse(source)).expect("Domain parser must not panic");

    assert!(
        parsed
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == DiagCode::MalformedSyntax),
        "{:#?}",
        parsed.diagnostics
    );
    assert_eq!(
        count_kind(&parsed.green, SyntaxKind::DOMAIN_PROPERTY_DECL),
        2
    );
    assert_eq!(
        count_kind(&parsed.green, SyntaxKind::DOMAIN_TYPE_REF),
        5,
        "recovery must retain the later generic argument before the following property"
    );
    assert_eq!(
        parsed
            .coverage_gaps
            .iter()
            .map(|gap| gap.kind)
            .collect::<Vec<_>>(),
        vec![DomainCoverageGapKind::MalformedDeclaration],
        "generic recovery must preserve later model facts: {:#?}",
        parsed.coverage_gaps
    );
    assert_lossless(source, &parsed);
}

#[test]
fn generic_type_recovery_consumes_adjacent_recovery_commas() {
    let source = r#"
Class demo::Broken
{
  broken: List<String junk,, Integer>[1];
  kept: Boolean[1];
}
"#;
    let parsed = panic::catch_unwind(|| parse(source)).expect("Domain parser must not panic");

    assert!(
        parsed
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == DiagCode::MalformedSyntax),
        "{:#?}",
        parsed.diagnostics
    );
    assert_eq!(
        count_kind(&parsed.green, SyntaxKind::DOMAIN_PROPERTY_DECL),
        2
    );
    assert_eq!(
        count_kind(&parsed.green, SyntaxKind::DOMAIN_TYPE_REF),
        5,
        "recovery must retain the later type argument and following property"
    );
    assert_eq!(
        parsed
            .coverage_gaps
            .iter()
            .map(|gap| gap.kind)
            .collect::<Vec<_>>(),
        vec![DomainCoverageGapKind::MalformedDeclaration],
        "generic recovery must preserve later model facts: {:#?}",
        parsed.coverage_gaps
    );
    assert_lossless(source, &parsed);
}

#[test]
fn multiplicity_requires_a_lower_bound_after_its_opening_bracket() {
    let source = r#"
Class demo::Broken
{
  value: String[];
}
"#;
    let parsed = parse(source);

    assert_eq!(
        count_kind(&parsed.green, SyntaxKind::DOMAIN_MULTIPLICITY),
        1
    );
    assert_eq!(
        parsed
            .coverage_gaps
            .iter()
            .map(|gap| gap.kind)
            .collect::<Vec<_>>(),
        vec![DomainCoverageGapKind::MalformedDeclaration],
        "an opening multiplicity bracket alone is not sufficient: {:#?}",
        parsed.coverage_gaps
    );
    assert_lossless(source, &parsed);
}

#[test]
fn malformed_profile_headers_do_not_yield_stereotype_facts() {
    let source = r#"
Profile demo::Broken
{
  stereotypes [sensitive];
}
"#;
    let parsed = parse(source);

    assert_eq!(
        count_kind(&parsed.green, SyntaxKind::DOMAIN_STEREOTYPE_DECL),
        0,
        "a section without its required colon must not yield stereotype declarations"
    );
    assert!(
        parsed
            .coverage_gaps
            .iter()
            .any(|gap| gap.kind == DomainCoverageGapKind::MalformedDeclaration),
        "a profile section requires both its colon and opening bracket: {:#?}",
        parsed.coverage_gaps
    );
    assert_lossless(source, &parsed);
}

#[test]
fn a_missing_stereotype_name_invalidates_the_profile_section() {
    let source = r#"
Profile demo::Broken
{
  stereotypes: [first, , second];
}
"#;
    let parsed = parse(source);

    assert_eq!(
        parsed
            .coverage_gaps
            .iter()
            .map(|gap| gap.kind)
            .collect::<Vec<_>>(),
        vec![DomainCoverageGapKind::MalformedDeclaration],
        "a valid stereotype before an omitted one cannot make the section valid: {:#?}",
        parsed.coverage_gaps
    );
    assert_lossless(source, &parsed);
}

#[test]
fn malformed_stereotype_tail_consumes_adjacent_recovery_commas() {
    let source = r#"
Profile demo::Broken
{
  stereotypes: [first junk,,second];
}
"#;
    let parsed = parse(source);

    assert!(
        parsed
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == DiagCode::MalformedSyntax),
        "{:#?}",
        parsed.diagnostics
    );
    assert_eq!(
        count_kind(&parsed.green, SyntaxKind::DOMAIN_STEREOTYPE_DECL),
        3
    );
    assert_eq!(
        parsed
            .coverage_gaps
            .iter()
            .map(|gap| gap.kind)
            .collect::<Vec<_>>(),
        vec![DomainCoverageGapKind::MalformedDeclaration],
        "recovery must not turn the profile tail opaque: {:#?}",
        parsed.coverage_gaps
    );
    assert_lossless(source, &parsed);
}

#[test]
fn annotations_require_a_complete_path_and_value() {
    for source in [
        r#"
Class {.meta = 'value'} demo::Broken
{
  value: String[1];
}
"#,
        r#"
Class {meta:: = 'value'} demo::Broken
{
  value: String[1];
}
"#,
        r#"
Class {meta::tag =} demo::Broken
{
  value: String[1];
}
"#,
    ] {
        let parsed = parse(source);

        assert_eq!(
            count_kind(&parsed.green, SyntaxKind::DOMAIN_STEREOTYPE_APPLICATIONS),
            0,
            "malformed annotations cannot become model-bearing applications"
        );
        assert!(contains_kind(&parsed.green, SyntaxKind::ERROR_NODE));
        assert_eq!(
            parsed
                .coverage_gaps
                .iter()
                .map(|gap| gap.kind)
                .collect::<Vec<_>>(),
            vec![DomainCoverageGapKind::MalformedDeclaration],
            "annotations need a complete path and a nonempty value: {:#?}",
            parsed.coverage_gaps
        );
        assert_lossless(source, &parsed);
    }
}

#[test]
fn braced_annotations_accept_leading_qualified_paths() {
    let source = r#"
Class {::meta::tag = 'value'} demo::Annotated
{
  value: String[1];
}
"#;
    let parsed = parse(source);

    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    assert!(
        parsed.coverage_gaps.is_empty(),
        "{:#?}",
        parsed.coverage_gaps
    );
    assert_eq!(
        count_kind(&parsed.green, SyntaxKind::DOMAIN_STEREOTYPE_APPLICATIONS),
        1
    );
    assert_eq!(
        count_kind(&parsed.green, SyntaxKind::DOMAIN_PROPERTY_DECL),
        1
    );
    assert_lossless(source, &parsed);
}

#[test]
fn unclosed_braced_annotations_do_not_close_at_nested_values() {
    let source = r#"
Class {meta::tag = { nested: 'value' } demo::Broken
{
  value: String[1];
}
"#;
    let parsed = parse(source);

    assert_eq!(
        count_kind(&parsed.green, SyntaxKind::DOMAIN_STEREOTYPE_APPLICATIONS),
        0,
        "a nested value cannot close the outer annotation"
    );
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
            .any(|gap| gap.kind == DomainCoverageGapKind::MalformedDeclaration),
        "{:#?}",
        parsed.coverage_gaps
    );
    assert_lossless(source, &parsed);
}

#[test]
fn braced_annotations_do_not_take_assignments_from_nested_values() {
    let source = r#"
Class {meta::tag { nested = 'value' }} demo::Broken
{
  value: String[1];
}
"#;
    let parsed = parse(source);

    assert_eq!(
        count_kind(&parsed.green, SyntaxKind::DOMAIN_STEREOTYPE_APPLICATIONS),
        0,
        "an inner assignment cannot become the annotation value"
    );
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
            .any(|gap| gap.kind == DomainCoverageGapKind::MalformedDeclaration),
        "{:#?}",
        parsed.coverage_gaps
    );
    assert_lossless(source, &parsed);
}

#[test]
fn nested_qualified_property_bodies_remain_balanced() {
    let source = r#"
Class demo::Known
{
  derived(): String[1] { $this->filter({x: String[1] | true}); };
  kept: String[1];
}
"#;
    let parsed = parse(source);

    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    assert!(
        parsed.coverage_gaps.is_empty(),
        "{:#?}",
        parsed.coverage_gaps
    );
    assert_eq!(
        count_kind(&parsed.green, SyntaxKind::DOMAIN_QUALIFIED_PROPERTY_DECL),
        1
    );
    assert_eq!(
        count_kind(&parsed.green, SyntaxKind::DOMAIN_PROPERTY_DECL),
        1
    );
    assert_eq!(count_kind(&parsed.green, SyntaxKind::DOMAIN_OPAQUE_BODY), 1);
    assert_lossless(source, &parsed);
}

#[test]
fn braced_tagged_values_keep_nested_assignments_opaque() {
    let source = r#"
Class {meta::tag = { nested = 'value' }} demo::Annotated
{
  value: String[1];
}
"#;
    let parsed = parse(source);

    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    assert!(
        parsed.coverage_gaps.is_empty(),
        "{:#?}",
        parsed.coverage_gaps
    );
    assert_eq!(
        count_kind(&parsed.green, SyntaxKind::DOMAIN_STEREOTYPE_APPLICATIONS),
        1
    );
    assert_eq!(
        count_kind(&parsed.green, SyntaxKind::DOMAIN_PROPERTY_DECL),
        1
    );
    assert_lossless(source, &parsed);
}

#[test]
fn opaque_top_level_regions_end_at_semicolons() {
    let source = r#"
Enum demo::First;
Enum demo::Second;
Class demo::Kept
{
  value: String[1];
}
"#;
    let parsed = parse(source);

    assert_eq!(
        parsed
            .coverage_gaps
            .iter()
            .map(|gap| gap.kind)
            .collect::<Vec<_>>(),
        vec![
            DomainCoverageGapKind::UnsupportedTopLevel,
            DomainCoverageGapKind::UnsupportedTopLevel,
        ],
        "each semicolon-delimited unsupported declaration needs its own gap: {:#?}",
        parsed.coverage_gaps
    );
    assert_eq!(
        gap_texts(source, &parsed)
            .iter()
            .map(|gap| gap.trim())
            .collect::<Vec<_>>(),
        vec!["Enum demo::First;", "Enum demo::Second;"]
    );
    assert_eq!(count_kind(&parsed.green, SyntaxKind::DOMAIN_CLASS_DECL), 1);
    assert_lossless(source, &parsed);
}

#[test]
fn opaque_top_level_regions_leave_a_stray_closing_brace_for_recovery() {
    let source = r#"
Enum demo::Skipped
}
Class demo::Kept
{
  value: String[1];
}
"#;
    let parsed = parse(source);

    assert_eq!(
        parsed
            .coverage_gaps
            .iter()
            .map(|gap| gap.kind)
            .collect::<Vec<_>>(),
        vec![DomainCoverageGapKind::UnsupportedTopLevel],
        "adjacent unsupported top-level recovery ranges coalesce: {:#?}",
        parsed.coverage_gaps
    );
    assert_eq!(
        count_kind(&parsed.green, SyntaxKind::DOMAIN_OPAQUE_NODE),
        2,
        "the stray closing brace needs its own recovery node"
    );
    assert_eq!(count_kind(&parsed.green, SyntaxKind::DOMAIN_CLASS_DECL), 1);
    assert_lossless(source, &parsed);
}

#[test]
fn opaque_member_regions_end_at_semicolons() {
    let source = r#"
Class demo::Known
{
  nativeThing first;
  nativeThing second;
  kept: String[1];
}
"#;
    let parsed = parse(source);

    assert_eq!(
        parsed
            .coverage_gaps
            .iter()
            .map(|gap| gap.kind)
            .collect::<Vec<_>>(),
        vec![
            DomainCoverageGapKind::UnsupportedMember,
            DomainCoverageGapKind::UnsupportedMember,
        ],
        "each semicolon-delimited unsupported member needs its own gap: {:#?}",
        parsed.coverage_gaps
    );
    assert_eq!(
        gap_texts(source, &parsed)
            .iter()
            .map(|gap| gap.trim())
            .collect::<Vec<_>>(),
        vec!["nativeThing first;", "nativeThing second;"]
    );
    assert_eq!(
        count_kind(&parsed.green, SyntaxKind::DOMAIN_PROPERTY_DECL),
        1
    );
    assert_lossless(source, &parsed);
}

#[test]
fn nested_opaque_member_colons_do_not_start_qualified_properties() {
    for source in [
        r#"
Class demo::Known
{
  nativeThing prefix (key: value) derived(): String[1] { $this; };
  kept: String[1];
}
"#,
        r#"
Class demo::Known
{
  nativeThing prefix [key: value] derived(): String[1] { $this; };
  kept: String[1];
}
"#,
        r#"
Class demo::Known
{
  nativeThing prefix { key: value } derived(): String[1] { $this; };
  kept: String[1];
}
"#,
    ] {
        let parsed = parse(source);

        assert_eq!(
            count_kind(&parsed.green, SyntaxKind::DOMAIN_QUALIFIED_PROPERTY_DECL),
            0,
            "a nested colon belongs to the opaque member, not a following qualified property"
        );
        assert_eq!(
            count_kind(&parsed.green, SyntaxKind::DOMAIN_PROPERTY_DECL),
            1
        );
        assert_eq!(
            parsed
                .coverage_gaps
                .iter()
                .map(|gap| gap.kind)
                .collect::<Vec<_>>(),
            vec![DomainCoverageGapKind::UnsupportedMember],
            "nested opaque content must remain one unsupported member: {:#?}",
            parsed.coverage_gaps
        );
        assert_lossless(source, &parsed);
    }
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
