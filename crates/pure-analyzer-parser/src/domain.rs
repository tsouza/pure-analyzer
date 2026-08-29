use pure_analyzer_diagnostics::{
    DiagCode, Diagnostic, FileId, Label, Severity, TextRange, TextSize,
};
use pure_analyzer_lexer::{SyntaxKind as TokenKind, lex};
use pure_analyzer_syntax::{BuildError, Event, GreenNode, GreenNodeBuilder, SyntaxKind};

const MAX_PARSE_DEPTH: usize = 256;

/// A conservatively identified part of a Domain source file.
///
/// Domain parsing intentionally recognizes only declarations which can
/// contribute model facts.  An unsupported construct is kept in the concrete
/// tree and reported here rather than being silently treated as a class,
/// property, or association end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DomainCoverageGap {
    /// Exact source range occupied by the conservatively unsupported region.
    pub span: TextRange,
    /// Why this source range prevents a complete model view.
    pub kind: DomainCoverageGapKind,
}

/// The source construct responsible for a [`DomainCoverageGap`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DomainCoverageGapKind {
    /// A top-level Domain declaration is outside the supported subset.
    UnsupportedTopLevel,
    /// A member of a supported declaration is outside the supported subset.
    UnsupportedMember,
    /// A malformed supported declaration cannot safely yield complete facts.
    MalformedDeclaration,
}

/// The result of parsing one Pure Domain source file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DomainParse {
    /// The owned, immutable, lossless concrete syntax tree.
    pub green: GreenNode,
    /// Parser and lexer diagnostics in source order.
    pub diagnostics: Vec<Diagnostic>,
    /// Conservatively unsupported source regions in source order.
    pub coverage_gaps: Vec<DomainCoverageGap>,
}

/// Parses the resilient, model-oriented Pure Domain subset.
///
/// The result always preserves every lexer token.  Classes, associations,
/// profiles, inheritance, properties, qualified-property signatures, types,
/// and multiplicities receive Domain-specific CST nodes.  Other legal Domain
/// constructs remain lossless opaque nodes and produce [`DomainCoverageGap`]
/// entries, so later model loading cannot invent facts from a partial parse.
///
/// Syntax failures are returned in [`DomainParse::diagnostics`]; an error
/// result indicates that the shared validated green-tree builder could not
/// represent the input, for example because it exceeds the `TextSize` limit.
pub fn parse_domain(source: &str, file: FileId) -> Result<DomainParse, BuildError> {
    let tokens = lex(source);
    let (events, diagnostics, coverage_gaps) = Parser::new(source, file, &tokens).parse();
    let mut builder = GreenNodeBuilder::new(source, &tokens);
    for event in events {
        builder.push(event);
    }
    builder.finish().map(|green| DomainParse {
        green,
        diagnostics,
        coverage_gaps,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeclarationKind {
    Class,
    Association,
    Profile,
}

struct Parser<'source, 'tokens> {
    source: &'source str,
    file: FileId,
    tokens: &'tokens [(TokenKind, TextRange)],
    index: usize,
    events: Vec<Event>,
    diagnostics: Vec<Diagnostic>,
    coverage_gaps: Vec<DomainCoverageGap>,
    fuel: usize,
    depth: usize,
}

impl<'source, 'tokens> Parser<'source, 'tokens> {
    fn new(source: &'source str, file: FileId, tokens: &'tokens [(TokenKind, TextRange)]) -> Self {
        Self {
            source,
            file,
            tokens,
            index: 0,
            events: Vec::with_capacity(tokens.len().saturating_add(2)),
            diagnostics: Vec::new(),
            coverage_gaps: Vec::new(),
            fuel: tokens.len(),
            depth: 0,
        }
    }

    fn parse(mut self) -> (Vec<Event>, Vec<Diagnostic>, Vec<DomainCoverageGap>) {
        self.open(SyntaxKind::ROOT);
        self.open(SyntaxKind::DOMAIN_FILE);

        while self.has_remaining_input() {
            self.consume_trivia();
            if !self.has_remaining_input() {
                break;
            }
            let semicolon_start = self.index;
            self.consume_if_raw(TokenKind::SEMICOLON);
            if self.index != semicolon_start {
                continue;
            }

            let before = self.index;
            match self.declaration_kind() {
                Some(DeclarationKind::Class) => self.parse_class(),
                Some(DeclarationKind::Association) => self.parse_association(),
                Some(DeclarationKind::Profile) => self.parse_profile(),
                None => self.parse_opaque_top_level(),
            }
            if self.index == before {
                self.syntax_error("parser made no progress while reading a Domain declaration");
                self.open(SyntaxKind::ERROR_NODE);
                let _ = self.bump();
                self.close();
            }
        }

        self.close();
        self.close();
        self.coverage_gaps
            .sort_by_key(|gap| (gap.span.start(), gap.span.end()));
        (self.events, self.diagnostics, self.coverage_gaps)
    }

    fn parse_class(&mut self) {
        let start = self.index;
        self.open(SyntaxKind::DOMAIN_CLASS_DECL);
        let keyword = self.expect_keyword("Class");
        self.parse_stereotype_applications();
        let name = self.parse_domain_qualified_name("a class name");
        let extends = if self.at_keyword("extends") {
            self.parse_extends_clause()
        } else {
            true
        };
        let body = self.parse_declaration_body(DeclarationKind::Class);
        let malformed = [keyword, name, extends, body].contains(&false);
        if malformed {
            self.mark_gap_from(start, DomainCoverageGapKind::MalformedDeclaration);
        }
        self.close();
    }

    fn parse_association(&mut self) {
        let start = self.index;
        self.open(SyntaxKind::DOMAIN_ASSOCIATION_DECL);
        let keyword = self.expect_keyword("Association");
        self.parse_stereotype_applications();
        let name = self.parse_domain_qualified_name("an association name");
        let body = self.parse_declaration_body(DeclarationKind::Association);
        let malformed = [keyword, name, body].contains(&false);
        if malformed {
            self.mark_gap_from(start, DomainCoverageGapKind::MalformedDeclaration);
        }
        self.close();
    }

    fn parse_profile(&mut self) {
        let start = self.index;
        self.open(SyntaxKind::DOMAIN_PROFILE_DECL);
        let keyword = self.expect_keyword("Profile");
        self.parse_stereotype_applications();
        let name = self.parse_domain_qualified_name("a profile name");
        let body = self.parse_profile_body();
        let malformed = [keyword, name, body].contains(&false);
        if malformed {
            self.mark_gap_from(start, DomainCoverageGapKind::MalformedDeclaration);
        }
        self.close();
    }

    fn parse_declaration_body(&mut self, kind: DeclarationKind) -> bool {
        if !self.expect(TokenKind::BRACE_OPEN, "`{` before declaration body") {
            self.recover_declaration_header();
            return false;
        }

        while self.has_remaining_input() {
            self.consume_trivia();
            if !self.has_remaining_input() {
                break;
            }
            if self.at(TokenKind::BRACE_CLOSE) {
                let _ = self.bump();
                return true;
            }
            if self.declaration_kind().is_some() {
                self.syntax_error("expected `}` before the next Domain declaration");
                return false;
            }
            let semicolon_start = self.index;
            self.consume_if_raw(TokenKind::SEMICOLON);
            if self.index != semicolon_start {
                continue;
            }

            let before = self.index;
            self.parse_member(kind);
            if self.index == before {
                self.syntax_error("parser made no progress while reading a Domain member");
                self.open(SyntaxKind::ERROR_NODE);
                let _ = self.bump();
                self.close();
            }
        }

        self.syntax_error("expected `}` before end of file");
        false
    }

    fn parse_member(&mut self, kind: DeclarationKind) {
        self.parse_stereotype_applications();
        self.consume_trivia();
        if !self.has_remaining_input() {
            return;
        }
        if self.member_starts_property() {
            self.parse_property();
        } else if kind == DeclarationKind::Class && self.member_starts_qualified_property() {
            self.parse_qualified_property();
        } else {
            self.parse_opaque_member();
        }
    }

    fn parse_property(&mut self) {
        let start = self.index;
        self.open(SyntaxKind::DOMAIN_PROPERTY_DECL);
        let name = self.consume_name("a property name");
        let colon = self.expect(TokenKind::COLON, "`:` after a property name");
        let ty = self.parse_domain_type_reference();
        let multiplicity = self.parse_domain_multiplicity();
        if self.consume_if(TokenKind::ASSIGN) {
            self.parse_opaque_expression();
        }
        let terminator = self.consume_member_terminator();
        let malformed = [name, colon, ty, multiplicity, terminator].contains(&false);
        if malformed {
            self.mark_gap_from(start, DomainCoverageGapKind::MalformedDeclaration);
        }
        self.close();
    }

    fn parse_qualified_property(&mut self) {
        let start = self.index;
        self.open(SyntaxKind::DOMAIN_QUALIFIED_PROPERTY_DECL);
        let name = self.consume_name("a qualified-property name");
        let open = self.expect(TokenKind::PAREN_OPEN, "`(` after a qualified-property name");
        let parameters = if open {
            self.parse_parameter_list()
        } else {
            false
        };
        let close = if open {
            self.expect(
                TokenKind::PAREN_CLOSE,
                "`)` after qualified-property parameters",
            )
        } else {
            false
        };
        let colon = self.expect(TokenKind::COLON, "`:` after qualified-property parameters");
        let ty = self.parse_domain_type_reference();
        let multiplicity = self.parse_domain_multiplicity();
        let body = if self.at(TokenKind::BRACE_OPEN) {
            self.parse_opaque_body();
            true
        } else {
            self.syntax_error("expected `{` before a qualified-property body");
            self.recover_member_tail();
            false
        };
        let _ = self.consume_if(TokenKind::SEMICOLON);
        let malformed =
            [name, open, parameters, close, colon, ty, multiplicity, body].contains(&false);
        if malformed {
            self.mark_gap_from(start, DomainCoverageGapKind::MalformedDeclaration);
        }
        self.close();
    }

    fn parse_parameter_list(&mut self) -> bool {
        self.consume_trivia();
        if self.at(TokenKind::PAREN_CLOSE) {
            return true;
        }

        let mut valid = true;
        loop {
            let before = self.index;
            self.open(SyntaxKind::DOMAIN_PARAMETER_DECL);
            let name = self.consume_name("a parameter name");
            let colon = self.expect(TokenKind::COLON, "`:` after a parameter name");
            let ty = self.parse_domain_type_reference();
            let multiplicity = self.parse_domain_multiplicity();
            self.close();
            valid &= name && colon && ty && multiplicity;
            self.consume_trivia();

            let comma_start = self.index;
            self.consume_if_raw(TokenKind::COMMA);
            if self.index != comma_start {
                self.consume_trivia();
                if self.at(TokenKind::PAREN_CLOSE) {
                    self.syntax_error("expected a parameter after `,`");
                    return false;
                }
                continue;
            }
            if self.at(TokenKind::PAREN_CLOSE) || !self.has_remaining_input() {
                return valid;
            }
            self.syntax_error("expected `,` or `)` after a qualified-property parameter");
            valid = false;
            self.recover_until(&[
                TokenKind::COMMA,
                TokenKind::PAREN_CLOSE,
                TokenKind::BRACE_CLOSE,
            ]);
            if self.index == before {
                return false;
            }
            let comma_start = self.index;
            self.consume_if_raw(TokenKind::COMMA);
            if self.index != comma_start {
                continue;
            }
            return false;
        }
    }

    fn parse_extends_clause(&mut self) -> bool {
        self.open(SyntaxKind::DOMAIN_EXTENDS_CLAUSE);
        let keyword = self.expect_keyword("extends");
        let mut valid = keyword && self.parse_domain_qualified_name("a supertype after `extends`");
        while self.consume_if(TokenKind::COMMA) {
            valid &= self.parse_domain_qualified_name("a supertype after `,`");
        }
        if !valid {
            self.syntax_error("expected a valid supertype after `extends`");
        }
        self.close();
        valid
    }

    fn parse_profile_body(&mut self) -> bool {
        if !self.expect(TokenKind::BRACE_OPEN, "`{` before profile body") {
            self.recover_declaration_header();
            return false;
        }

        let mut valid = true;
        while self.has_remaining_input() {
            self.consume_trivia();
            if !self.has_remaining_input() {
                break;
            }
            if self.at(TokenKind::BRACE_CLOSE) {
                let _ = self.bump();
                return valid;
            }
            if self.declaration_kind().is_some() {
                self.syntax_error("expected `}` before the next Domain declaration");
                return false;
            }
            let before = self.index;
            if self.at_keyword("stereotypes") || self.at_keyword("tags") {
                valid &= self.parse_profile_section();
            } else {
                let semicolon_start = self.index;
                self.consume_if_raw(TokenKind::SEMICOLON);
                if self.index == semicolon_start {
                    self.parse_opaque_member();
                }
            }
            if self.index == before {
                self.syntax_error("parser made no progress while reading Domain profile member");
                self.open(SyntaxKind::ERROR_NODE);
                let _ = self.bump();
                self.close();
                valid = false;
            }
        }

        self.syntax_error("expected `}` before end of file");
        false
    }

    fn parse_profile_section(&mut self) -> bool {
        let stereotypes = self.at_keyword("stereotypes");
        self.open(SyntaxKind::DOMAIN_PROFILE_SECTION);
        let _ = self.bump();
        let colon = self.expect(TokenKind::COLON, "`:` after a profile section name");
        let open = self.expect(TokenKind::BRACKET_OPEN, "`[` after a profile section name");
        let header = colon && open;
        let contents = if header {
            if stereotypes {
                self.parse_stereotype_list()
            } else {
                self.consume_profile_tag_list();
                true
            }
        } else {
            false
        };
        let close = if header {
            self.expect(TokenKind::BRACKET_CLOSE, "`]` after a profile section")
        } else {
            false
        };
        let _ = self.consume_if(TokenKind::SEMICOLON);
        self.close();
        header && contents && close
    }

    fn parse_stereotype_list(&mut self) -> bool {
        self.consume_trivia();
        let mut valid = true;
        while self.has_remaining_input() && !self.at(TokenKind::BRACKET_CLOSE) {
            self.open(SyntaxKind::DOMAIN_STEREOTYPE_DECL);
            let name = self.consume_name("a stereotype name");
            self.close();
            valid &= name;
            if !name {
                self.recover_until(&[TokenKind::COMMA, TokenKind::BRACKET_CLOSE]);
            }
            self.consume_trivia();
            let comma_start = self.index;
            self.consume_if_raw(TokenKind::COMMA);
            if self.index != comma_start {
                self.consume_trivia();
                continue;
            }
            if !self.at(TokenKind::BRACKET_CLOSE) {
                self.syntax_error("expected `,` or `]` after a stereotype name");
                valid = false;
                self.recover_until(&[TokenKind::COMMA, TokenKind::BRACKET_CLOSE]);
                let comma_start = self.index;
                self.consume_if_raw(TokenKind::COMMA);
                if self.index == comma_start && self.raw_at(TokenKind::COMMA) {
                    return false;
                }
            }
        }
        valid
    }

    fn consume_profile_tag_list(&mut self) {
        let mut depth = 0usize;
        while self.has_remaining_input() {
            match self.raw_kind() {
                Some(TokenKind::BRACKET_CLOSE) if depth == 0 => break,
                Some(TokenKind::BRACKET_OPEN | TokenKind::PAREN_OPEN | TokenKind::BRACE_OPEN) => {
                    depth = depth.saturating_add(1);
                }
                Some(
                    TokenKind::BRACKET_CLOSE | TokenKind::PAREN_CLOSE | TokenKind::BRACE_CLOSE,
                ) => {
                    depth = depth.saturating_sub(1);
                }
                _ => {}
            }
            let _ = self.bump();
        }
    }

    fn parse_domain_qualified_name(&mut self, expected: &str) -> bool {
        self.open(SyntaxKind::DOMAIN_QUALIFIED_NAME);
        let _ = self.consume_if(TokenKind::PATH_SEPARATOR);
        let mut valid = self.consume_name(expected);
        while self.consume_if(TokenKind::PATH_SEPARATOR) {
            valid &= self.consume_name("a name after `::`");
            if !valid {
                break;
            }
        }
        self.close();
        valid
    }

    fn parse_domain_type_reference(&mut self) -> bool {
        if !self.enter_parse_depth() {
            return false;
        }
        self.open(SyntaxKind::DOMAIN_TYPE_REF);
        let mut valid = self.parse_domain_qualified_name("a type name");
        if self.consume_if(TokenKind::LT) {
            self.consume_trivia();
            if self.at(TokenKind::GT) {
                self.syntax_error("expected a type argument after `<`");
                valid = false;
            }
            while self.has_remaining_input() && !self.at(TokenKind::GT) {
                valid &= self.parse_domain_type_reference();
                self.consume_trivia();
                let comma_start = self.index;
                self.consume_if_raw(TokenKind::COMMA);
                if self.index != comma_start {
                    self.consume_trivia();
                    continue;
                }
                if !self.at(TokenKind::GT) {
                    self.syntax_error("expected `,` or `>` after a type argument");
                    self.recover_until(&[TokenKind::COMMA, TokenKind::GT]);
                    let comma_start = self.index;
                    self.consume_if_raw(TokenKind::COMMA);
                    if self.index == comma_start && self.raw_at(TokenKind::COMMA) {
                        valid = false;
                        break;
                    }
                }
            }
            valid &= self.expect(TokenKind::GT, "`>` after type arguments");
        }
        self.close();
        self.leave_parse_depth();
        valid
    }

    fn parse_domain_multiplicity(&mut self) -> bool {
        self.consume_trivia();
        if !self.at(TokenKind::BRACKET_OPEN) {
            self.syntax_error("expected a multiplicity after a type");
            return false;
        }
        self.open(SyntaxKind::DOMAIN_MULTIPLICITY);
        let open = self.expect(TokenKind::BRACKET_OPEN, "`[` before a multiplicity");
        let lower = self.consume_multiplicity_bound();
        let upper = if self.consume_if(TokenKind::DOT) {
            self.expect(TokenKind::DOT, "second `.` in a multiplicity range")
                && self.consume_multiplicity_bound()
        } else {
            true
        };
        let close = self.expect(TokenKind::BRACKET_CLOSE, "`]` after a multiplicity");
        self.close();
        open && lower && upper && close
    }

    fn consume_multiplicity_bound(&mut self) -> bool {
        if self.consume_if(TokenKind::INTEGER) || self.consume_if(TokenKind::STAR) {
            true
        } else {
            self.syntax_error("expected a multiplicity bound");
            false
        }
    }

    fn parse_stereotype_applications(&mut self) {
        self.consume_trivia();
        while self.at(TokenKind::BRACE_OPEN) || self.at_double_angle_open() {
            let start = self.index;
            let braced = self.at(TokenKind::BRACE_OPEN);
            let structurally_valid = if braced {
                self.braced_annotation_is_valid()
            } else {
                self.double_angle_stereotype_is_valid()
            };
            self.open(if structurally_valid {
                SyntaxKind::DOMAIN_STEREOTYPE_APPLICATIONS
            } else {
                SyntaxKind::ERROR_NODE
            });
            let closed = if braced {
                self.consume_balanced_braces()
            } else {
                self.consume_double_angle()
            };
            self.close();
            if self.index == start {
                self.syntax_error(
                    "parser made no progress while reading Domain stereotype applications",
                );
                self.open(SyntaxKind::ERROR_NODE);
                let _ = self.bump();
                self.close();
            }
            if !structurally_valid || !closed {
                if closed {
                    self.syntax_error("malformed Domain stereotype or tagged-value application");
                }
                self.mark_gap_from(start, DomainCoverageGapKind::MalformedDeclaration);
            }
            self.consume_trivia();
        }
    }

    fn braced_annotation_is_valid(&self) -> bool {
        let Some(open) = self.significant_index() else {
            return false;
        };
        if self.tokens[open].0 != TokenKind::BRACE_OPEN {
            return false;
        }

        let mut depth = 1usize;
        let mut assignment = None;
        for (index, (kind, _)) in self.tokens.iter().enumerate().skip(open.saturating_add(1)) {
            if *kind == TokenKind::BRACE_OPEN {
                depth = depth.saturating_add(1);
                continue;
            }
            match (*kind, assignment, depth) {
                (TokenKind::BRACE_CLOSE, _, _) => {
                    depth = depth.saturating_sub(1);
                    if let 0 = depth {
                        let Some(assignment) = assignment else {
                            return false;
                        };
                        if self.annotation_path_is_valid(open.saturating_add(1), assignment) {
                            return self
                                .next_significant_index_from(assignment.saturating_add(1))
                                .is_some_and(|value| value < index);
                        }
                        return false;
                    }
                }
                (TokenKind::ASSIGN, None, 1) => assignment = Some(index),
                _ => {}
            }
        }
        false
    }

    fn double_angle_stereotype_is_valid(&self) -> bool {
        let Some(first_open) = self.significant_index() else {
            return false;
        };
        let Some(second_open) = self.next_significant_index_from(first_open.saturating_add(1))
        else {
            return false;
        };
        let (TokenKind::LT, TokenKind::LT) =
            (self.tokens[first_open].0, self.tokens[second_open].0)
        else {
            return false;
        };

        let mut cursor = second_open.saturating_add(1);
        while let Some(first_close) = self.next_significant_index_from(cursor) {
            if self.tokens[first_close].0 == TokenKind::GT
                && self
                    .next_significant_index_from(first_close.saturating_add(1))
                    .is_some_and(|second_close| self.tokens[second_close].0 == TokenKind::GT)
            {
                return self.annotation_path_is_valid(second_open.saturating_add(1), first_close);
            }
            cursor = first_close.saturating_add(1);
        }
        false
    }

    fn annotation_path_is_valid(&self, start: usize, end: usize) -> bool {
        let mut cursor = start;
        let mut saw_name = false;
        let mut expect_name = true;
        let mut leading_path_separator_allowed = true;

        while let Some(index) = self.next_significant_index_from(cursor) {
            if index >= end {
                break;
            }
            let kind = self.tokens[index].0;
            if expect_name {
                if is_name(kind) {
                    saw_name = true;
                    expect_name = false;
                    leading_path_separator_allowed = false;
                } else if !saw_name
                    && leading_path_separator_allowed
                    && kind == TokenKind::PATH_SEPARATOR
                {
                    leading_path_separator_allowed = false;
                } else {
                    return false;
                }
            } else if matches!(kind, TokenKind::PATH_SEPARATOR | TokenKind::DOT) {
                expect_name = true;
            } else {
                return false;
            }
            cursor = index.saturating_add(1);
        }

        saw_name && !expect_name
    }

    fn parse_opaque_body(&mut self) {
        self.open(SyntaxKind::DOMAIN_OPAQUE_BODY);
        let _ = self.consume_balanced_braces();
        self.close();
    }

    fn parse_opaque_expression(&mut self) {
        self.open(SyntaxKind::DOMAIN_OPAQUE_BODY);
        let mut parentheses = 0usize;
        let mut brackets = 0usize;
        let mut braces = 0usize;
        while self.has_remaining_input() {
            if parentheses == 0
                && brackets == 0
                && braces == 0
                && (self.raw_at(TokenKind::SEMICOLON) || self.raw_at(TokenKind::BRACE_CLOSE))
            {
                break;
            }
            self.adjust_delimiter_depths(&mut parentheses, &mut brackets, &mut braces);
            let _ = self.bump();
        }
        self.close();
    }

    fn parse_opaque_top_level(&mut self) {
        let start = self.index;
        self.open(SyntaxKind::DOMAIN_OPAQUE_NODE);
        let mut parentheses = 0usize;
        let mut brackets = 0usize;
        let mut braces = 0usize;
        let mut consumed = false;

        while self.has_remaining_input() {
            if consumed
                && parentheses == 0
                && brackets == 0
                && braces == 0
                && self.declaration_kind().is_some()
            {
                break;
            }
            if consumed
                && parentheses == 0
                && brackets == 0
                && braces == 0
                && self.raw_at(TokenKind::BRACE_CLOSE)
            {
                break;
            }

            let ends_declaration = self.raw_at(TokenKind::SEMICOLON)
                && parentheses == 0
                && brackets == 0
                && braces == 0;
            let ends_braced_declaration = self.raw_at(TokenKind::BRACE_CLOSE) && braces == 1;
            self.adjust_delimiter_depths(&mut parentheses, &mut brackets, &mut braces);
            let _ = self.bump();
            consumed = true;
            if ends_declaration || ends_braced_declaration {
                break;
            }
        }
        self.close();
        self.mark_gap_from(start, DomainCoverageGapKind::UnsupportedTopLevel);
    }

    fn parse_opaque_member(&mut self) {
        let start = self.index;
        self.open(SyntaxKind::DOMAIN_OPAQUE_NODE);
        let mut parentheses = 0usize;
        let mut brackets = 0usize;
        let mut braces = 0usize;
        let mut consumed = false;
        let mut saw_property_colon = false;

        while self.has_remaining_input() {
            if consumed
                && parentheses == 0
                && brackets == 0
                && braces == 0
                && self.at_opaque_member_recovery_boundary(saw_property_colon)
            {
                break;
            }

            let ends_member = self.raw_at(TokenKind::SEMICOLON)
                && parentheses == 0
                && brackets == 0
                && braces == 0;
            saw_property_colon |=
                self.raw_at(TokenKind::COLON) && parentheses == 0 && brackets == 0 && braces == 0;
            self.adjust_delimiter_depths(&mut parentheses, &mut brackets, &mut braces);
            let _ = self.bump();
            consumed = true;
            if ends_member {
                break;
            }
        }
        self.close();
        self.mark_gap_from(start, DomainCoverageGapKind::UnsupportedMember);
    }

    fn at_opaque_member_recovery_boundary(&self, saw_property_colon: bool) -> bool {
        if self.raw_at(TokenKind::BRACE_CLOSE) {
            return true;
        }
        if self.member_starts_property() {
            return true;
        }
        // A bare `name(` may be part of an unsupported member, such as
        // `nativeThing helper(...)`. It is a safe recovery boundary only
        // after this opaque region has already looked like a malformed
        // property (`name: Type ...`).
        if saw_property_colon && self.member_starts_qualified_property() {
            return true;
        }
        self.declaration_kind().is_some()
    }

    fn consume_balanced_braces(&mut self) -> bool {
        if !self.expect(
            TokenKind::BRACE_OPEN,
            "`{` before a braced Domain construct",
        ) {
            return false;
        }
        let mut depth = 1usize;
        while self.has_remaining_input() {
            match self.raw_kind() {
                Some(TokenKind::BRACE_OPEN) => depth = depth.saturating_add(1),
                Some(TokenKind::BRACE_CLOSE) => {
                    depth = depth.saturating_sub(1);
                    let _ = self.bump();
                    if depth == 0 {
                        return true;
                    }
                    continue;
                }
                _ => {}
            }
            let _ = self.bump();
        }
        self.syntax_error("expected `}` before end of file");
        false
    }

    fn consume_double_angle(&mut self) -> bool {
        let first = self.expect(TokenKind::LT, "first `<` before stereotype applications");
        let second = self.expect(TokenKind::LT, "second `<` before stereotype applications");
        let (true, true) = (first, second) else {
            return false;
        };
        while self.has_remaining_input() {
            if self.at_double_angle_close() {
                let _ = self.bump();
                let _ = self.bump();
                return true;
            }
            let _ = self.bump();
        }
        self.syntax_error("expected `>>` before end of file");
        false
    }

    fn consume_member_terminator(&mut self) -> bool {
        self.consume_trivia();
        let semicolon_start = self.index;
        self.consume_if_raw(TokenKind::SEMICOLON);
        if self.index != semicolon_start {
            return true;
        }
        self.syntax_error("expected `;` after a property declaration");
        if self.has_remaining_input() {
            self.recover_member_tail();
        }
        false
    }

    fn recover_declaration_header(&mut self) {
        self.open(SyntaxKind::ERROR_NODE);
        while self.has_remaining_input() {
            if self.at_declaration_header_recovery_boundary() {
                break;
            }
            let before = self.index;
            let _ = self.bump();
            if self.index == before {
                break;
            }
        }
        let semicolon_start = self.index;
        self.consume_if_raw(TokenKind::SEMICOLON);
        if self.index == semicolon_start && self.raw_at(TokenKind::SEMICOLON) {
            self.syntax_error("parser made no progress while consuming Domain recovery terminator");
        }
        self.close();
    }

    fn at_declaration_header_recovery_boundary(&self) -> bool {
        if self.raw_at(TokenKind::SEMICOLON) {
            return true;
        }
        if self.raw_at(TokenKind::BRACE_CLOSE) {
            return true;
        }
        self.declaration_kind().is_some()
    }

    fn recover_member_tail(&mut self) {
        // A successfully parsed prefix may be followed immediately by another
        // member after a missing multiplicity or terminator.  Leave a plausible
        // member start for the outer declaration loop: it can still recover a
        // model fact from that later member.  Returning here is safe because
        // the caller has already consumed the malformed member prefix.
        if self.at_member_recovery_boundary() {
            return;
        }

        self.open(SyntaxKind::ERROR_NODE);
        while self.has_remaining_input() {
            if self.at_member_recovery_boundary() {
                break;
            }
            let before = self.index;
            self.bump();
            if self.index == before {
                break;
            }
        }
        if self.at(TokenKind::SEMICOLON) {
            let _ = self.consume_if(TokenKind::SEMICOLON);
        }
        self.close();
    }

    fn at_member_recovery_boundary(&self) -> bool {
        if self.at(TokenKind::SEMICOLON) {
            return true;
        }
        if self.at(TokenKind::BRACE_CLOSE) {
            return true;
        }
        if self.declaration_kind().is_some() {
            return true;
        }
        if self.member_starts_property() {
            return true;
        }
        self.member_starts_qualified_property()
    }

    fn recover_until(&mut self, boundaries: &[TokenKind]) {
        while self.has_remaining_input() && !self.at_any(boundaries) {
            let _ = self.bump();
        }
    }

    fn member_starts_property(&self) -> bool {
        let Some(name) = self.significant_index() else {
            return false;
        };
        self.tokens
            .get(name)
            .is_some_and(|(kind, _)| is_name(*kind))
            && self.next_significant_kind(name.saturating_add(1)) == Some(TokenKind::COLON)
    }

    fn member_starts_qualified_property(&self) -> bool {
        let Some(name) = self.significant_index() else {
            return false;
        };
        self.tokens
            .get(name)
            .is_some_and(|(kind, _)| is_name(*kind))
            && self.next_significant_kind(name.saturating_add(1)) == Some(TokenKind::PAREN_OPEN)
    }

    fn declaration_kind(&self) -> Option<DeclarationKind> {
        if self.at_keyword("Class") {
            Some(DeclarationKind::Class)
        } else if self.at_keyword("Association") {
            Some(DeclarationKind::Association)
        } else if self.at_keyword("Profile") {
            Some(DeclarationKind::Profile)
        } else {
            None
        }
    }

    fn at_keyword(&self, keyword: &str) -> bool {
        self.significant_index().is_some_and(|index| {
            self.tokens
                .get(index)
                .is_some_and(|(kind, range)| is_name(*kind) && self.text(*range) == keyword)
        })
    }

    fn expect_keyword(&mut self, keyword: &str) -> bool {
        self.consume_trivia();
        if self.at_keyword(keyword) {
            let _ = self.bump();
            true
        } else {
            self.syntax_error(&format!("expected `{keyword}`"));
            false
        }
    }

    fn at_double_angle_open(&self) -> bool {
        if self.at(TokenKind::LT) {
            return self.significant_index().is_some_and(|index| {
                self.next_significant_kind(index.saturating_add(1)) == Some(TokenKind::LT)
            });
        }
        false
    }

    fn at_double_angle_close(&self) -> bool {
        self.at(TokenKind::GT)
            && self.significant_index().is_some_and(|index| {
                self.next_significant_kind(index.saturating_add(1)) == Some(TokenKind::GT)
            })
    }

    fn adjust_delimiter_depths(
        &self,
        parentheses: &mut usize,
        brackets: &mut usize,
        braces: &mut usize,
    ) {
        match self.raw_kind() {
            Some(TokenKind::PAREN_OPEN) => *parentheses = parentheses.saturating_add(1),
            Some(TokenKind::PAREN_CLOSE) => *parentheses = parentheses.saturating_sub(1),
            Some(TokenKind::BRACKET_OPEN) => *brackets = brackets.saturating_add(1),
            Some(TokenKind::BRACKET_CLOSE) => *brackets = brackets.saturating_sub(1),
            Some(TokenKind::BRACE_OPEN) => *braces = braces.saturating_add(1),
            Some(TokenKind::BRACE_CLOSE) => *braces = braces.saturating_sub(1),
            _ => {}
        }
    }

    fn expect(&mut self, kind: TokenKind, expected: &str) -> bool {
        self.consume_trivia();
        if self.raw_kind() == Some(kind) {
            let _ = self.bump();
            true
        } else {
            self.syntax_error(&format!("expected {expected}"));
            false
        }
    }

    fn consume_if(&mut self, kind: TokenKind) -> bool {
        self.consume_trivia();
        let before = self.index;
        self.consume_if_raw(kind);
        self.index != before
    }

    fn consume_if_raw(&mut self, kind: TokenKind) {
        if self.raw_at(kind) {
            let _ = self.bump();
        }
    }

    fn consume_name(&mut self, expected: &str) -> bool {
        self.consume_trivia();
        if self.raw_kind().is_some_and(is_name) {
            self.bump()
        } else {
            self.syntax_error(&format!("expected {expected}"));
            false
        }
    }

    fn consume_trivia(&mut self) {
        while self.has_remaining_input() && self.raw_kind().is_some_and(is_trivia) {
            let _ = self.bump();
        }
    }

    fn at(&self, kind: TokenKind) -> bool {
        self.significant_kind() == Some(kind)
    }

    fn raw_at(&self, kind: TokenKind) -> bool {
        self.raw_kind() == Some(kind)
    }

    fn at_any(&self, kinds: &[TokenKind]) -> bool {
        self.significant_kind()
            .is_some_and(|kind| kinds.contains(&kind))
    }

    fn significant_kind(&self) -> Option<TokenKind> {
        self.significant_index()
            .and_then(|index| self.tokens.get(index).map(|(kind, _)| *kind))
    }

    fn significant_index(&self) -> Option<usize> {
        self.next_significant_index_from(self.index)
    }

    fn next_significant_kind(&self, start: usize) -> Option<TokenKind> {
        self.next_significant_index_from(start)
            .map(|index| self.tokens[index].0)
    }

    fn next_significant_index_from(&self, start: usize) -> Option<usize> {
        self.tokens
            .get(start..)
            .and_then(|tokens| tokens.iter().position(|(kind, _)| !is_trivia(*kind)))
            .map(|offset| start.saturating_add(offset))
    }

    fn raw_kind(&self) -> Option<TokenKind> {
        self.tokens.get(self.index).map(|(kind, _)| *kind)
    }

    fn has_remaining_input(&self) -> bool {
        if self.fuel == 0 {
            return false;
        }
        self.tokens.get(self.index).is_some()
    }

    fn bump(&mut self) -> bool {
        if self.has_remaining_input() {
            if self.raw_kind() == Some(TokenKind::ERROR) {
                self.push_diagnostic(
                    DiagCode::BadToken,
                    self.current_span(),
                    "unrecognized token",
                );
            }
            self.events.push(Event::Advance);
            self.index = self.index.saturating_add(1);
            self.fuel = self.fuel.saturating_sub(1);
            return true;
        }
        false
    }

    fn enter_parse_depth(&mut self) -> bool {
        if self.depth >= MAX_PARSE_DEPTH {
            self.syntax_error("Domain type nesting limit reached");
            return false;
        }
        self.depth = self.depth.saturating_add(1);
        true
    }

    fn leave_parse_depth(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    fn open(&mut self, kind: SyntaxKind) {
        self.events.push(Event::Open(kind));
    }

    fn close(&mut self) {
        self.events.push(Event::Close);
    }

    fn syntax_error(&mut self, message: &str) {
        self.push_diagnostic(DiagCode::MalformedSyntax, self.current_span(), message);
    }

    fn push_diagnostic(&mut self, code: DiagCode, span: TextRange, message: &str) {
        self.diagnostics.push(
            Diagnostic::builder(code, Severity::Error, message, Label::new(self.file, span))
                .build(),
        );
    }

    fn mark_gap_from(&mut self, start: usize, kind: DomainCoverageGapKind) {
        let span = match self.tokens.get(start) {
            Some((_, range)) => {
                let end = self
                    .tokens
                    .get(self.index.saturating_sub(1))
                    .map_or(range.end(), |(_, end)| end.end());
                TextRange::new(range.start(), end)
            }
            None => self.eof_span(),
        };
        if let Some(previous) = self.coverage_gaps.last_mut()
            && previous.kind == kind
            && previous.span.end() >= span.start()
            && span.end() >= previous.span.start()
        {
            previous.span = TextRange::new(
                previous.span.start().min(span.start()),
                previous.span.end().max(span.end()),
            );
        } else if self
            .coverage_gaps
            .last()
            .is_none_or(|gap| gap.span != span || gap.kind != kind)
        {
            self.coverage_gaps.push(DomainCoverageGap { span, kind });
        }
    }

    fn current_span(&self) -> TextRange {
        self.tokens
            .get(self.index)
            .map_or_else(|| self.eof_span(), |(_, range)| *range)
    }

    fn eof_span(&self) -> TextRange {
        let end = self
            .tokens
            .last()
            .map_or(TextSize::from(0), |(_, range)| range.end());
        TextRange::new(end, end)
    }

    fn text(&self, range: TextRange) -> &str {
        self.source
            .get(usize::from(range.start())..usize::from(range.end()))
            .unwrap_or_default()
    }
}

fn is_trivia(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::WHITESPACE | TokenKind::LINE_COMMENT | TokenKind::BLOCK_COMMENT
    )
}

fn is_name(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::IDENT
            | TokenKind::ALL_KW
            | TokenKind::LET_KW
            | TokenKind::ALL_VERSIONS_KW
            | TokenKind::ALL_VERSIONS_IN_RANGE_KW
            | TokenKind::TO_BYTES_KW
    )
}
