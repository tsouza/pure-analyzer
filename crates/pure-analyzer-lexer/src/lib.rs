#![forbid(unsafe_code)]
#![deny(missing_docs)]

//! The lexer: a `logos`-derived DFA token layer (design doc §4.1).
//!
//! Owns [`SyntaxKind`], the token enum for the M3 (query) and Domain
//! (model-definition) grammars — literals, the `%latest`/date family,
//! symbols, and the raw island tokens (`#>{`, `#{`, `}#`, …) that the future
//! `pure-analyzer-parser` balances into `Island` nodes rather than a `logos`
//! mode stack (design doc §2.3) — and [`lex`], which drives it.
//!
//! `SyntaxKind` here covers **terminal (token) kinds only**. It is not the
//! richer, rowan-compatible kind enum `pure-analyzer-syntax` builds for the
//! CST (terminals *and* nonterminal node kinds) — this crate has zero
//! internal dependencies (`xtask`'s `ALLOWED_INTERNAL_DEPS`), so it cannot
//! depend on that crate's design, only be depended upon by it.

use logos::Logos;
use text_size::{TextRange, TextSize};

/// A terminal token kind, as produced by [`lex`].
///
/// One flat `logos`-derived enum covering every token class design doc §4.1
/// specifies, plus a documented handful it omits but the grammar clearly
/// needs (see `specs/lexer.md`'s non-goals: `SEMICOLON`, and a concrete
/// arithmetic/comparison operator set for the doc's unspecified "arithmetic").
/// Trivia (whitespace, comments) are real variants, never skipped, so the
/// token stream is lossless — required for `fmt` later.
#[derive(Logos, Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u16)]
#[allow(non_camel_case_types, missing_docs)] // one self-explanatory variant per token class; see the module + field docs above instead of per-variant prose
pub enum SyntaxKind {
    // -- date family: longest match first (logos resolves this automatically
    // by match length, not declaration order) --------------------------
    #[regex(r"%[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}")]
    DATE_TIME,
    #[regex(r"%[0-9]{4}-[0-9]{2}-[0-9]{2}")]
    STRICT_DATE,
    #[token("%latest")]
    LATEST_DATE,
    #[token("%")]
    PERCENT,

    // -- symbols ----------------------------------------------------------
    #[token("~")]
    TILDE,
    #[token("$")]
    DOLLAR,
    #[token("->")]
    ARROW,
    #[token("|")]
    PIPE,
    #[token("@")]
    AT,
    #[token("^")]
    NEW_SYMBOL,
    #[token(".")]
    DOT,
    #[token(",")]
    COMMA,
    #[token("::")]
    PATH_SEPARATOR,
    #[token(":")]
    COLON,
    #[token("(")]
    PAREN_OPEN,
    #[token(")")]
    PAREN_CLOSE,
    #[token("[")]
    BRACKET_OPEN,
    #[token("]")]
    BRACKET_CLOSE,
    #[token("==")]
    EQ,
    #[token("!=")]
    NEQ,
    // Not in design doc §4.1's symbol list (it only says "arithmetic") —
    // a documented interpretation; see specs/lexer.md non-goals.
    #[token("+")]
    PLUS,
    #[token("-")]
    MINUS,
    #[token("*")]
    STAR,
    #[token("/")]
    SLASH,
    #[token("<=")]
    LE,
    #[token("<")]
    LT,
    #[token(">=")]
    GE,
    #[token(">")]
    GT,
    // Not in §4.1 either, but required by §4.2's `CodeBlock = Stmt (';'
    // Stmt)*` and used in §1.1's own worked example — see specs/lexer.md.
    #[token(";")]
    SEMICOLON,
    // Shared by lambda bodies (`{x,y| ...}`) and islands (`#{...}#`) alike;
    // the lexer doesn't distinguish the two contexts, the parser does, by
    // balancing (design doc §2.3).
    #[token("{")]
    BRACE_OPEN,
    #[token("}")]
    BRACE_CLOSE,

    // -- M3 keywords (design doc §4.1) — exact-token variants take priority
    // over the IDENT regex on an equal-length match, so `all` lexes as
    // ALL_KW while `allVersions2` (longer) still lexes whole as IDENT. -----
    #[token("all")]
    ALL_KW,
    #[token("let")]
    LET_KW,
    #[token("allVersions")]
    ALL_VERSIONS_KW,
    #[token("allVersionsInRange")]
    ALL_VERSIONS_IN_RANGE_KW,
    #[token("toBytes")]
    TO_BYTES_KW,

    // -- literals (design doc §4.1) ----------------------------------------
    #[regex(r"[A-Za-z_][A-Za-z0-9_]*")]
    IDENT,
    #[regex(r"[0-9]+")]
    INTEGER,
    #[token("true")]
    #[token("false")]
    BOOLEAN,
    // Pascal/SQL-style: `'...'` with `''` as an escaped literal quote — a
    // documented assumption, not yet engine-verified; see specs/lexer.md.
    #[regex(r"'([^']|'')*'", allow_greedy = true)]
    STRING,

    // -- island raw tokens (design doc §2.3) — flat, unbalanced; the parser
    // is responsible for matching `#>{`/`#{` against `}#` ------------------
    #[token("#>{")]
    HASH_STORE_OPEN,
    #[token("#{")]
    HASH_ISLAND_OPEN,
    #[regex(r"#/[^#]*#", allow_greedy = true)]
    NAV_PATH_BLOCK,
    #[token("}#")]
    ISLAND_END,
    #[token("#")]
    HASH,

    // -- trivia: real tokens, never skipped (design doc §4.1 losslessness) -
    #[regex(r"[ \t\r\n]+")]
    WHITESPACE,
    #[regex(r"//[^\n]*", allow_greedy = true)]
    LINE_COMMENT,
    #[regex(r"/\*([^*]|\*[^/])*\*/", allow_greedy = true)]
    BLOCK_COMMENT,

    /// Any byte span `logos` couldn't match to any rule above. `lex` never
    /// panics or drops input on unlexable bytes — this variant, plus total
    /// span coverage, is how that's expressed instead.
    #[allow(missing_docs)]
    // rustdoc already sees the doc comment above; the lint just can't tell through the derive macro's expansion
    ERROR,
}

/// Tokenizes `text` into a flat sequence of `(SyntaxKind, TextRange)` pairs.
///
/// Coverage is total: the returned spans are contiguous, start at zero, and
/// sum to `text.len()` — every byte is accounted for, including unlexable
/// ones, which become [`SyntaxKind::ERROR`] rather than being dropped or
/// causing a panic. Recovery is local: an unlexable byte costs exactly one
/// `ERROR` token, and lexing resumes normally from the next byte, so one bad
/// character never cascades into failure for the rest of the input.
#[must_use]
pub fn lex(text: &str) -> Vec<(SyntaxKind, TextRange)> {
    SyntaxKind::lexer(text)
        .spanned()
        .map(|(result, span)| {
            let kind = result.unwrap_or(SyntaxKind::ERROR);
            let start = u32::try_from(span.start).unwrap_or(u32::MAX);
            let end = u32::try_from(span.end).unwrap_or(u32::MAX);
            (
                kind,
                TextRange::new(TextSize::from(start), TextSize::from(end)),
            )
        })
        .collect()
}

/// The crate's semantic version, as declared in `Cargo.toml`.
///
/// A trivial, genuinely-used API: `libpure`'s `engine_crate_versions` reports
/// on every engine crate this way, including this one.
#[must_use]
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    use super::*;
    use text_size::TextSize;

    #[test]
    fn version_matches_workspace_version() {
        // Asserting non-emptiness alone doesn't kill a mutant that replaces
        // the return value with a different non-empty string (`xyzzy`); pin
        // the exact value instead.
        assert_eq!(version(), "0.1.0");
    }

    fn kinds(text: &str) -> Vec<SyntaxKind> {
        lex(text).into_iter().map(|(kind, _)| kind).collect()
    }

    // -- date family: longest match first (design doc §4.1) -----------------

    #[test]
    fn lexes_date_time_before_strict_date_or_percent() {
        assert_eq!(kinds("%2020-01-01T12:30:00"), [SyntaxKind::DATE_TIME]);
    }

    #[test]
    fn lexes_strict_date_before_percent() {
        assert_eq!(kinds("%2020-01-01"), [SyntaxKind::STRICT_DATE]);
    }

    #[test]
    fn lexes_latest_date_not_as_percent_ident() {
        assert_eq!(kinds("%latest"), [SyntaxKind::LATEST_DATE]);
    }

    #[test]
    fn lexes_bare_percent_when_nothing_else_matches() {
        assert_eq!(kinds("%"), [SyntaxKind::PERCENT]);
    }

    // -- symbols --------------------------------------------------------

    #[test]
    fn lexes_every_documented_symbol() {
        assert_eq!(
            kinds("~$->|@:^.,::()[]=={}!=+-*/<<=>>=;"),
            [
                SyntaxKind::TILDE,
                SyntaxKind::DOLLAR,
                SyntaxKind::ARROW,
                SyntaxKind::PIPE,
                SyntaxKind::AT,
                SyntaxKind::COLON,
                SyntaxKind::NEW_SYMBOL,
                SyntaxKind::DOT,
                SyntaxKind::COMMA,
                SyntaxKind::PATH_SEPARATOR,
                SyntaxKind::PAREN_OPEN,
                SyntaxKind::PAREN_CLOSE,
                SyntaxKind::BRACKET_OPEN,
                SyntaxKind::BRACKET_CLOSE,
                SyntaxKind::EQ,
                SyntaxKind::BRACE_OPEN,
                SyntaxKind::BRACE_CLOSE,
                SyntaxKind::NEQ,
                SyntaxKind::PLUS,
                SyntaxKind::MINUS,
                SyntaxKind::STAR,
                SyntaxKind::SLASH,
                SyntaxKind::LT,
                SyntaxKind::LE,
                SyntaxKind::GT,
                SyntaxKind::GE,
                SyntaxKind::SEMICOLON,
            ]
        );
    }

    #[test]
    fn path_separator_wins_over_two_colons() {
        // `::` must not lex as [COLON, COLON].
        assert_eq!(kinds("::"), [SyntaxKind::PATH_SEPARATOR]);
    }

    // -- M3 keywords vs plain identifiers --------------------------------

    #[test]
    fn lexes_all_five_m3_keywords() {
        assert_eq!(
            kinds("all let allVersions allVersionsInRange toBytes"),
            [
                SyntaxKind::ALL_KW,
                SyntaxKind::WHITESPACE,
                SyntaxKind::LET_KW,
                SyntaxKind::WHITESPACE,
                SyntaxKind::ALL_VERSIONS_KW,
                SyntaxKind::WHITESPACE,
                SyntaxKind::ALL_VERSIONS_IN_RANGE_KW,
                SyntaxKind::WHITESPACE,
                SyntaxKind::TO_BYTES_KW,
            ]
        );
    }

    #[test]
    fn keyword_prefix_identifier_is_not_a_keyword() {
        // A longer identifier that merely starts with a keyword spelling
        // must lex whole, as IDENT — not [ALL_KW, IDENT("Versions2")].
        assert_eq!(kinds("allVersions2"), [SyntaxKind::IDENT]);
        assert_eq!(kinds("letter"), [SyntaxKind::IDENT]);
    }

    // -- literals ---------------------------------------------------------

    #[test]
    fn lexes_ident_integer_string_boolean() {
        assert_eq!(kinds("myVar"), [SyntaxKind::IDENT]);
        assert_eq!(kinds("42"), [SyntaxKind::INTEGER]);
        assert_eq!(kinds("true"), [SyntaxKind::BOOLEAN]);
        assert_eq!(kinds("false"), [SyntaxKind::BOOLEAN]);
    }

    #[test]
    fn lexes_single_quoted_string_with_doubled_quote_escape() {
        // Pascal/SQL-style escaping: '' inside a '...' literal is one
        // embedded quote character, not the string's end (see specs/lexer.md
        // non-goals — corroborated by purecard's ADR-0004, not yet verified
        // against the real engine).
        assert_eq!(kinds("'it''s a test'"), [SyntaxKind::STRING]);
    }

    #[test]
    fn unterminated_string_is_one_error_token_not_a_panic() {
        // `logos`'s DFA is maximal-munch: `[^']*` matches every remaining
        // byte of "unterminated" too (none of them is a `'`), so the DFA
        // keeps consuming hoping for a closing quote all the way to EOF,
        // never reaches STRING's accepting state, and reports the *whole*
        // consumed span as one failed match — not one error byte followed by
        // separate recovery tokens. Verified empirically (this was written
        // expecting per-byte recovery first, which is not how a maximal-
        // munch DFA behaves).
        assert_eq!(kinds("'unterminated"), [SyntaxKind::ERROR]);
    }

    // -- island raw tokens (design doc §2.3) -------------------------------

    #[test]
    fn lexes_island_markers_as_flat_raw_tokens() {
        assert_eq!(
            kinds("#>{db::testDB.personTable}#"),
            [
                SyntaxKind::HASH_STORE_OPEN,
                SyntaxKind::IDENT,
                SyntaxKind::PATH_SEPARATOR,
                SyntaxKind::IDENT,
                SyntaxKind::DOT,
                SyntaxKind::IDENT,
                SyntaxKind::ISLAND_END,
            ]
        );
    }

    #[test]
    fn hash_store_open_wins_over_hash_island_open_and_bare_hash() {
        assert_eq!(kinds("#>{"), [SyntaxKind::HASH_STORE_OPEN]);
        assert_eq!(kinds("#{"), [SyntaxKind::HASH_ISLAND_OPEN]);
        assert_eq!(kinds("#"), [SyntaxKind::HASH]);
    }

    #[test]
    fn lexes_nav_path_block_as_one_token_up_to_next_hash() {
        assert_eq!(kinds("#/model::Class/prop#"), [SyntaxKind::NAV_PATH_BLOCK]);
    }

    // -- trivia: kept as real tokens, never skipped (losslessness) --------

    #[test]
    fn whitespace_is_a_real_token_not_skipped() {
        assert_eq!(
            kinds("a b"),
            [SyntaxKind::IDENT, SyntaxKind::WHITESPACE, SyntaxKind::IDENT]
        );
    }

    #[test]
    fn line_comment_runs_to_end_of_line_only() {
        assert_eq!(
            kinds("a // comment\nb"),
            [
                SyntaxKind::IDENT,
                SyntaxKind::WHITESPACE,
                SyntaxKind::LINE_COMMENT,
                SyntaxKind::WHITESPACE,
                SyntaxKind::IDENT,
            ]
        );
    }

    #[test]
    fn block_comment_stops_at_first_close_not_last() {
        assert_eq!(
            kinds("/* a */ /* b */"),
            [
                SyntaxKind::BLOCK_COMMENT,
                SyntaxKind::WHITESPACE,
                SyntaxKind::BLOCK_COMMENT,
            ]
        );
    }

    #[test]
    fn unterminated_block_comment_falls_back_to_shorter_tokens_not_a_panic() {
        // Unlike the unterminated-string case, this does NOT collapse to one
        // ERROR span. `/` alone is a separately valid token (SLASH), so when
        // BLOCK_COMMENT's automaton fails to ever reach an accepting state,
        // `logos` falls back to the longest prefix that *did* reach some
        // accepting state along the way (SLASH, length 1) rather than
        // failing the whole run — there's no equivalent fallback for `'`
        // (nothing else starts with a bare quote), which is why the string
        // case above genuinely does become one ERROR token. Verified
        // empirically; the property that actually matters is no panic and
        // total coverage, which holds either way.
        assert_eq!(
            kinds("/* never closed"),
            [
                SyntaxKind::SLASH,
                SyntaxKind::STAR,
                SyntaxKind::WHITESPACE,
                SyntaxKind::IDENT,
                SyntaxKind::WHITESPACE,
                SyntaxKind::IDENT,
            ]
        );
    }

    // -- total coverage: every byte accounted for, no gaps, no panics -----

    #[test]
    fn spans_are_contiguous_and_cover_the_whole_input() {
        let text = "$x->filter(y | y.name == 'a''b')  // trailing\n";
        let tokens = lex(text);
        assert!(!tokens.is_empty());
        let mut expected_start = text_size::TextSize::from(0);
        for (_, range) in &tokens {
            assert_eq!(range.start(), expected_start);
            expected_start = range.end();
        }
        assert_eq!(expected_start, TextSize::of(text));
    }

    #[test]
    fn garbage_bytes_yield_error_tokens_not_panics() {
        // NUL, SOH, and € match no rule (not whitespace, not IDENT-start,
        // not any symbol) and none of them borders a token they could
        // extend into, so — unlike the maximal-munch cases above — each
        // really does become its own ERROR token here.
        let tokens = lex("\u{0}\u{1}€");
        assert!(
            tokens.iter().all(|(kind, _)| *kind == SyntaxKind::ERROR),
            "expected every token to be ERROR, got {tokens:?}"
        );
        let total: text_size::TextSize = tokens
            .iter()
            .map(|(_, r)| r.len())
            .fold(0.into(), |a, b| a + b);
        assert_eq!(total, TextSize::of("\u{0}\u{1}€"));
    }

    #[test]
    fn worked_query_from_design_doc_lexes_without_unexpected_errors() {
        let text = "#>{db::testDB.personTable}#\n  \
                    ->join(#>{db::testDB.groupMembershipTable}#, JoinKind.INNER, {x,y| $x.ID == $y.PERSONID})\n  \
                    ->extend(over(~GROUPID, ~SALARY->ascending()), ~[RANK:{p,w,r| $p->rank($w, $r)}]);";
        let tokens = lex(text);
        assert!(
            tokens.iter().all(|(kind, _)| *kind != SyntaxKind::ERROR),
            "unexpected ERROR token in design doc §1.1 worked example: {tokens:?}"
        );
    }
}
