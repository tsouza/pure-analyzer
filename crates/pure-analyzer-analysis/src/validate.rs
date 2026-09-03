//! Model-free grammar validation and targeted parser over-admission guards.

use pure_analyzer_diagnostics::{DiagCode, Diagnostic, Label, Severity};
use pure_analyzer_syntax::{GreenElement, GreenNode, SyntaxKind};

use crate::{AnalysisInput, AnalysisPass};

/// Validates parser recovery findings and the intentionally permissive CST
/// shapes that Legend rejects without consulting a model.
#[derive(Debug, Default, Clone, Copy)]
pub struct ValidatePass;

impl AnalysisPass for ValidatePass {
    fn name(&self) -> &'static str {
        "validate"
    }

    fn analyze(&self, input: AnalysisInput<'_, '_>) -> Vec<Diagnostic> {
        let mut diagnostics = input.parse_diagnostics().to_vec();
        let mut walker = GuardWalker {
            file: input.file(),
            diagnostics: &mut diagnostics,
        };
        walker.visit(input.tree(), None);
        diagnostics
    }
}

struct GuardWalker<'diagnostics> {
    file: pure_analyzer_diagnostics::FileId,
    diagnostics: &'diagnostics mut Vec<Diagnostic>,
}

impl GuardWalker<'_> {
    fn visit(&mut self, node: &GreenNode, parent: Option<SyntaxKind>) {
        match node.kind() {
            SyntaxKind::PAREN_EXPR if parent != Some(SyntaxKind::CALL_ARGS) => {
                self.parenthesized_tuple(node);
            }
            SyntaxKind::BRACKET_INDEX => self.bracket_index(node),
            SyntaxKind::PROPERTY_NAV => self.milestoning_arguments(node),
            _ => {}
        }
        self.join_kind_references(node);
        for child in node.children().iter().filter_map(GreenElement::as_node) {
            self.visit(child, Some(node.kind()));
        }
    }

    fn parenthesized_tuple(&mut self, node: &GreenNode) {
        let comma = node
            .children()
            .iter()
            .filter_map(GreenElement::as_token)
            .find(|token| token.kind() == SyntaxKind::COMMA);
        if let Some(comma) = comma {
            self.error(
                DiagCode::ParenthesizedTuple,
                comma.text_range(),
                "parenthesized value tuples are not valid Pure expressions",
            );
        }
    }

    fn bracket_index(&mut self, node: &GreenNode) {
        let contents = node
            .tokens()
            .filter(|token| !is_trivia(token.kind()))
            .filter(|token| {
                !matches!(
                    token.kind(),
                    SyntaxKind::BRACKET_OPEN | SyntaxKind::BRACKET_CLOSE
                )
            })
            .collect::<Vec<_>>();
        let literal = matches!(contents.as_slice(), [token] if matches!(token.kind(), SyntaxKind::STRING | SyntaxKind::INTEGER));
        if !literal {
            self.error(
                DiagCode::IllegalBracketIndex,
                node.text_range(),
                "a bracket index must be a string or integer literal",
            );
        }
    }

    fn milestoning_arguments(&mut self, node: &GreenNode) {
        let Some(arguments) = node
            .children()
            .iter()
            .filter_map(GreenElement::as_node)
            .find(|child| child.kind() == SyntaxKind::CALL_ARGS)
        else {
            return;
        };
        let values = arguments
            .tokens()
            .filter(|token| !is_trivia(token.kind()))
            .filter(|token| {
                !matches!(
                    token.kind(),
                    SyntaxKind::PAREN_OPEN | SyntaxKind::PAREN_CLOSE | SyntaxKind::COMMA
                )
            })
            .collect::<Vec<_>>();
        if values.len() > 2 && values.iter().all(|token| is_date_literal(token.kind())) {
            self.error(
                DiagCode::MalformedMilestoningArguments,
                arguments.text_range(),
                "milestoning navigation accepts at most two date arguments",
            );
        }
    }

    /// Flags `JoinKind.<member>` where `<member>` is not a recognized join
    /// kind, but only when `JoinKind` is a genuine enum reference: a bare
    /// `QUALIFIED_NAME` immediately followed, among its parent's children, by
    /// the `PROPERTY_NAV` that reads `.<member>`. A `$`-prefixed variable
    /// (`$JoinKind`) parses as `VARIABLE_EXPR`, never `QUALIFIED_NAME`, so it
    /// can never match here regardless of spelling — matching on node kind
    /// rather than token text is what excludes it.
    fn join_kind_references(&mut self, node: &GreenNode) {
        let children = node
            .children()
            .iter()
            .filter_map(GreenElement::as_node)
            .collect::<Vec<_>>();
        for pair in children.windows(2) {
            let [target, property_nav] = pair else {
                continue;
            };
            if property_nav.kind() == SyntaxKind::PROPERTY_NAV && is_bare_join_kind(target) {
                self.join_kind_member(property_nav);
            }
        }
    }

    fn join_kind_member(&mut self, property_nav: &GreenNode) {
        let Some(member) = property_nav
            .children()
            .iter()
            .filter_map(GreenElement::as_token)
            .find(|token| !is_trivia(token.kind()) && token.kind() != SyntaxKind::DOT)
        else {
            return;
        };
        if member.kind() == SyntaxKind::IDENT && !matches!(member.text(), "INNER" | "LEFT") {
            self.error(
                DiagCode::UnknownJoinKind,
                member.text_range(),
                "unknown join kind; expected JoinKind.INNER or JoinKind.LEFT",
            );
        }
    }

    fn error(
        &mut self,
        code: DiagCode,
        span: pure_analyzer_syntax::TextRange,
        message: &'static str,
    ) {
        self.diagnostics.push(
            Diagnostic::builder(code, Severity::Error, message, Label::new(self.file, span))
                .build(),
        );
    }
}

const fn is_trivia(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        SyntaxKind::WHITESPACE | SyntaxKind::LINE_COMMENT | SyntaxKind::BLOCK_COMMENT
    )
}

/// Reports whether `kind` is one of the three date-literal token kinds Pure's
/// milestoning surface admits (`%2020-01-01T…`, `%2020-01-01`, `%latest`).
/// The engine caps milestoning arity at two dates regardless of which kind is
/// spelled — bitemporal, the widest stereotype, takes exactly two — so this
/// helper backs a model-free ceiling, not a claim about any one kind alone.
const fn is_date_literal(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        SyntaxKind::DATE_TIME | SyntaxKind::STRICT_DATE | SyntaxKind::LATEST_DATE
    )
}

/// Reports whether `node` is an unqualified `JoinKind` reference: a
/// `QUALIFIED_NAME` whose only non-trivia content is the single identifier
/// `JoinKind` (no `::` path segments). This is what distinguishes the real
/// `JoinKind` enum from a `VARIABLE_EXPR` or lambda parameter that merely
/// spells the same word.
fn is_bare_join_kind(node: &GreenNode) -> bool {
    node.kind() == SyntaxKind::QUALIFIED_NAME
        && matches!(
            node.children()
                .iter()
                .filter_map(GreenElement::as_token)
                .filter(|token| !is_trivia(token.kind()))
                .collect::<Vec<_>>()
                .as_slice(),
            [token] if token.kind() == SyntaxKind::IDENT && token.text() == "JoinKind"
        )
}

#[cfg(test)]
mod tests {
    use pure_analyzer_diagnostics::FileId;
    use pure_analyzer_parser::parse_query;

    use super::*;
    use crate::{AnalysisEngine, FindingPolicy};

    fn diagnostics(source: &str) -> Vec<Diagnostic> {
        let parsed = parse_query(source, FileId::new(7)).expect("fixture must build a tree");
        AnalysisEngine::new(vec![Box::new(ValidatePass)], FindingPolicy::new())
            .analyze(AnalysisInput::new(
                FileId::new(7),
                source,
                &parsed.green,
                &parsed.diagnostics,
                None,
            ))
            .into_diagnostics()
    }

    fn codes(source: &str) -> Vec<DiagCode> {
        diagnostics(source)
            .into_iter()
            .map(|diagnostic| diagnostic.code)
            .collect()
    }

    #[test]
    fn pass_name_is_stable() {
        assert_eq!(ValidatePass.name(), "validate");
    }

    #[test]
    fn preserves_parser_recovery_findings() {
        assert!(codes("#>{db::Table").contains(&DiagCode::UnterminatedIsland));
        assert!(codes("\0").contains(&DiagCode::BadToken));
    }

    #[test]
    fn rejects_each_targeted_over_admission_shape_at_its_local_span() {
        assert_eq!(codes("(a, b)"), [DiagCode::ParenthesizedTuple]);
        assert_eq!(codes("$rows[$index]"), [DiagCode::IllegalBracketIndex]);
        assert_eq!(
            codes("$x.prop(%latest, %latest, %latest)"),
            [DiagCode::MalformedMilestoningArguments]
        );
        assert_eq!(
            codes(
                "#>{db::testDB.left}#->join(#>{db::testDB.right}#, JoinKind.OUTER, {x,y| $x == $y})"
            ),
            [DiagCode::UnknownJoinKind]
        );
    }

    #[test]
    fn guard_spans_identify_the_offending_local_shape() {
        let cases = [
            ("(a, b)", DiagCode::ParenthesizedTuple, 2..3),
            ("$rows[$index]", DiagCode::IllegalBracketIndex, 5..13),
            (
                "$x.prop(%latest, %latest, %latest)",
                DiagCode::MalformedMilestoningArguments,
                7..34,
            ),
            (
                "#>{db::testDB.left}#->join(#>{db::testDB.right}#, JoinKind.OUTER, {x,y| $x == $y})",
                DiagCode::UnknownJoinKind,
                59..64,
            ),
        ];
        for (source, code, expected) in cases {
            let diagnostic = diagnostics(source)
                .into_iter()
                .find(|diagnostic| diagnostic.code == code)
                .expect("targeted guard must emit its registered diagnostic");
            assert_eq!(
                usize::from(diagnostic.primary.span.start())
                    ..usize::from(diagnostic.primary.span.end()),
                expected,
                "{source}"
            );
        }
    }

    #[test]
    fn accepts_legal_neighbours_without_model_facts() {
        for source in [
            "~column",
            "$x.prop()",
            "$x.prop($date)",
            "$rows['name'][0]",
            "f(a, b)",
            "f((a, b))",
            "$x.prop(%latest, %latest)",
            "$x.prop(%2020-01-01, %2020-01-02)",
            "$x.prop(%2020-01-01T12:30:00, %latest)",
            "#>{db::testDB.left}#->join(#>{db::testDB.right}#, JoinKind.LEFT, {x,y| $x == $y})",
        ] {
            assert!(codes(source).is_empty(), "{source}");
        }
    }

    #[test]
    fn milestoning_arity_guard_fires_for_every_date_token_kind_and_every_mix_of_them() {
        // Regression for #284: the guard's condition must agree with its own
        // message ("at most two date arguments") for STRICT_DATE and
        // DATE_TIME literals, not only the LATEST_DATE (`%latest`) kind it
        // originally special-cased. Bitemporal milestoning — the widest
        // stereotype the engine recognizes — never legally takes more than
        // two dates, and that ceiling does not depend on which date-literal
        // kind is spelled, so three-deep combinations of all three kinds
        // must all raise PUR1204.
        for source in [
            "$x.prop(%2020-01-01, %2020-01-02, %2020-01-03)",
            "$x.prop(%2020-01-01T12:30:00, %2020-01-02T12:30:00, %2020-01-03T12:30:00)",
            "$x.prop(%2020-01-01, %2020-01-02, %latest)",
            "$x.prop(%2020-01-01T12:30:00, %latest, %latest)",
            "$x.prop(%latest, %2020-01-01, %2020-01-01T12:30:00)",
        ] {
            assert_eq!(
                codes(source),
                [DiagCode::MalformedMilestoningArguments],
                "{source}"
            );
        }
    }

    #[test]
    fn a_variable_or_lambda_parameter_named_joinkind_is_not_the_enum() {
        // Regression for #283: `join_kinds` must key off the qualified-enum
        // CST shape, not a flat token scan, so a `$`-variable or lambda
        // parameter that merely spells "JoinKind" never collides with the
        // real `meta::pure::functions::relation::JoinKind` enum reference.
        for source in [
            "$JoinKind.name",
            "model::Person.all()->filter(JoinKind| $JoinKind.name == 'x')",
        ] {
            assert!(codes(source).is_empty(), "{source}");
        }
    }

    #[test]
    fn a_genuine_joinkind_enum_reference_with_an_unknown_member_still_errors() {
        // Regression for #283: the fix above must not weaken the guard into
        // a false negative for the real enum-qualified case.
        assert_eq!(
            codes(
                "#>{db::testDB.left}#->join(#>{db::testDB.right}#, JoinKind.OUTER, {x,y| $x == $y})"
            ),
            [DiagCode::UnknownJoinKind]
        );
    }
}
