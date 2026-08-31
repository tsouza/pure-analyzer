//! Lossless-CST layout formatting for M3 query source.

use pure_analyzer_diagnostics::{Diagnostic, FileId};
use pure_analyzer_parser::parse_query;
use pure_analyzer_syntax::{BuildError, GreenElement, GreenNode, SyntaxKind};

const UNLIMITED_LINE_WIDTH: usize = usize::MAX;

/// Result of formatting one query without consulting a model or filesystem.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormatResult {
    text: String,
    diagnostics: Vec<Diagnostic>,
}

impl FormatResult {
    /// Return the canonical layout text.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Return parser diagnostics retained from the source being formatted.
    #[must_use]
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    /// Consume this result and return its canonical text and diagnostics.
    #[must_use]
    pub fn into_parts(self) -> (String, Vec<Diagnostic>) {
        (self.text, self.diagnostics)
    }
}

/// Format M3 query source while preserving its concrete token sequence.
///
/// Parsing is deliberately recovery-tolerant: syntax diagnostics are returned
/// alongside formatted text, and opaque islands are copied byte-for-byte.
pub fn format_query(source: &str, file: FileId) -> Result<FormatResult, BuildError> {
    format_query_with_width(source, file, UNLIMITED_LINE_WIDTH)
}

/// Format M3 query source with a preferred maximum line width.
///
/// Lines wrap only at whitespace boundaries introduced by canonical layout.
/// Tokens and opaque islands that exceed `line_width` remain intact.
pub fn format_query_with_width(
    source: &str,
    file: FileId,
    line_width: usize,
) -> Result<FormatResult, BuildError> {
    let parsed = parse_query(source, file)?;
    let mut formatter = LayoutFormatter::new(source, island_ranges(&parsed.green), line_width);
    for token in parsed.green.tokens() {
        formatter.token(token.kind(), token.text(), token.text_range());
    }
    Ok(FormatResult {
        text: formatter.finish(),
        diagnostics: parsed.diagnostics,
    })
}

fn island_ranges(node: &GreenNode) -> Vec<pure_analyzer_syntax::TextRange> {
    let mut ranges = Vec::new();
    collect_island_ranges(node, &mut ranges);
    ranges
}

fn collect_island_ranges(node: &GreenNode, ranges: &mut Vec<pure_analyzer_syntax::TextRange>) {
    if node.kind() == SyntaxKind::OPAQUE_ISLAND {
        ranges.push(node.text_range());
        return;
    }
    for child in node.children().iter().filter_map(GreenElement::as_node) {
        collect_island_ranges(child, ranges);
    }
}

struct LayoutFormatter<'source> {
    source: &'source str,
    islands: Vec<pure_analyzer_syntax::TextRange>,
    island_index: usize,
    output: String,
    previous: Option<SyntaxKind>,
    parens: usize,
    brackets: Vec<bool>,
    braces: usize,
    line_start: bool,
    line_width: usize,
}

impl<'source> LayoutFormatter<'source> {
    fn new(
        source: &'source str,
        mut islands: Vec<pure_analyzer_syntax::TextRange>,
        line_width: usize,
    ) -> Self {
        islands.sort_by_key(|range| range.start());
        Self {
            source,
            islands,
            island_index: 0,
            output: String::new(),
            previous: None,
            parens: 0,
            brackets: Vec::new(),
            braces: 0,
            line_start: true,
            line_width,
        }
    }

    fn token(&mut self, kind: SyntaxKind, text: &str, range: pure_analyzer_syntax::TextRange) {
        if kind == SyntaxKind::WHITESPACE {
            return;
        }
        if self.copy_island_if_open(range) {
            return;
        }
        if self.inside_island(range) {
            return;
        }
        if kind == SyntaxKind::LINE_COMMENT {
            self.whitespace_before(text.chars().count());
            self.output.push_str(text);
            self.newline();
            self.previous = Some(kind);
            return;
        }
        if kind == SyntaxKind::BLOCK_COMMENT {
            self.whitespace_before(text.chars().count());
            self.output.push_str(text);
            self.previous = Some(kind);
            return;
        }
        match kind {
            SyntaxKind::PAREN_OPEN => {
                self.write_before(kind, text.chars().count());
                self.output.push_str(text);
                self.parens = self.parens.saturating_add(1);
            }
            SyntaxKind::PAREN_CLOSE => {
                self.trim_space();
                self.output.push_str(text);
                self.parens = self.parens.saturating_sub(1);
            }
            SyntaxKind::BRACKET_OPEN => {
                self.write_before(kind, text.chars().count());
                self.output.push_str(text);
                self.brackets.push(self.previous == Some(SyntaxKind::TILDE));
            }
            SyntaxKind::BRACKET_CLOSE => {
                let multiline = self.brackets.pop().unwrap_or(false);
                if multiline && !self.line_start {
                    self.newline();
                }
                self.trim_space();
                self.output.push_str(text);
            }
            SyntaxKind::BRACE_OPEN => {
                self.write_before(kind, text.chars().count());
                self.output.push_str(text);
                self.braces = self.braces.saturating_add(1);
            }
            SyntaxKind::BRACE_CLOSE => {
                self.trim_space();
                self.output.push_str(text);
                self.braces = self.braces.saturating_sub(1);
            }
            SyntaxKind::COMMA => {
                self.trim_space();
                self.output.push_str(text);
                if self.brackets.last() == Some(&true) {
                    self.newline();
                    self.indent();
                }
            }
            SyntaxKind::SEMICOLON => {
                self.trim_space();
                self.output.push_str(text);
                self.newline();
            }
            SyntaxKind::ARROW => {
                self.trim_space();
                if !self.line_start {
                    self.newline();
                }
                self.indent();
                self.output.push_str(text);
            }
            SyntaxKind::DOT | SyntaxKind::PATH_SEPARATOR | SyntaxKind::COLON => {
                self.trim_space();
                self.output.push_str(text);
            }
            _ => {
                self.write_before(kind, text.chars().count());
                self.output.push_str(text);
            }
        }
        self.previous = Some(kind);
        if kind != SyntaxKind::SEMICOLON {
            self.line_start = false;
        }
    }

    fn copy_island_if_open(&mut self, range: pure_analyzer_syntax::TextRange) -> bool {
        let Some(island) = self.islands.get(self.island_index).copied() else {
            return false;
        };
        if range.start() != island.start() {
            return false;
        }
        let island_start = usize::from(island.start());
        let island_end = usize::from(island.end());
        let island_width = self
            .source
            .get(island_start..island_end)
            .unwrap_or_default()
            .chars()
            .count();
        self.write_before(SyntaxKind::HASH, island_width);
        self.output.push_str(
            self.source
                .get(island_start..island_end)
                .unwrap_or_default(),
        );
        self.island_index = self.island_index.saturating_add(1);
        self.previous = Some(SyntaxKind::HASH);
        self.line_start = false;
        true
    }

    fn inside_island(&self, range: pure_analyzer_syntax::TextRange) -> bool {
        self.island_index
            .checked_sub(1)
            .and_then(|index| self.islands.get(index))
            .is_some_and(|island| range.start() >= island.start() && range.end() <= island.end())
    }
    fn write_before(&mut self, kind: SyntaxKind, token_width: usize) {
        if needs_space(self.previous, kind) {
            self.whitespace_before(token_width);
        }
    }
    fn whitespace_before(&mut self, token_width: usize) {
        if !self.line_start && !self.output.ends_with(char::is_whitespace) {
            if self
                .current_column()
                .saturating_add(1)
                .saturating_add(token_width)
                > self.line_width
            {
                self.newline();
                self.indent();
            } else {
                self.output.push(' ');
            }
        }
    }
    fn current_column(&self) -> usize {
        self.output
            .rsplit_once('\n')
            .map_or(self.output.as_str(), |(_, line)| line)
            .chars()
            .count()
    }
    fn trim_space(&mut self) {
        while self.output.ends_with(' ') {
            self.output.pop();
        }
    }
    fn newline(&mut self) {
        self.trim_space();
        if !self.output.ends_with('\n') {
            self.output.push('\n');
        }
        self.line_start = true;
    }
    fn indent(&mut self) {
        if self.line_start {
            self.output.extend(std::iter::repeat_n(
                ' ',
                4 * (self.parens + self.braces + 1),
            ));
            self.line_start = false;
        }
    }
    fn finish(mut self) -> String {
        self.trim_space();
        if !self.output.is_empty() && !self.output.ends_with('\n') {
            self.output.push('\n');
        }
        self.output
    }
}

fn needs_space(previous: Option<SyntaxKind>, current: SyntaxKind) -> bool {
    let Some(previous) = previous else {
        return false;
    };
    !matches!(
        (previous, current),
        (SyntaxKind::DOLLAR, _)
            | (SyntaxKind::TILDE, _)
            | (SyntaxKind::AT, _)
            | (SyntaxKind::NEW_SYMBOL, _)
            | (
                SyntaxKind::HASH_STORE_OPEN | SyntaxKind::HASH_ISLAND_OPEN | SyntaxKind::HASH,
                _
            )
            | (SyntaxKind::DOT, _)
            | (SyntaxKind::PATH_SEPARATOR, _)
            | (SyntaxKind::ARROW, _)
            | (
                _,
                SyntaxKind::DOT
                    | SyntaxKind::PATH_SEPARATOR
                    | SyntaxKind::PAREN_CLOSE
                    | SyntaxKind::BRACKET_CLOSE
                    | SyntaxKind::COMMA
                    | SyntaxKind::SEMICOLON
            )
            | (
                _,
                SyntaxKind::PAREN_OPEN | SyntaxKind::BRACKET_OPEN | SyntaxKind::ISLAND_END
            )
            | (
                SyntaxKind::PAREN_OPEN | SyntaxKind::BRACKET_OPEN | SyntaxKind::BRACE_OPEN,
                _
            )
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use pure_analyzer_lexer::lex;
    fn format(source: &str) -> FormatResult {
        format_query(source, FileId::new(3)).expect("fixture must remain representable")
    }
    fn non_whitespace_tokens(source: &str) -> Vec<(SyntaxKind, String)> {
        lex(source)
            .into_iter()
            .filter_map(|(kind, range)| {
                let kind: SyntaxKind = kind.into();
                (kind != SyntaxKind::WHITESPACE).then(|| {
                    (
                        kind,
                        source[usize::from(range.start())..usize::from(range.end())].to_owned(),
                    )
                })
            })
            .collect()
    }
    #[test]
    fn formats_pipeline_columns_comments_and_islands_without_reordering_tokens() {
        let source = "#>{db::testDB.personTable}# ->join(#>{db::testDB.groupTable}#,JoinKind.INNER,{x,y|$x.ID==$y.PERSONID})->extend(~[id:Integer[1], name : String[1]]) // keep\n";
        let formatted = format(source).text().to_owned();
        let expected = concat!(
            "#>{db::testDB.personTable}#\n",
            "    ->join(#>{db::testDB.groupTable}#, JoinKind.INNER, {x, y | $x.ID == $y.PERSONID})\n",
            "    ->extend(~[id: Integer[1],\n",
            "        name: String[1]\n",
            "]) // keep\n",
        );
        assert_eq!(formatted, expected);
        assert_eq!(
            non_whitespace_tokens(source),
            non_whitespace_tokens(&formatted)
        );
    }
    #[test]
    fn formatting_is_idempotent_and_keeps_recovery_diagnostics() {
        let source = "$rows[ $index ] ->filter(x|$x.name=='Ada'";
        let once = format(source);
        let twice = format(once.text());
        assert_eq!(once.text(), twice.text());
        assert!(!once.diagnostics().is_empty());
        assert_eq!(
            non_whitespace_tokens(source),
            non_whitespace_tokens(once.text())
        );
    }
    #[test]
    fn preserves_opaque_island_bytes() {
        let source = "#{  opaque  #{ nested }#  }#->f()";
        let formatted = format(source).text().to_owned();
        assert!(formatted.starts_with("#{  opaque  #{ nested }#  }#"));
        assert_eq!(
            non_whitespace_tokens(source),
            non_whitespace_tokens(&formatted)
        );
    }
    #[test]
    fn line_width_wraps_at_layout_whitespace_without_splitting_tokens() {
        let source = "function(firstArgument,secondArgument,thirdArgument)";
        let wide = format_query_with_width(source, FileId::new(3), 80)
            .expect("fixture must remain representable");
        let narrow = format_query_with_width(source, FileId::new(3), 30)
            .expect("fixture must remain representable");

        assert_eq!(
            wide.text(),
            "function(firstArgument, secondArgument, thirdArgument)\n"
        );
        assert_eq!(
            narrow.text(),
            "function(firstArgument,\n        secondArgument,\n        thirdArgument)\n"
        );
        assert_eq!(
            non_whitespace_tokens(source),
            non_whitespace_tokens(narrow.text())
        );
        assert_eq!(
            format_query_with_width(narrow.text(), FileId::new(3), 30)
                .expect("formatted fixture must remain representable")
                .text(),
            narrow.text()
        );
    }
    #[test]
    fn line_width_keeps_opaque_islands_indivisible() {
        let island = "#{  opaque content that is intentionally wide  }#";
        let source = format!("function(firstArgument,{island},last)");
        let formatted = format_query_with_width(&source, FileId::new(3), 24)
            .expect("fixture must remain representable");

        assert!(formatted.text().contains(island));
        assert_eq!(
            non_whitespace_tokens(&source),
            non_whitespace_tokens(formatted.text())
        );
    }
    #[test]
    fn formatter_control_tokens_preserve_delimiters_whitespace_and_final_newline() {
        let range = pure_analyzer_syntax::TextRange::new(0.into(), 1.into());
        let mut formatter = LayoutFormatter::new("", Vec::new(), UNLIMITED_LINE_WIDTH);
        formatter.token(SyntaxKind::BRACE_OPEN, "{", range);
        assert_eq!(formatter.braces, 1);
        formatter.token(SyntaxKind::IDENT, "x", range);
        formatter.token(SyntaxKind::SEMICOLON, ";", range);

        assert_eq!(formatter.output, "{x;\n");
        formatter.output.push(' ');
        formatter.trim_space();
        assert_eq!(formatter.finish(), "{x;\n");
    }
    #[test]
    fn formatter_indentation_uses_all_nesting_levels() {
        let mut formatter = LayoutFormatter::new("", Vec::new(), UNLIMITED_LINE_WIDTH);
        formatter.parens = 1;
        formatter.braces = 2;
        formatter.indent();
        assert_eq!(formatter.output, "                ");
    }
    #[test]
    fn formatter_keeps_empty_input_empty() {
        assert_eq!(
            LayoutFormatter::new("", Vec::new(), UNLIMITED_LINE_WIDTH).finish(),
            ""
        );
    }

    #[test]
    fn consuming_format_result_retains_text_and_recovery_diagnostics() {
        let source = "$rows->filter(x|$x.name == 'Ada'";
        let result = format(source);
        let expected_text = result.text().to_owned();
        let expected_diagnostics = result.diagnostics().to_vec();

        assert!(!expected_diagnostics.is_empty());
        assert_eq!(result.into_parts(), (expected_text, expected_diagnostics));
    }
}
