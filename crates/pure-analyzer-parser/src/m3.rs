use pure_analyzer_diagnostics::{
    DiagCode, Diagnostic, FileId, Label, Severity, TextRange, TextSize,
};
use pure_analyzer_lexer::{SyntaxKind as TokenKind, lex};
use pure_analyzer_syntax::{BuildError, Event, GreenNode, GreenNodeBuilder, SyntaxKind};

const LOWEST_PRECEDENCE: u8 = 0;
const EQUALITY_PRECEDENCE: u8 = 1;
const ADDITIVE_PRECEDENCE: u8 = 2;
const MULTIPLICATIVE_PRECEDENCE: u8 = 3;
/// Recursion-depth budget for [`Parser::enter_parse_depth`], bounding how
/// deep `parse_expression` may recurse into itself before recovering
/// instead of overflowing the call stack.
const MAX_RECURSION_DEPTH: usize = 256;
/// Prefix-operator-count budget for `parse_unary_expression`, bounding how
/// many leading `+`/`-` a single unary chain may accumulate.
const MAX_UNARY_OPERATOR_COUNT: usize = 256;
/// Retroactive-wrap budget for `reserve_retroactive_wrap`, bounding how many
/// times one postfix chain may be nested inside itself (each wrap re-anchors
/// the whole prior chain, so this — not recursion depth — is what bounds
/// that particular growth).
const MAX_RETROACTIVE_WRAP_DEPTH: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OpaqueIslandDelimiter {
    Island,
    Brace,
}

/// The result of parsing one source file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parse {
    /// The owned, immutable, lossless concrete syntax tree.
    pub green: GreenNode,
    /// Parser and lexer diagnostics in source order.
    pub diagnostics: Vec<Diagnostic>,
}

/// Parses a modern M3/Relation query into a lossless concrete syntax tree.
///
/// Grammar errors, including incomplete input, remain in the returned
/// [`Parse::diagnostics`] while preserving every source byte in
/// [`Parse::green`]. An error result means the shared syntax builder could not
/// represent the input, such as a source beyond the `TextSize` capacity.
pub fn parse_query(source: &str, file: FileId) -> Result<Parse, BuildError> {
    let tokens = lex(source);
    let (events, diagnostics) = Parser::new(file, &tokens).parse();
    let mut builder = GreenNodeBuilder::new(source, &tokens);
    for event in events {
        builder.push(event);
    }
    builder.finish().map(|green| Parse { green, diagnostics })
}

struct Parser<'tokens> {
    file: FileId,
    tokens: &'tokens [(TokenKind, TextRange)],
    index: usize,
    events: EventStream,
    diagnostics: Vec<Diagnostic>,
    depth: usize,
}

/// Append-friendly parser events with constant-time retroactive wrapping.
///
/// Pratt parsing learns about a binary or postfix wrapper only after it has
/// consumed the expression's left side. A `Vec::insert` at that checkpoint
/// makes a long left-associative chain quadratic, so the parser keeps events
/// in a small linked arena and flattens them once before handing them to the
/// validated green-tree builder.
#[derive(Debug)]
struct EventStream {
    nodes: Vec<EventLink>,
    head: Option<usize>,
    tail: Option<usize>,
}

#[derive(Debug)]
struct EventLink {
    event: Event,
    next: Option<usize>,
}

impl EventStream {
    fn with_capacity(capacity: usize) -> Self {
        Self {
            nodes: Vec::with_capacity(capacity),
            head: None,
            tail: None,
        }
    }

    fn append(&mut self, event: Event) {
        let index = self.nodes.len();
        self.nodes.push(EventLink { event, next: None });
        match self.tail {
            Some(tail) => self.nodes[tail].next = Some(index),
            None => self.head = Some(index),
        }
        self.tail = Some(index);
    }

    fn anchor(&self) -> Option<usize> {
        self.tail
    }

    fn insert_after(&mut self, anchor: usize, event: Event) {
        let index = self.nodes.len();
        let next = self.nodes[anchor].next;
        self.nodes.push(EventLink { event, next });
        self.nodes[anchor].next = Some(index);
        if self.tail == Some(anchor) {
            self.tail = Some(index);
        }
    }

    fn into_events(self) -> Vec<Event> {
        let mut events = Vec::with_capacity(self.nodes.len());
        let mut current = self.head;
        while let Some(index) = current {
            let link = &self.nodes[index];
            events.push(link.event);
            current = link.next;
        }
        events
    }
}

impl<'tokens> Parser<'tokens> {
    fn new(file: FileId, tokens: &'tokens [(TokenKind, TextRange)]) -> Self {
        Self {
            file,
            tokens,
            index: 0,
            events: EventStream::with_capacity(tokens.len().saturating_add(2)),
            diagnostics: Vec::new(),
            depth: 0,
        }
    }

    fn parse(mut self) -> (Vec<Event>, Vec<Diagnostic>) {
        self.open(SyntaxKind::ROOT);
        self.parse_source();
        self.close();
        (self.events.into_events(), self.diagnostics)
    }

    fn parse_source(&mut self) {
        self.consume_trivia();
        while self.index < self.tokens.len() {
            let before = self.index;
            if self.consume_if(TokenKind::SEMICOLON) {
                self.consume_trivia();
                if self.index == before {
                    break;
                }
                continue;
            }

            self.parse_query_expression();
            self.consume_trivia();
            self.consume_source_separator();
            if self.index == before {
                break;
            }
            self.consume_trivia();
        }
    }

    fn consume_source_separator(&mut self) {
        if self.at_eof() || self.consume_if(TokenKind::SEMICOLON) {
            return;
        }
        self.error_current("expected `;` between query expressions");
        self.recover_until(&[TokenKind::SEMICOLON]);
        let _ = self.consume_if(TokenKind::SEMICOLON);
    }

    fn parse_query_expression(&mut self) {
        self.open(SyntaxKind::QUERY_EXPR);
        if !self.parse_expression(LOWEST_PRECEDENCE) {
            self.error_current("expected an expression");
            self.recover_one();
        }
        self.close();
    }

    fn parse_expression(&mut self, minimum_precedence: u8) -> bool {
        if !self.enter_parse_depth() {
            return false;
        }
        let parsed = self.parse_expression_at_depth(minimum_precedence);
        self.leave_parse_depth();
        parsed
    }

    fn parse_expression_at_depth(&mut self, minimum_precedence: u8) -> bool {
        let Some(expression_start) = self.events.anchor() else {
            self.error_current("parser has no containing syntax node");
            return false;
        };
        if !self.parse_unary_expression() {
            return false;
        }

        let mut active_binary_precedence = None;
        while let Some(precedence) = self.binary_precedence() {
            let before = self.index;
            if precedence < minimum_precedence {
                break;
            }
            if active_binary_precedence != Some(precedence) {
                if active_binary_precedence.is_some() {
                    self.close();
                }
                self.wrap_from(expression_start, SyntaxKind::BINARY_EXPR);
                active_binary_precedence = Some(precedence);
            }
            self.consume_trivia();
            let _ = self.bump();
            if !self.parse_expression(precedence.saturating_add(1)) {
                self.error_current("expected an expression after an operator");
                self.recover_until(&EXPRESSION_BOUNDARIES);
            }
            if self.index == before {
                break;
            }
        }
        if active_binary_precedence.is_some() {
            self.close();
        }
        true
    }

    fn parse_unary_expression(&mut self) -> bool {
        self.consume_trivia();
        let mut unary_count = 0;
        while self.at_any(&[TokenKind::PLUS, TokenKind::MINUS]) {
            if unary_count == MAX_UNARY_OPERATOR_COUNT {
                self.error_current("unary-expression nesting limit reached");
                for _ in 0..unary_count {
                    self.close();
                }
                return false;
            }
            self.open(SyntaxKind::UNARY_EXPR);
            let _ = self.bump();
            unary_count = unary_count.saturating_add(1);
        }
        let parsed_operand = self.parse_postfix_expression();
        if !parsed_operand && unary_count > 0 {
            self.error_current("expected an operand after a unary operator");
        }
        for _ in 0..unary_count {
            self.close();
        }
        parsed_operand
    }

    fn parse_postfix_expression(&mut self) -> bool {
        let Some(expression_start) = self.events.anchor() else {
            self.error_current("parser has no containing syntax node");
            return false;
        };
        if !self.parse_primary_expression() {
            return false;
        }

        let mut wrap_depth = 0usize;
        while let Some(kind) = self.significant_kind() {
            let before = self.index;
            match kind {
                TokenKind::DOT if self.dot_starts_all_expression() => {
                    if !self.reserve_retroactive_wrap(&mut wrap_depth) {
                        break;
                    }
                    self.parse_all_expression(expression_start);
                }
                TokenKind::DOT => self.parse_property_navigation(),
                TokenKind::ARROW => self.parse_arrow_call(),
                TokenKind::BRACKET_OPEN => self.parse_bracket_index(),
                TokenKind::PAREN_OPEN => {
                    if !self.reserve_retroactive_wrap(&mut wrap_depth) {
                        break;
                    }
                    self.parse_function_call(expression_start);
                }
                _ => break,
            }
            if self.index == before {
                break;
            }
        }
        true
    }

    fn dot_starts_all_expression(&self) -> bool {
        let Some(dot_index) = self.significant_index() else {
            return false;
        };
        let Some(kind) = self.next_significant_kind(dot_index.saturating_add(1)) else {
            return false;
        };
        matches!(
            kind,
            TokenKind::ALL_KW | TokenKind::ALL_VERSIONS_KW | TokenKind::ALL_VERSIONS_IN_RANGE_KW
        )
    }

    fn parse_all_expression(&mut self, expression_start: usize) {
        self.wrap_from(expression_start, SyntaxKind::ALL_EXPR);
        let _ = self.expect(TokenKind::DOT, "`.` before an all-expression");
        self.consume_trivia();
        match self.raw_kind() {
            Some(TokenKind::ALL_KW | TokenKind::ALL_VERSIONS_IN_RANGE_KW) => {
                let _ = self.bump();
                if self.at(TokenKind::PAREN_OPEN) {
                    self.parse_call_arguments(false);
                } else {
                    self.error_current("expected arguments after an all-expression name");
                }
            }
            // `allVersions` is a property-like generated form, not a call.
            // Leave an immediately following `(` to ordinary postfix parsing:
            // that keeps malformed source lossless without treating its shape
            // as the grammar's canonical form.
            Some(TokenKind::ALL_VERSIONS_KW) => {
                let _ = self.bump();
            }
            _ => self.error_current("expected an all-expression name"),
        }
        self.close();
    }

    fn parse_property_navigation(&mut self) {
        self.open(SyntaxKind::PROPERTY_NAV);
        let _ = self.expect(TokenKind::DOT, "`.` before a property name");
        let _ = self.consume_name("a property name");
        if self.at(TokenKind::PAREN_OPEN) {
            self.parse_call_arguments(false);
        }
        self.close();
    }

    fn parse_arrow_call(&mut self) {
        self.open(SyntaxKind::ARROW_CALL);
        let _ = self.expect(TokenKind::ARROW, "`->`");
        let _ = self.parse_qualified_name();
        if self.at(TokenKind::PAREN_OPEN) {
            self.parse_call_arguments(false);
        } else {
            self.error_current("expected arguments after an arrow-call name");
        }
        self.close();
    }

    fn parse_function_call(&mut self, expression_start: usize) {
        self.wrap_from(expression_start, SyntaxKind::FUNCTION_CALL);
        self.parse_call_arguments(false);
        self.close();
    }

    fn parse_bracket_index(&mut self) {
        self.open(SyntaxKind::BRACKET_INDEX);
        let _ = self.expect(TokenKind::BRACKET_OPEN, "`[` before an index");
        if !self.at(TokenKind::BRACKET_CLOSE) && !self.parse_expression(LOWEST_PRECEDENCE) {
            self.error_current("expected an index expression");
            self.recover_until(&[TokenKind::BRACKET_CLOSE, TokenKind::SEMICOLON]);
        }
        let _ = self.expect(TokenKind::BRACKET_CLOSE, "`]` after an index");
        self.close();
    }

    fn parse_primary_expression(&mut self) -> bool {
        self.consume_trivia();
        let Some(kind) = self.raw_kind() else {
            return false;
        };
        match kind {
            TokenKind::DOLLAR => self.parse_variable_expression(),
            TokenKind::INTEGER
            | TokenKind::BOOLEAN
            | TokenKind::STRING
            | TokenKind::DATE_TIME
            | TokenKind::STRICT_DATE
            | TokenKind::LATEST_DATE => self.parse_literal_expression(),
            TokenKind::PAREN_OPEN => self.parse_parenthesized_expression(),
            TokenKind::BRACKET_OPEN => self.parse_collection_literal(),
            TokenKind::BRACE_OPEN => self.parse_braced_lambda(),
            TokenKind::PIPE => self.parse_parameterless_lambda(),
            TokenKind::TILDE => self.parse_column_builder(),
            TokenKind::NEW_SYMBOL => self.parse_new_instance_expression(),
            TokenKind::AT => self.parse_cast_expression(),
            TokenKind::HASH_STORE_OPEN
            | TokenKind::HASH_ISLAND_OPEN
            | TokenKind::NAV_PATH_BLOCK
            | TokenKind::HASH => self.parse_island(),
            TokenKind::ERROR => self.parse_bad_token(),
            kind if is_name(kind) && self.is_short_lambda_start() => self.parse_short_lambda(),
            kind if is_qualified_name_start(kind) => self.parse_qualified_name(),
            _ => false,
        }
    }

    fn parse_variable_expression(&mut self) -> bool {
        self.open(SyntaxKind::VARIABLE_EXPR);
        let has_dollar = self.expect(TokenKind::DOLLAR, "`$` before a variable name");
        let has_name = self.consume_name("a variable name");
        self.close();
        has_dollar && has_name
    }

    fn parse_literal_expression(&mut self) -> bool {
        self.open(SyntaxKind::LITERAL_EXPR);
        let consumed = self.bump();
        self.close();
        consumed
    }

    fn parse_parenthesized_expression(&mut self) -> bool {
        self.open(SyntaxKind::PAREN_EXPR);
        let has_open = self.expect(TokenKind::PAREN_OPEN, "`(` before an expression");
        self.parse_parenthesized_items();
        let has_close = self.expect(TokenKind::PAREN_CLOSE, "`)` after an expression");
        self.close();
        has_open && has_close
    }

    fn parse_parenthesized_items(&mut self) {
        if self.at(TokenKind::PAREN_CLOSE) {
            return;
        }
        loop {
            let before = self.index;
            if !self.parse_expression(LOWEST_PRECEDENCE) {
                self.error_current("expected an expression inside parentheses");
                self.recover_until(&[
                    TokenKind::COMMA,
                    TokenKind::PAREN_CLOSE,
                    TokenKind::SEMICOLON,
                ]);
            }
            let has_comma = self.consume_if(TokenKind::COMMA);
            if self.index == before || !has_comma {
                break;
            }
            if self.at(TokenKind::PAREN_CLOSE) {
                self.error_current("expected an expression after `,`");
                break;
            }
        }
    }

    fn parse_collection_literal(&mut self) -> bool {
        self.open(SyntaxKind::COLLECTION_LITERAL);
        let _ = self.expect(TokenKind::BRACKET_OPEN, "`[` before a collection literal");
        self.parse_collection_items();
        let has_close = self.expect(TokenKind::BRACKET_CLOSE, "`]` after a collection literal");
        self.close();
        has_close
    }

    fn parse_collection_items(&mut self) {
        if self.at(TokenKind::BRACKET_CLOSE) {
            return;
        }
        loop {
            let before = self.index;
            if !self.parse_expression(LOWEST_PRECEDENCE) {
                self.error_current("expected an expression inside a collection literal");
                self.recover_until(&[
                    TokenKind::COMMA,
                    TokenKind::BRACKET_CLOSE,
                    TokenKind::SEMICOLON,
                ]);
            }
            self.consume_trivia();
            if self.at(TokenKind::SEMICOLON) {
                break;
            }

            let has_comma = self.consume_if(TokenKind::COMMA);
            if self.index == before {
                break;
            }
            if has_comma {
                if self.at(TokenKind::BRACKET_CLOSE) {
                    self.error_current("expected an expression after `,`");
                    break;
                }
                continue;
            }
            if self.at(TokenKind::BRACKET_CLOSE) || self.at_eof() {
                break;
            }

            self.error_current("expected `,` or `]` after a collection item");
            let recovery_start = self.index;
            self.recover_until(&[
                TokenKind::COMMA,
                TokenKind::BRACKET_CLOSE,
                TokenKind::SEMICOLON,
            ]);
            if self.at(TokenKind::SEMICOLON) || self.at(TokenKind::BRACKET_CLOSE) || self.at_eof() {
                break;
            }
            if self.index == recovery_start {
                break;
            }
            let _ = self.consume_if(TokenKind::COMMA);
            if self.at(TokenKind::BRACKET_CLOSE) {
                self.error_current("expected an expression after `,`");
                break;
            }
        }
    }

    fn parse_braced_lambda(&mut self) -> bool {
        self.open(SyntaxKind::LAMBDA_EXPR);
        let _ = self.expect(TokenKind::BRACE_OPEN, "`{` before a lambda");
        self.parse_lambda_parameters();
        let has_pipe = self.expect(TokenKind::PIPE, "`|` after lambda parameters");
        self.parse_code_block();
        let has_close = self.expect(TokenKind::BRACE_CLOSE, "`}` after a lambda body");
        self.close();
        has_pipe && has_close
    }

    fn parse_short_lambda(&mut self) -> bool {
        self.open(SyntaxKind::LAMBDA_EXPR);
        self.open(SyntaxKind::LAMBDA_PARAMS);
        let _ = self.consume_name("a lambda parameter");
        self.parse_optional_parameter_type();
        self.close();
        let has_pipe = self.expect(TokenKind::PIPE, "`|` after a lambda parameter");
        self.parse_code_block();
        self.close();
        has_pipe
    }

    fn parse_parameterless_lambda(&mut self) -> bool {
        self.open(SyntaxKind::LAMBDA_EXPR);
        let has_pipe = self.expect(TokenKind::PIPE, "`|` before lambda body");
        self.parse_code_block();
        self.close();
        has_pipe
    }

    fn parse_lambda_parameters(&mut self) {
        if self.at(TokenKind::PIPE) {
            return;
        }
        self.open(SyntaxKind::LAMBDA_PARAMS);
        loop {
            let before = self.index;
            let parsed_parameter = self.consume_name("a lambda parameter");
            self.parse_optional_parameter_type();
            let has_comma = self.consume_if(TokenKind::COMMA);
            if self.index == before {
                break;
            }
            if !parsed_parameter || !has_comma {
                break;
            }
        }
        self.close();
    }

    fn parse_optional_parameter_type(&mut self) {
        if self.consume_if(TokenKind::COLON) {
            let _ = self.parse_type_reference();
            self.parse_optional_multiplicity();
        }
    }

    fn parse_code_block(&mut self) {
        self.open(SyntaxKind::CODE_BLOCK);
        while !self.at_code_block_boundary() {
            let before = self.index;
            if self.at(TokenKind::LET_KW) {
                self.parse_let_statement();
            } else {
                self.parse_query_expression();
            }
            self.consume_trivia();
            let has_separator = self.consume_if(TokenKind::SEMICOLON);
            if self.index == before {
                break;
            }
            if !has_separator {
                break;
            }
            self.consume_trivia();
        }
        self.close();
    }

    fn parse_let_statement(&mut self) {
        self.open(SyntaxKind::LET_STMT);
        let _ = self.expect(TokenKind::LET_KW, "`let`");
        let _ = self.consume_name("a binding name");
        let _ = self.expect(TokenKind::ASSIGN, "`=` after a binding name");
        if !self.parse_expression(LOWEST_PRECEDENCE) {
            self.error_current("expected an expression after `=`");
            self.recover_until(&[
                TokenKind::SEMICOLON,
                TokenKind::BRACE_CLOSE,
                TokenKind::COMMA,
                TokenKind::PAREN_CLOSE,
                TokenKind::BRACKET_CLOSE,
            ]);
        }
        self.close();
    }

    fn parse_column_builder(&mut self) -> bool {
        if self.next_significant_is(TokenKind::BRACKET_OPEN) {
            self.parse_column_spec_array()
        } else {
            self.parse_column_spec(true)
        }
    }

    fn parse_column_spec_array(&mut self) -> bool {
        self.open(SyntaxKind::COLUMN_SPEC_ARRAY);
        let _ = self.expect(TokenKind::TILDE, "`~` before a column specification");
        let _ = self.expect(TokenKind::BRACKET_OPEN, "`[` after `~`");
        self.parse_column_spec_array_items();
        let has_close = self.expect(TokenKind::BRACKET_CLOSE, "`]` after column specifications");
        self.close();
        // A source separator is a safe outer recovery boundary.  Treat the
        // malformed array as a completed primary expression here so the
        // source parser, rather than generic expression recovery, consumes
        // the `;` and retains the next query.
        has_close || self.at(TokenKind::SEMICOLON)
    }

    fn parse_column_spec_array_items(&mut self) {
        if self.at(TokenKind::BRACKET_CLOSE) {
            return;
        }

        loop {
            let before = self.index;
            let _ = self.parse_column_spec(false);
            self.consume_trivia();
            if self.at(TokenKind::SEMICOLON) {
                break;
            }

            let has_comma = self.consume_if(TokenKind::COMMA);
            if self.index == before {
                self.recover_until(&[
                    TokenKind::COMMA,
                    TokenKind::BRACKET_CLOSE,
                    TokenKind::SEMICOLON,
                ]);
                // `recover_until` deliberately leaves its boundary in place.
                // Only a comma starts another member; every other boundary
                // belongs to the enclosing grammar.
                match self.significant_kind() {
                    Some(TokenKind::COMMA) => {}
                    _ => break,
                }
                let comma_index = self.index;
                let recovered_comma = self.consume_if(TokenKind::COMMA);
                match (recovered_comma, self.index.cmp(&comma_index)) {
                    (true, std::cmp::Ordering::Greater) => {}
                    _ => break,
                }
                if self.at(TokenKind::BRACKET_CLOSE) || self.at_eof() {
                    self.error_current("expected a column specification after `,`");
                    break;
                }
                continue;
            }
            if has_comma {
                if self.at(TokenKind::BRACKET_CLOSE) || self.at_eof() {
                    self.error_current("expected a column specification after `,`");
                    break;
                }
                continue;
            }
            if self.at(TokenKind::BRACKET_CLOSE) || self.at_eof() {
                break;
            }

            self.error_current("expected `,` or `]` after a column specification");
            let recovery_start = self.index;
            self.recover_until(&[
                TokenKind::COMMA,
                TokenKind::BRACKET_CLOSE,
                TokenKind::SEMICOLON,
            ]);
            if self.at(TokenKind::SEMICOLON) || self.at(TokenKind::BRACKET_CLOSE) || self.at_eof() {
                break;
            }
            if self.index == recovery_start {
                break;
            }
            let _ = self.consume_if(TokenKind::COMMA);
            if self.at(TokenKind::BRACKET_CLOSE) || self.at_eof() {
                self.error_current("expected a column specification after `,`");
                break;
            }
        }
    }

    fn parse_column_spec(&mut self, has_tilde: bool) -> bool {
        self.open(SyntaxKind::COLUMN_SPEC);
        if has_tilde {
            let _ = self.expect(TokenKind::TILDE, "`~` before a column name");
        }
        let name = self.parse_column_name();
        if self.consume_if(TokenKind::COLON) {
            self.parse_column_spec_body();
        }
        self.close();
        name
    }

    fn parse_column_name(&mut self) -> bool {
        self.consume_trivia();
        let name = self
            .raw_kind()
            .is_some_and(|kind| is_name(kind) || kind == TokenKind::STRING);
        if !name {
            self.error_current("expected a column name");
            return false;
        }

        self.open(SyntaxKind::COLUMN_NAME);
        let _ = self.bump();
        self.close();
        true
    }

    fn parse_column_spec_body(&mut self) {
        if self.is_lambda_body_start() {
            let _ = self.parse_primary_expression();
        } else {
            let _ = self.parse_type_reference();
            self.parse_optional_multiplicity();
        }
        if self.consume_if(TokenKind::COLON) {
            if self.is_lambda_body_start() {
                let _ = self.parse_primary_expression();
            } else {
                self.error_current("expected a lambda after `:` in a column specification");
            }
        }
    }

    fn parse_new_instance_expression(&mut self) -> bool {
        self.open(SyntaxKind::NEW_INSTANCE_EXPR);
        let _ = self.expect(TokenKind::NEW_SYMBOL, "`^` before an instance type");
        let has_target = if self.at(TokenKind::DOLLAR) {
            self.parse_variable_expression()
        } else {
            self.parse_qualified_name()
        };
        if self.at(TokenKind::PAREN_OPEN) {
            self.parse_call_arguments(true);
        }
        self.close();
        has_target
    }

    fn parse_cast_expression(&mut self) -> bool {
        self.open(SyntaxKind::CAST_EXPR);
        let _ = self.expect(TokenKind::AT, "`@` before a cast type");
        let has_type = self.parse_type_reference();
        self.close();
        has_type
    }

    fn parse_qualified_name(&mut self) -> bool {
        self.open(SyntaxKind::QUALIFIED_NAME);
        let _ = self.consume_if(TokenKind::PATH_SEPARATOR);
        let mut valid = self.consume_name("a name");
        loop {
            let before = self.index;
            let has_separator = self.consume_if(TokenKind::PATH_SEPARATOR);
            if self.index == before || !has_separator {
                break;
            }
            valid = self.consume_name("a name after `::`");
            if !valid {
                break;
            }
        }
        self.close();
        valid
    }

    fn parse_type_reference(&mut self) -> bool {
        if !self.enter_parse_depth() {
            return false;
        }
        self.open(SyntaxKind::TYPE_REF);
        let mut valid = self.parse_qualified_name();
        if self.consume_if(TokenKind::LT) {
            self.parse_type_arguments();
            valid = self.expect(TokenKind::GT, "`>` after type arguments") && valid;
        }
        self.close();
        self.leave_parse_depth();
        valid
    }

    fn parse_type_arguments(&mut self) {
        if self.at(TokenKind::PAREN_OPEN) {
            self.parse_relation_type();
            return;
        }
        loop {
            let before = self.index;
            let _ = self.parse_type_reference();
            let has_comma = self.consume_if(TokenKind::COMMA);
            if self.index == before || !has_comma {
                break;
            }
        }
    }

    fn parse_relation_type(&mut self) {
        self.open(SyntaxKind::RELATION_TYPE);
        let _ = self.expect(TokenKind::PAREN_OPEN, "`(` before relation columns");
        if !self.at(TokenKind::PAREN_CLOSE) {
            loop {
                let before = self.index;
                self.parse_column_info();
                let has_comma = self.consume_if(TokenKind::COMMA);
                if self.index == before || !has_comma {
                    break;
                }
            }
        }
        let _ = self.expect(TokenKind::PAREN_CLOSE, "`)` after relation columns");
        self.close();
    }

    fn parse_column_info(&mut self) {
        self.open(SyntaxKind::COLUMN_INFO);
        let _ = self.consume_name("a relation column name");
        let _ = self.expect(TokenKind::COLON, "`:` after a relation column name");
        let _ = self.parse_type_reference();
        self.parse_optional_multiplicity();
        self.close();
    }

    fn parse_optional_multiplicity(&mut self) {
        if self.at(TokenKind::BRACKET_OPEN) {
            self.parse_multiplicity();
        }
    }

    fn parse_multiplicity(&mut self) {
        self.open(SyntaxKind::MULTIPLICITY);
        let _ = self.expect(TokenKind::BRACKET_OPEN, "`[` before a multiplicity");
        self.consume_multiplicity_bound();
        if self.consume_if(TokenKind::DOT) {
            let _ = self.expect(TokenKind::DOT, "a second `.` in a multiplicity range");
            self.consume_multiplicity_bound();
        }
        let _ = self.expect(TokenKind::BRACKET_CLOSE, "`]` after a multiplicity");
        self.close();
    }

    fn consume_multiplicity_bound(&mut self) {
        if !self.consume_if(TokenKind::INTEGER) && !self.consume_if(TokenKind::STAR) {
            self.error_current("expected a multiplicity bound");
        }
    }

    fn parse_call_arguments(&mut self, allow_named: bool) {
        self.open(SyntaxKind::CALL_ARGS);
        let has_open = self.expect(TokenKind::PAREN_OPEN, "`(` before arguments");
        if has_open {
            self.parse_argument_list(allow_named);
        }
        let _ = self.expect(TokenKind::PAREN_CLOSE, "`)` after arguments");
        self.close();
    }

    fn parse_argument_list(&mut self, allow_named: bool) {
        if self.at(TokenKind::PAREN_CLOSE) {
            return;
        }
        loop {
            let before = self.index;
            self.parse_argument(allow_named);
            self.consume_trivia();
            if self.at(TokenKind::SEMICOLON) {
                break;
            }
            let has_comma = self.consume_if(TokenKind::COMMA);
            if self.index == before {
                break;
            }
            if has_comma {
                if self.at(TokenKind::PAREN_CLOSE) {
                    self.error_current("expected an argument after `,`");
                    break;
                }
                continue;
            }
            if self.at(TokenKind::PAREN_CLOSE) || self.at_eof() {
                break;
            }
            self.error_current("expected `,` or `)` after an argument");
            self.recover_until(&[
                TokenKind::COMMA,
                TokenKind::PAREN_CLOSE,
                TokenKind::SEMICOLON,
            ]);
            if self.at(TokenKind::SEMICOLON) {
                break;
            }
            if self.index == before {
                break;
            }
        }
    }

    fn parse_argument(&mut self, allow_named: bool) {
        if allow_named && self.is_named_argument_start() {
            let _ = self.consume_name("an argument name");
            let _ = self.expect(TokenKind::ASSIGN, "`=` after an argument name");
        }
        if !self.parse_expression(LOWEST_PRECEDENCE) {
            self.error_current("expected an argument expression");
            self.recover_until(&[
                TokenKind::COMMA,
                TokenKind::PAREN_CLOSE,
                TokenKind::SEMICOLON,
            ]);
        }
    }

    fn is_named_argument_start(&self) -> bool {
        let Some(name_index) = self.significant_index() else {
            return false;
        };
        let Some((kind, _)) = self.tokens.get(name_index) else {
            return false;
        };
        is_name(*kind)
            && self.next_significant_is_from(name_index.saturating_add(1), TokenKind::ASSIGN)
    }

    fn parse_island(&mut self) -> bool {
        match self.raw_kind() {
            Some(TokenKind::HASH_STORE_OPEN) => self.parse_store_table_pointer(),
            Some(TokenKind::NAV_PATH_BLOCK) => self.parse_navigation_path_island(),
            Some(TokenKind::HASH_ISLAND_OPEN) => self.parse_braced_opaque_island(),
            Some(TokenKind::HASH) => self.parse_hash_opaque_island(),
            _ => false,
        }
    }

    fn parse_store_table_pointer(&mut self) -> bool {
        self.open(SyntaxKind::ISLAND);
        self.open(SyntaxKind::STORE_TABLE_POINTER);
        let _ = self.expect(TokenKind::HASH_STORE_OPEN, "`#>{` before a table pointer");
        let has_path = self.parse_qualified_name();
        let has_dot = self.expect(TokenKind::DOT, "`.` between a database and table name");
        let has_table = self.consume_name("a table name");
        let has_close = self.consume_if(TokenKind::ISLAND_END);
        if !has_close {
            self.unterminated_island();
            self.recover_island_end();
        }
        self.close();
        self.close();
        has_path && has_dot && has_table && has_close
    }

    fn parse_navigation_path_island(&mut self) -> bool {
        self.open(SyntaxKind::ISLAND);
        self.open(SyntaxKind::NAV_PATH_ISLAND);
        let consumed = self.bump();
        self.close();
        self.close();
        consumed
    }

    fn parse_braced_opaque_island(&mut self) -> bool {
        self.open(SyntaxKind::ISLAND);
        self.open(SyntaxKind::OPAQUE_ISLAND);
        let _ = self.bump();
        let mut delimiters = vec![OpaqueIslandDelimiter::Island];
        loop {
            if self.at_eof() {
                break;
            }
            let before = self.index;
            match self.raw_kind() {
                Some(TokenKind::HASH_ISLAND_OPEN | TokenKind::HASH_STORE_OPEN) => {
                    delimiters.push(OpaqueIslandDelimiter::Island);
                    let _ = self.bump();
                }
                Some(TokenKind::BRACE_OPEN) => {
                    delimiters.push(OpaqueIslandDelimiter::Brace);
                    let _ = self.bump();
                }
                Some(TokenKind::BRACE_CLOSE)
                    if delimiters.last() == Some(&OpaqueIslandDelimiter::Brace) =>
                {
                    let _ = delimiters.pop();
                    let _ = self.bump();
                }
                Some(TokenKind::ISLAND_END)
                    if delimiters.last() == Some(&OpaqueIslandDelimiter::Island) =>
                {
                    let _ = delimiters.pop();
                    let _ = self.bump();
                    if delimiters.is_empty() {
                        self.close();
                        self.close();
                        return true;
                    }
                }
                _ => {
                    let _ = self.bump();
                }
            }
            if self.index == before {
                break;
            }
        }
        self.unterminated_island();
        self.close();
        self.close();
        false
    }

    fn parse_hash_opaque_island(&mut self) -> bool {
        self.open(SyntaxKind::ISLAND);
        self.open(SyntaxKind::OPAQUE_ISLAND);
        let _ = self.bump();
        loop {
            if self.at_eof() {
                break;
            }
            let before = self.index;
            if self.raw_kind() == Some(TokenKind::HASH)
                || self.raw_kind() == Some(TokenKind::ISLAND_END)
            {
                let _ = self.bump();
                self.close();
                self.close();
                return true;
            }
            let _ = self.bump();
            if self.index == before {
                break;
            }
        }
        self.unterminated_island();
        self.close();
        self.close();
        false
    }

    fn unterminated_island(&mut self) {
        self.push_diagnostic(
            DiagCode::UnterminatedIsland,
            self.current_span(),
            "unterminated island",
        );
        self.open(SyntaxKind::ERROR_NODE);
        self.close();
    }

    fn recover_island_end(&mut self) {
        self.open(SyntaxKind::ERROR_NODE);
        while !self.at_eof() && !self.at_any(&[TokenKind::ISLAND_END, TokenKind::SEMICOLON]) {
            let before = self.index;
            let _ = self.bump();
            if self.index == before {
                break;
            }
        }
        let _ = self.consume_if(TokenKind::ISLAND_END);
        self.close();
    }

    fn parse_bad_token(&mut self) -> bool {
        self.open(SyntaxKind::ERROR_NODE);
        let consumed = self.bump();
        self.close();
        consumed
    }

    fn binary_precedence(&self) -> Option<u8> {
        match self.significant_kind() {
            Some(
                TokenKind::EQ
                | TokenKind::NEQ
                | TokenKind::LE
                | TokenKind::LT
                | TokenKind::GE
                | TokenKind::GT,
            ) => Some(EQUALITY_PRECEDENCE),
            Some(TokenKind::PLUS | TokenKind::MINUS) => Some(ADDITIVE_PRECEDENCE),
            Some(TokenKind::STAR | TokenKind::SLASH) => Some(MULTIPLICATIVE_PRECEDENCE),
            _ => None,
        }
    }

    fn recover_one(&mut self) {
        if self.at_eof() {
            self.open(SyntaxKind::ERROR_NODE);
            self.close();
            return;
        }
        self.open(SyntaxKind::ERROR_NODE);
        let _ = self.bump();
        self.close();
    }

    fn recover_until(&mut self, boundaries: &[TokenKind]) {
        self.open(SyntaxKind::ERROR_NODE);
        loop {
            if self.at_eof() {
                break;
            }
            if self.at_any(boundaries) {
                break;
            }
            let current = self.index;
            let _ = self.bump();
            if self.index == current {
                break;
            }
        }
        self.close();
    }

    fn expect(&mut self, kind: TokenKind, expected: &str) -> bool {
        self.consume_trivia();
        if self.raw_kind() == Some(kind) {
            return self.bump();
        }
        self.error_current(&format!("expected {expected}"));
        false
    }

    fn consume_if(&mut self, kind: TokenKind) -> bool {
        self.consume_trivia();
        if self.raw_kind() == Some(kind) {
            return self.bump();
        }
        false
    }

    fn consume_name(&mut self, expected: &str) -> bool {
        self.consume_trivia();
        if self.raw_kind().is_some_and(is_name) {
            return self.bump();
        }
        self.error_current(&format!("expected {expected}"));
        false
    }

    fn consume_trivia(&mut self) {
        while self.raw_kind().is_some_and(is_trivia) {
            let before = self.index;
            if !self.bump() {
                break;
            }
            if self.index == before {
                break;
            }
        }
    }

    fn at(&self, kind: TokenKind) -> bool {
        self.significant_kind() == Some(kind)
    }

    fn at_any(&self, kinds: &[TokenKind]) -> bool {
        self.significant_kind()
            .is_some_and(|kind| kinds.contains(&kind))
    }

    fn at_code_block_boundary(&self) -> bool {
        if self.at_eof() {
            return true;
        }
        self.at_any(&CODE_BLOCK_BOUNDARIES)
    }

    fn next_significant_is(&self, kind: TokenKind) -> bool {
        let Some(index) = self.significant_index() else {
            return false;
        };
        self.next_significant_is_from(index.saturating_add(1), kind)
    }

    fn next_significant_is_from(&self, start: usize, kind: TokenKind) -> bool {
        self.next_significant_kind(start) == Some(kind)
    }

    fn is_short_lambda_start(&self) -> bool {
        let Some(index) = self.significant_index() else {
            return false;
        };
        if !self
            .tokens
            .get(index)
            .is_some_and(|(kind, _)| is_name(*kind))
        {
            return false;
        }

        let Some(next) = self.next_significant_index_from(index.saturating_add(1)) else {
            return false;
        };
        match self.tokens[next].0 {
            TokenKind::PIPE => true,
            TokenKind::COLON => self.typed_short_lambda_has_pipe(next.saturating_add(1)),
            _ => false,
        }
    }

    fn is_lambda_body_start(&self) -> bool {
        if matches!(
            self.significant_kind(),
            Some(TokenKind::BRACE_OPEN | TokenKind::PIPE)
        ) {
            return true;
        }
        self.is_short_lambda_start()
    }

    /// Determines whether the type annotation after a single lambda parameter
    /// ends at a top-level pipe without committing parser state. Type grammar
    /// is parsed for real by [`Self::parse_optional_parameter_type`] once this
    /// lookahead recognizes the lambda; this scan only tracks nested delimiters
    /// so commas in `Relation<(name:String[1])>` do not terminate it early.
    fn typed_short_lambda_has_pipe(&self, start: usize) -> bool {
        let mut index = start;
        let mut angles = 0_usize;
        let mut brackets = 0_usize;

        while let Some(next) = self.next_significant_index_from(index) {
            let kind = self.tokens[next].0;
            let at_top_level = angles == 0 && brackets == 0;
            match kind {
                TokenKind::PIPE if at_top_level => return true,
                TokenKind::LT => angles = angles.saturating_add(1),
                TokenKind::GT => angles = angles.saturating_sub(1),
                TokenKind::BRACKET_OPEN => brackets = brackets.saturating_add(1),
                TokenKind::BRACKET_CLOSE if brackets != 0 => {
                    brackets = brackets.saturating_sub(1);
                }
                TokenKind::COMMA
                | TokenKind::SEMICOLON
                | TokenKind::BRACE_CLOSE
                | TokenKind::PAREN_CLOSE
                | TokenKind::BRACKET_CLOSE
                    if at_top_level =>
                {
                    return false;
                }
                _ => {}
            }
            index = next.saturating_add(1);
        }
        false
    }

    fn significant_kind(&self) -> Option<TokenKind> {
        self.significant_index()
            .and_then(|index| self.tokens.get(index).map(|(kind, _)| *kind))
    }

    fn next_significant_kind(&self, start: usize) -> Option<TokenKind> {
        self.next_significant_index_from(start)
            .map(|index| self.tokens[index].0)
    }

    fn significant_index(&self) -> Option<usize> {
        self.next_significant_index_from(self.index)
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

    fn at_eof(&self) -> bool {
        self.index >= self.tokens.len()
    }

    fn bump(&mut self) -> bool {
        if self.index >= self.tokens.len() {
            return false;
        }
        if self.raw_kind() == Some(TokenKind::ERROR) {
            self.push_diagnostic(
                DiagCode::BadToken,
                self.current_span(),
                "unrecognized token",
            );
        }
        self.events.append(Event::Advance);
        self.index = self.index.saturating_add(1);
        true
    }

    fn enter_parse_depth(&mut self) -> bool {
        if self.depth >= MAX_RECURSION_DEPTH {
            self.error_current("parser nesting limit reached");
            return false;
        }
        self.depth = self.depth.saturating_add(1);
        true
    }

    fn leave_parse_depth(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    fn open(&mut self, kind: SyntaxKind) {
        self.events.append(Event::Open(kind));
    }

    fn close(&mut self) {
        self.events.append(Event::Close);
    }

    /// Reserves one retroactive wrap in the postfix chain currently being
    /// parsed. `wrap_depth` is local to a single [`Self::parse_postfix_expression`]
    /// call: each wrap in that chain nests the whole prior chain inside the
    /// new node (`wrap_from` retroactively inserts the new `Open` right
    /// before the chain's start), so consecutive wraps genuinely deepen the
    /// resulting tree and must share one budget. Sibling chains — separate
    /// primaries such as list elements or call arguments — start their own
    /// `parse_postfix_expression` call and so their own fresh budget; only
    /// real recursion into a new expression (bounded by
    /// [`Self::enter_parse_depth`]) links one chain's depth to another's.
    fn reserve_retroactive_wrap(&mut self, wrap_depth: &mut usize) -> bool {
        if *wrap_depth >= MAX_RETROACTIVE_WRAP_DEPTH {
            self.error_current("expression nesting limit reached");
            return false;
        }
        *wrap_depth = wrap_depth.saturating_add(1);
        true
    }

    fn wrap_from(&mut self, anchor: usize, kind: SyntaxKind) {
        self.events.insert_after(anchor, Event::Open(kind));
    }

    fn error_current(&mut self, message: &str) {
        self.push_diagnostic(DiagCode::MalformedSyntax, self.current_span(), message);
    }

    fn push_diagnostic(&mut self, code: DiagCode, span: TextRange, message: &str) {
        self.diagnostics.push(
            Diagnostic::builder(code, Severity::Error, message, Label::new(self.file, span))
                .build(),
        );
    }

    fn current_span(&self) -> TextRange {
        match self.tokens.get(self.index) {
            Some((_, range)) => *range,
            None => self.eof_span(),
        }
    }

    fn eof_span(&self) -> TextRange {
        let end = match self.tokens.last() {
            Some((_, range)) => range.end(),
            None => TextSize::from(0),
        };
        TextRange::new(end, end)
    }
}

const EXPRESSION_BOUNDARIES: [TokenKind; 5] = [
    TokenKind::COMMA,
    TokenKind::PAREN_CLOSE,
    TokenKind::BRACKET_CLOSE,
    TokenKind::BRACE_CLOSE,
    TokenKind::SEMICOLON,
];

const CODE_BLOCK_BOUNDARIES: [TokenKind; 4] = [
    TokenKind::BRACE_CLOSE,
    TokenKind::COMMA,
    TokenKind::PAREN_CLOSE,
    TokenKind::BRACKET_CLOSE,
];

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

fn is_qualified_name_start(kind: TokenKind) -> bool {
    kind == TokenKind::PATH_SEPARATOR || is_name(kind)
}
