//! Model-free grammar validation and targeted parser over-admission guards.

use pure_analyzer_diagnostics::{DiagCode, Diagnostic, Label, Severity};
use pure_analyzer_syntax::{GreenElement, GreenNode, GreenToken, SyntaxKind};

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
        walker.join_kinds(input.tree().tokens());
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
        if values.len() > 2
            && values
                .iter()
                .all(|token| token.kind() == SyntaxKind::LATEST_DATE)
        {
            self.error(
                DiagCode::MalformedMilestoningArguments,
                arguments.text_range(),
                "milestoning navigation accepts at most two date arguments",
            );
        }
    }

    fn join_kinds<'tree>(&mut self, tokens: impl Iterator<Item = &'tree GreenToken>) {
        let tokens = tokens
            .filter(|token| !is_trivia(token.kind()))
            .collect::<Vec<_>>();
        for window in tokens.windows(3) {
            let [prefix, dot, member] = window else {
                continue;
            };
            if prefix.kind() == SyntaxKind::IDENT
                && prefix.text() == "JoinKind"
                && dot.kind() == SyntaxKind::DOT
                && member.kind() == SyntaxKind::IDENT
                && !matches!(member.text(), "INNER" | "LEFT")
            {
                self.error(
                    DiagCode::UnknownJoinKind,
                    member.text_range(),
                    "unknown join kind; expected JoinKind.INNER or JoinKind.LEFT",
                );
            }
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
            "#>{db::testDB.left}#->join(#>{db::testDB.right}#, JoinKind.LEFT, {x,y| $x == $y})",
        ] {
            assert!(codes(source).is_empty(), "{source}");
        }
    }
}
