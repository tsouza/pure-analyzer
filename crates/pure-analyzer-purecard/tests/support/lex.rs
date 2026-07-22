//! The shared lexeme tokenizer for the token-granularity test lanes.
//!
//! The shipped core has no model tokenizer (a host supplies one). To exercise the
//! token surface the way a host would, these lanes tokenize a gold `pure_text` at
//! **lexeme granularity**: each identifier/classpath, string/number/date literal,
//! and operator is one token. [`lex`] partitions a query byte-for-byte (no byte is
//! dropped), so concatenating the tokens reproduces the input exactly — a stream
//! M1 already proves streams soundly through the byte-PDA.
//!
//! Single home for the tokenizer (constitution §4, DRY): the L2 harness
//! (`support/l2.rs`), the criterion bench, and the tokenizer self-check corpus
//! lane (`tests/selfcheck_corpus.rs`) all draw on it.

/// Split `text` into lexeme tokens (owned byte strings), partitioning it exactly:
/// the concatenation of the returned tokens equals `text`'s bytes.
#[must_use]
pub fn lex(text: &str) -> Vec<Vec<u8>> {
    let bytes = text.as_bytes();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let start = i;
        let b = bytes[i];
        if b.is_ascii_whitespace() {
            while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                i += 1;
            }
        } else if two_byte_op(bytes, i) {
            i += 2;
        } else if b == b'\'' {
            i = scan_string(bytes, i);
        } else if b == b'%' {
            i += 1;
            while i < bytes.len() && is_date_char(bytes[i]) {
                i += 1;
            }
        } else if b.is_ascii_digit() || (b == b'-' && next_is_digit(bytes, i)) {
            i = scan_number(bytes, i);
        } else if is_ident_start(b) {
            i = scan_ident(bytes, i);
        } else {
            // Any single structural byte: `. $ | , ; : ( ) [ ] { } ! + - * / = & < >`.
            i += 1;
        }
        tokens.push(bytes[start..i].to_vec());
    }
    tokens
}

/// Whether a two-byte operator starts at `i` (`-> :: == != <= >= && ||`).
fn two_byte_op(bytes: &[u8], i: usize) -> bool {
    matches!(
        (bytes.get(i), bytes.get(i + 1)),
        (Some(b'-'), Some(b'>'))
            | (Some(b'='), Some(b'='))
            | (Some(b'!'), Some(b'='))
            | (Some(b'<'), Some(b'='))
            | (Some(b'>'), Some(b'='))
            | (Some(b'&'), Some(b'&'))
            | (Some(b'|'), Some(b'|'))
    )
}

fn next_is_digit(bytes: &[u8], i: usize) -> bool {
    bytes.get(i + 1).is_some_and(u8::is_ascii_digit)
}

/// Scan a single-quoted string, honouring `''` doubling (§5.5).
fn scan_string(bytes: &[u8], mut i: usize) -> usize {
    i += 1; // opening quote
    while i < bytes.len() {
        if bytes[i] == b'\'' {
            if bytes.get(i + 1) == Some(&b'\'') {
                i += 2; // a doubled quote stays inside the string
            } else {
                return i + 1; // closing quote
            }
        } else {
            i += 1;
        }
    }
    i
}

/// Scan a numeric literal: optional leading `-`, integer digits, optional
/// `.`-fraction.
fn scan_number(bytes: &[u8], mut i: usize) -> usize {
    if bytes[i] == b'-' {
        i += 1;
    }
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    if bytes.get(i) == Some(&b'.') && bytes.get(i + 1).is_some_and(u8::is_ascii_digit) {
        i += 1;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
    }
    i
}

/// Scan an identifier or `::`-joined classpath.
fn scan_ident(bytes: &[u8], mut i: usize) -> usize {
    while i < bytes.len() {
        if is_ident_tail(bytes[i]) {
            i += 1;
        } else if bytes[i] == b':'
            && bytes.get(i + 1) == Some(&b':')
            && bytes.get(i + 2).is_some_and(|&b| is_ident_start(b))
        {
            i += 3; // `::` plus the first char of the next segment
        } else {
            break;
        }
    }
    i
}

fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_'
}

fn is_ident_tail(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

fn is_date_char(b: u8) -> bool {
    b.is_ascii_digit() || matches!(b, b'-' | b'T' | b':')
}
