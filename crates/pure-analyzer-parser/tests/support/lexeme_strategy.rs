//! A weighted token-sequence `proptest` [`Strategy`] over the parser's real
//! lexeme alphabet (issue #299).
//!
//! `any::<String>()`/`".{0,2048}"` draw from the full Unicode codepoint
//! space, so the odds of ever emitting the exact byte sequences `Class`,
//! `Association`, `extends`, `<<`, `#>{`, or even one balanced `(`/`)` pair
//! are astronomically small — none of the parser's recovery machinery these
//! two proptests are named for was ever actually being exercised. This
//! module instead samples *tokens* from the same alphabet
//! `pure_analyzer_lexer::SyntaxKind` accepts (symbols, M3 keywords, island
//! markers) plus the Domain grammar's textual keywords (`Class`,
//! `Association`, `Profile`, `extends`, `stereotypes`, `tags` — recognized by
//! `Parser::at_keyword` against a plain `IDENT`, not a separate lexer token
//! kind), so real grammar structure is reached at a meaningful rate instead
//! of effectively never. A low-weight "noise" arm keeps a genuine slice of
//! the original arbitrary-Unicode/`ERROR`-token territory alive.

use proptest::prelude::*;
use proptest::sample::select;

/// Upper bound on how many lexemes one generated source draws — long enough
/// to nest a `Class { ... }` body or a multi-argument call chain, short
/// enough that a `PROPTEST_CASES`-sized run stays fast.
const MAX_TOKENS: usize = 48;

// -- weight tiers (constitution §4 — no magic constants): relative, not
// required to sum to any particular total; `prop_oneof!` normalizes them. --
const STRUCTURAL_WEIGHT: u32 = 6;
const KEYWORD_WEIGHT: u32 = 5;
const IDENT_WEIGHT: u32 = 5;
const LITERAL_WEIGHT: u32 = 3;
const WHITESPACE_WEIGHT: u32 = 8;
const COMMENT_WEIGHT: u32 = 1;
const NOISE_WEIGHT: u32 = 2;

/// One generated lexeme: its text, and whether that text's recognition
/// depends on *not* touching an adjacent identifier/keyword/number character
/// (an `IDENT`-class token, a keyword, or a digit run). Two wordish lexemes
/// placed back to back would lexically merge into one bigger `IDENT`/number
/// token — e.g. `Class` immediately followed by `demo` becomes the single
/// identifier `Classdemo`, silently destroying the keyword the sequence was
/// built to exercise. [`join`] forces a separating space exactly there.
type Lexeme = (String, bool);

/// Every non-alphanumeric M3 token (`pure_analyzer_lexer::SyntaxKind`'s
/// `#[token(...)]` symbols and island markers), plus `<<`/`>>` — two adjacent
/// `LT`/`GT` tokens, the Domain stereotype-application delimiter
/// (`Parser::at_double_angle_open`). None of these touch a word character, so
/// none needs a forced separator.
const STRUCTURAL_TOKENS: &[&str] = &[
    "~", "$", "->", "|", "@", "^", ".", ",", "::", ":", "(", ")", "[", "]", "==", "!=", "+", "-",
    "*", "/", "<=", "<", ">=", ">", ";", "{", "}", "=", "%", "#>{", "#{", "}#", "#", "<<", ">>",
];

/// M3's five reserved words, its two `bool` literals, `%latest`, and the
/// Domain grammar's textual keywords — every one recognized only by exact
/// spelling against an `IDENT`/literal token, so every one is wordish.
const KEYWORD_TOKENS: &[&str] = &[
    "all",
    "let",
    "allVersions",
    "allVersionsInRange",
    "toBytes",
    "true",
    "false",
    "%latest",
    "Class",
    "Association",
    "Profile",
    "extends",
    "stereotypes",
    "tags",
];

/// Real whitespace trivia (`SyntaxKind::WHITESPACE`) — the separator that
/// keeps a sequence of wordish lexemes from merging by accident, and common
/// enough here to make that the typical case rather than the forced-space
/// fallback in [`join`].
const WHITESPACE_TOKENS: &[&str] = &[" ", "  ", "\n", "\t", "\n\n"];

/// Line and block comment trivia (`SyntaxKind::LINE_COMMENT`/`BLOCK_COMMENT`).
const COMMENT_TOKENS: &[&str] = &["// c\n", "/* c */"];

fn fixed(tokens: &'static [&'static str], wordish: bool) -> impl Strategy<Value = Lexeme> {
    select(tokens).prop_map(move |text| (text.to_string(), wordish))
}

/// A dynamic identifier, integer, single-quoted string, or `dateLit`, each a
/// real `SyntaxKind` production rather than one fixed spelling, so the
/// generator still explores length/content the fixed lists don't enumerate.
fn literal() -> impl Strategy<Value = Lexeme> {
    prop_oneof![
        "[a-zA-Z][a-zA-Z0-9_]{0,10}".prop_map(|s| (s, true)),
        "[0-9]{1,6}".prop_map(|s| (s, true)),
        "'[a-zA-Z0-9 ]{0,12}'".prop_map(|s| (s, false)),
        "%[0-9]{4}-[0-9]{2}-[0-9]{2}".prop_map(|s| (s, true)),
    ]
}

/// One arbitrary `char` (the full codepoint space `any::<String>()` used to
/// draw every byte from) at low weight, so unlexable-byte recovery
/// (`SyntaxKind::ERROR`/`DiagCode::BadToken`) stays covered without
/// dominating the sequence the way it did before.
fn noise() -> impl Strategy<Value = Lexeme> {
    any::<char>().prop_map(|c| (c.to_string(), true))
}

fn lexeme() -> impl Strategy<Value = Lexeme> {
    prop_oneof![
        STRUCTURAL_WEIGHT => fixed(STRUCTURAL_TOKENS, false),
        KEYWORD_WEIGHT => fixed(KEYWORD_TOKENS, true),
        IDENT_WEIGHT => "[a-zA-Z][a-zA-Z0-9_]{0,10}".prop_map(|s| (s, true)),
        LITERAL_WEIGHT => literal(),
        WHITESPACE_WEIGHT => fixed(WHITESPACE_TOKENS, false),
        COMMENT_WEIGHT => fixed(COMMENT_TOKENS, false),
        NOISE_WEIGHT => noise(),
    ]
}

/// Concatenate generated lexemes, inserting one space wherever two wordish
/// lexemes would otherwise merge into a single, different token.
fn join(tokens: Vec<Lexeme>) -> String {
    let mut out = String::new();
    let mut prev_wordish = false;
    for (text, wordish) in tokens {
        if prev_wordish && wordish {
            out.push(' ');
        }
        out.push_str(&text);
        prev_wordish = wordish;
    }
    out
}

/// A weighted token-sequence source string over the parser's real lexeme
/// alphabet — the replacement for `any::<String>()`/`".{0,2048}"`.
pub fn arbitrary_source() -> impl Strategy<Value = String> {
    proptest::collection::vec(lexeme(), 0..=MAX_TOKENS).prop_map(join)
}
