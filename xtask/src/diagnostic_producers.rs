//! Producer-coverage gate for the registered [`DiagCode`] catalog.
//!
//! A `DiagCode` earns its registration by being constructed somewhere the
//! product actually runs. This module scans tracked Rust source for a real,
//! non-test, non-comment construction site of every entry in
//! [`ALL_DIAG_CODES`] and fails closed when one has none — the exact class of
//! defect tracked by issue #287 (`DiagCode::DerivedQualifiedProperty` /
//! `DiagCode::ModelRequired` were registered, documented, and explainable,
//! but never constructed anywhere).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use pure_analyzer_diagnostics::{ALL_DIAG_CODES, DiagCode};

use crate::process::run_stdout;

/// Files that define or describe the catalog rather than producing findings:
/// the registry itself, its explain-content catalog, and the SARIF
/// description table. A match here is definitional, never a producer.
const CATALOG_FILES: &[&str] = &[
    "crates/pure-analyzer-diagnostics/src/code.rs",
    "crates/pure-analyzer-diagnostics/src/explain.rs",
    "crates/pure-analyzer-render/src/sarif.rs",
];

/// Verify every registered [`DiagCode`] has at least one non-test constructor.
///
/// # Errors
///
/// Returns an error when Git cannot enumerate tracked files, a tracked Rust
/// source file cannot be read as UTF-8, or one or more registered codes have
/// zero non-test construction sites.
pub fn check() -> Result<()> {
    let root = repository_root()?;
    let sources = non_test_rust_sources(&root)?;

    let unproduced: Vec<DiagCode> = ALL_DIAG_CODES
        .iter()
        .copied()
        .filter(|&code| !is_constructed(code, &sources))
        .collect();

    if unproduced.is_empty() {
        return Ok(());
    }

    anyhow::bail!(
        "registered diagnostic code(s) with no non-test producer: {}. Either wire a real \
         producer or remove the code from the registry, its explain catalog entry, the SARIF \
         description table, and its generated `docs/explain/<CODE>.md` page.",
        unproduced
            .iter()
            .map(|code| code.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );
}

/// Resolve the Git repository root so the command works from any subdirectory.
fn repository_root() -> Result<PathBuf> {
    let output = run_stdout("git", &["rev-parse", "--show-toplevel"])?;
    let root = output.trim();
    if root.is_empty() {
        anyhow::bail!("`git rev-parse --show-toplevel` returned an empty repository root");
    }
    Ok(PathBuf::from(root))
}

/// Read every tracked `.rs` file outside integration-test crates and the
/// catalog files, reduced to real code: comments, string/char literals, and
/// `#[cfg(test)]`-gated item bodies are all blanked out.
fn non_test_rust_sources(root: &Path) -> Result<Vec<String>> {
    let root_str = root
        .to_str()
        .context("Git repository root is not valid UTF-8")?;
    let output = run_stdout("git", &["-C", root_str, "ls-files", "-z"])?;
    let mut sources = Vec::new();
    for path in output
        .split_terminator('\0')
        .filter(|path| !path.is_empty())
    {
        let relative = Path::new(path);
        if relative
            .extension()
            .and_then(|extension| extension.to_str())
            != Some("rs")
        {
            continue;
        }
        if relative
            .components()
            .any(|component| component.as_os_str() == "tests")
        {
            continue;
        }
        if CATALOG_FILES
            .iter()
            .any(|catalog_file| Path::new(catalog_file) == relative)
        {
            continue;
        }
        let text = std::fs::read_to_string(root.join(relative))
            .with_context(|| format!("reading tracked source {}", relative.display()))?;
        sources.push(code_only(&text));
    }
    Ok(sources)
}

/// Whether any pre-filtered source still constructs `code`.
fn is_constructed(code: DiagCode, sources: &[String]) -> bool {
    let needle = format!("DiagCode::{code:?}");
    sources.iter().any(|source| contains_word(source, &needle))
}

/// `source.contains(needle)` with word boundaries on both sides, so
/// `UnknownProperty` never spuriously matches a longer variant name or a
/// look-alike type (`MyDiagCode::Foo` does not match `DiagCode::Foo`).
fn contains_word(source: &str, needle: &str) -> bool {
    source.match_indices(needle).any(|(index, _)| {
        let before_ok = source[..index]
            .chars()
            .next_back()
            .is_none_or(|previous| !(previous.is_alphanumeric() || previous == '_'));
        let after_ok = source[index + needle.len()..]
            .chars()
            .next()
            .is_none_or(|next| !(next.is_alphanumeric() || next == '_'));
        before_ok && after_ok
    })
}

/// Reduce `source` to real code: every comment, string/char literal, and
/// `#[cfg(test)]`-gated item body is blanked to spaces (newlines preserved),
/// so a textual search only sees construction sites the compiler itself
/// would treat as live code.
///
/// This is a source-level scan, not a full parser: it tracks comments,
/// strings, and char literals only closely enough that a `{`/`}`/`'` inside
/// one of them is never mistaken for real brace or quote structure (this
/// codebase's own lexer/parser sources match against `'{'` and `'}'` char
/// literals, so naive brace counting would misfire on exactly those files).
fn code_only(source: &str) -> String {
    let chars: Vec<char> = source.chars().collect();
    let code_positions = code_positions(&chars);
    let mut excluded = vec![false; chars.len()];
    mark_cfg_test_regions(&chars, &code_positions, &mut excluded);

    chars
        .iter()
        .enumerate()
        .map(|(index, &character)| {
            if character == '\n' {
                '\n'
            } else if code_positions[index] && !excluded[index] {
                character
            } else {
                ' '
            }
        })
        .collect()
}

/// The lexical region a byte position falls in, for [`code_positions`].
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Code,
    LineComment,
    BlockComment(u32),
    StringLiteral,
    RawStringLiteral(usize),
    CharLiteral,
}

/// `true` at every index that is ordinary code: not inside a line comment,
/// block comment, string literal, byte string, raw string, or char literal.
fn code_positions(chars: &[char]) -> Vec<bool> {
    let mut mask = vec![false; chars.len()];
    let mut mode = Mode::Code;
    let mut index = 0;
    while index < chars.len() {
        mask[index] = matches!(mode, Mode::Code);
        (index, mode) = advance(chars, index, mode);
    }
    mask
}

/// Advance one lexical step from `(index, mode)`, returning the next position
/// and mode. Each mode's transition rule lives in its own small function so
/// this dispatcher stays a flat match.
fn advance(chars: &[char], index: usize, mode: Mode) -> (usize, Mode) {
    match mode {
        Mode::Code => advance_in_code(chars, index),
        Mode::LineComment => advance_in_line_comment(chars, index),
        Mode::BlockComment(depth) => advance_in_block_comment(chars, index, depth),
        Mode::StringLiteral => advance_in_string_literal(chars, index),
        Mode::RawStringLiteral(hashes) => advance_in_raw_string_literal(chars, index, hashes),
        Mode::CharLiteral => advance_in_char_literal(chars, index),
    }
}

fn advance_in_code(chars: &[char], index: usize) -> (usize, Mode) {
    if starts_with_at(chars, index, "//") {
        return (index + 2, Mode::LineComment);
    }
    if starts_with_at(chars, index, "/*") {
        return (index + 2, Mode::BlockComment(1));
    }
    if chars[index] == '"' {
        return (index + 1, Mode::StringLiteral);
    }
    if let Some(hashes) = raw_string_prefix_hashes(chars, index) {
        return (
            index + prefix_len(chars, index) + 1 + hashes,
            Mode::RawStringLiteral(hashes),
        );
    }
    if chars[index] == '\'' && is_char_literal(chars, index) {
        return (index + 1, Mode::CharLiteral);
    }
    (index + 1, Mode::Code)
}

fn advance_in_line_comment(chars: &[char], index: usize) -> (usize, Mode) {
    let mode = if chars[index] == '\n' {
        Mode::Code
    } else {
        Mode::LineComment
    };
    (index + 1, mode)
}

fn advance_in_block_comment(chars: &[char], index: usize, depth: u32) -> (usize, Mode) {
    if starts_with_at(chars, index, "/*") {
        return (index + 2, Mode::BlockComment(depth + 1));
    }
    if starts_with_at(chars, index, "*/") {
        let mode = if depth == 1 {
            Mode::Code
        } else {
            Mode::BlockComment(depth - 1)
        };
        return (index + 2, mode);
    }
    (index + 1, Mode::BlockComment(depth))
}

fn advance_in_string_literal(chars: &[char], index: usize) -> (usize, Mode) {
    if chars[index] == '\\' && index + 1 < chars.len() {
        return (index + 2, Mode::StringLiteral);
    }
    let mode = if chars[index] == '"' {
        Mode::Code
    } else {
        Mode::StringLiteral
    };
    (index + 1, mode)
}

fn advance_in_raw_string_literal(chars: &[char], index: usize, hashes: usize) -> (usize, Mode) {
    if chars[index] == '"' && has_hash_run(chars, index + 1, hashes) {
        return (index + 1 + hashes, Mode::Code);
    }
    (index + 1, Mode::RawStringLiteral(hashes))
}

fn advance_in_char_literal(chars: &[char], index: usize) -> (usize, Mode) {
    let mode = if chars[index] == '\'' {
        Mode::Code
    } else {
        Mode::CharLiteral
    };
    (index + 1, mode)
}

/// Length of a raw-string/byte-raw-string prefix (`r`/`br`) starting at `index`.
fn prefix_len(chars: &[char], index: usize) -> usize {
    if starts_with_at(chars, index, "br") {
        2
    } else {
        1
    }
}

/// If `index` begins a raw string opener (`r"`, `r#"`, `br"`, `br#"`, ...),
/// the number of `#` characters in that opener.
fn raw_string_prefix_hashes(chars: &[char], index: usize) -> Option<usize> {
    if !(starts_with_at(chars, index, "r") || starts_with_at(chars, index, "br")) {
        return None;
    }
    let mut cursor = index + prefix_len(chars, index);
    let mut hashes = 0;
    while chars.get(cursor) == Some(&'#') {
        hashes += 1;
        cursor += 1;
    }
    if chars.get(cursor) == Some(&'"') {
        Some(hashes)
    } else {
        None
    }
}

/// Whether `hashes` consecutive `#` characters start at `index`.
fn has_hash_run(chars: &[char], index: usize, hashes: usize) -> bool {
    (0..hashes).all(|offset| chars.get(index + offset) == Some(&'#'))
}

/// Distinguish a char literal (`'{'`, `'\n'`, `'a'`) from a lifetime (`'a`,
/// `'static`) at a `'` found in [`Mode::Code`].
fn is_char_literal(chars: &[char], index: usize) -> bool {
    let Some(&next) = chars.get(index + 1) else {
        return false;
    };
    if next == '\\' {
        // An escape sequence always closes with a single unescaped `'`.
        let mut cursor = index + 2;
        while let Some(&candidate) = chars.get(cursor) {
            if candidate == '\'' {
                return true;
            }
            if candidate == '\n' {
                return false;
            }
            cursor += 1;
        }
        return false;
    }
    chars.get(index + 2) == Some(&'\'')
}

/// Whether `chars[index..]` starts with the literal `needle`.
fn starts_with_at(chars: &[char], index: usize, needle: &str) -> bool {
    needle
        .chars()
        .enumerate()
        .all(|(offset, expected)| chars.get(index + offset) == Some(&expected))
}

/// Find the matching close bracket for the open bracket at `open_index`,
/// counting only positions where `code_positions` is `true`.
fn matching_bracket(
    chars: &[char],
    code_positions: &[bool],
    open_index: usize,
    open: char,
    close: char,
) -> Option<usize> {
    let mut depth = 0i32;
    let mut index = open_index;
    while index < chars.len() {
        if code_positions[index] {
            if chars[index] == open {
                depth += 1;
            } else if chars[index] == close {
                depth -= 1;
                if depth == 0 {
                    return Some(index);
                }
            }
        }
        index += 1;
    }
    None
}

/// Find the next occurrence of `target` at a code position, at or after `from`.
fn find_at(chars: &[char], code_positions: &[bool], from: usize, target: char) -> Option<usize> {
    (from..chars.len()).find(|&index| code_positions[index] && chars[index] == target)
}

/// Whether the meta-item body `chars[start..end)` names `test` as a bare
/// word, covering both `#[cfg(test)]` and `#[cfg(all(test, ...))]`.
fn attribute_gates_test(chars: &[char], start: usize, end: usize) -> bool {
    let body: String = chars[start..end].iter().collect();
    body.match_indices("test").any(|(offset, _)| {
        let before_ok = body[..offset]
            .chars()
            .next_back()
            .is_none_or(|previous| !(previous.is_alphanumeric() || previous == '_'));
        let after_ok = body[offset + "test".len()..]
            .chars()
            .next()
            .is_none_or(|next| !(next.is_alphanumeric() || next == '_'));
        before_ok && after_ok
    })
}

/// Find the end of the item gated by the attribute closing at `from - 1`: the
/// matching `}` of its first brace body, or the first top-level `;` when the
/// item has no brace body (e.g. `#[cfg(test)] use super::*;`).
fn gated_item_end(chars: &[char], code_positions: &[bool], from: usize) -> usize {
    let mut index = from;
    while index < chars.len() {
        if code_positions[index] {
            if chars[index] == '{' {
                return matching_bracket(chars, code_positions, index, '{', '}')
                    .unwrap_or_else(|| chars.len().saturating_sub(1));
            }
            if chars[index] == ';' {
                return index;
            }
        }
        index += 1;
    }
    chars.len().saturating_sub(1)
}

/// Mark every index inside a `#[cfg(test)]`-gated item body as excluded.
fn mark_cfg_test_regions(chars: &[char], code_positions: &[bool], excluded: &mut [bool]) {
    let mut index = 0;
    while index + 6 <= chars.len() {
        if !code_positions[index] || !starts_with_at(chars, index, "#[cfg(") {
            index += 1;
            continue;
        }
        let open_paren = index + 5;
        let Some(close_paren) = matching_bracket(chars, code_positions, open_paren, '(', ')')
        else {
            index += 1;
            continue;
        };
        if !attribute_gates_test(chars, open_paren + 1, close_paren) {
            index = close_paren + 1;
            continue;
        }
        let Some(attribute_close) = find_at(chars, code_positions, close_paren + 1, ']') else {
            index = close_paren + 1;
            continue;
        };
        let item_end = gated_item_end(chars, code_positions, attribute_close + 1);
        for slot in &mut excluded[index..=item_end] {
            *slot = true;
        }
        index = item_end + 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_currently_registered_code_has_a_non_test_producer() {
        // `check()` (exercised below in `check_passes_against_the_real_workspace`)
        // is the general, permanent gate. This asserts specifically that issue
        // #287's four confirmed producerless codes — the two it named
        // (`DerivedQualifiedProperty`/PUR2100, `ModelRequired`/PUR9001) and the
        // two the new gate additionally caught under the same rule
        // (`CardinalityMisuse`/PUR2003, `EquivalenceVerdict`/PUR3001) — stay
        // removed rather than silently creeping back into the registry.
        let removed_by_this_issue = [
            "DerivedQualifiedProperty",
            "ModelRequired",
            "CardinalityMisuse",
            "EquivalenceVerdict",
        ];
        let still_registered: Vec<_> = removed_by_this_issue
            .into_iter()
            .filter(|removed| {
                ALL_DIAG_CODES
                    .iter()
                    .any(|code| format!("{code:?}") == *removed)
            })
            .collect();
        assert!(
            still_registered.is_empty(),
            "issue #287's producerless codes must be removed from ALL_DIAG_CODES, found: {still_registered:?}"
        );
    }

    #[test]
    fn code_only_blanks_a_cfg_test_gated_item_but_keeps_real_code() {
        let source = "fn real() { DiagCode::Real; }\n\
                       #[cfg(test)]\n\
                       mod tests {\n\
                           fn hidden() { DiagCode::Hidden; }\n\
                       }\n";
        let stripped = code_only(source);
        assert!(stripped.contains("DiagCode::Real"));
        assert!(!stripped.contains("DiagCode::Hidden"));
        assert_eq!(stripped.lines().count(), source.lines().count());
    }

    #[test]
    fn code_only_ignores_char_literal_braces() {
        // The `'{'`/`'}'` char literals (their quoted contents are blanked,
        // same as any other literal) must not be mistaken for real braces
        // when locating the `#[cfg(test)]` item that follows: the match
        // arms' structure survives, the gated diagnostic construction is
        // excluded, and a real one right after it is not swallowed too.
        let source = "match c { '{' => 1, '}' => 2, _ => 0 };\n\
                       #[cfg(test)]\n\
                       fn hidden() { DiagCode::Hidden; }\n\
                       fn real() { DiagCode::Real; }\n";
        let stripped = code_only(source);
        assert!(stripped.contains("match c"));
        assert!(stripped.contains("=> 1"));
        assert!(stripped.contains("=> 2"));
        assert!(!stripped.contains("DiagCode::Hidden"));
        assert!(stripped.contains("DiagCode::Real"));
    }

    #[test]
    fn code_only_blanks_comments_and_string_literals() {
        let source = "const S: &str = \"mentions DiagCode::NotReal\";\n\
                       // also mentions DiagCode::NotReal in a comment\n\
                       /// [`DiagCode::NotReal`] doc reference\n\
                       fn real() { DiagCode::Real; }\n";
        let stripped = code_only(source);
        assert!(!stripped.contains("DiagCode::NotReal"));
        assert!(stripped.contains("DiagCode::Real"));
    }

    #[test]
    fn code_only_handles_semicolon_terminated_gated_items() {
        let source = "#[cfg(test)]\nuse super::Hidden;\nfn real() { DiagCode::Real; }\n";
        let stripped = code_only(source);
        assert!(!stripped.contains("Hidden"));
        assert!(stripped.contains("DiagCode::Real"));
    }

    #[test]
    fn code_only_handles_cfg_all_test_attributes() {
        let source = "#[cfg(all(test, feature = \"x\"))]\nfn hidden() { DiagCode::Hidden; }\n";
        let stripped = code_only(source);
        assert!(!stripped.contains("DiagCode::Hidden"));
    }

    #[test]
    fn contains_word_rejects_partial_identifier_matches_on_either_side() {
        assert!(!contains_word(
            "DiagCode::UnknownPropertyExtra",
            "DiagCode::UnknownProperty"
        ));
        assert!(!contains_word(
            "MyDiagCode::UnknownProperty",
            "DiagCode::UnknownProperty"
        ));
        assert!(contains_word(
            "DiagCode::UnknownProperty,",
            "DiagCode::UnknownProperty"
        ));
    }

    #[test]
    fn check_passes_against_the_real_workspace() {
        check().expect("every registered DiagCode must have a non-test producer");
    }
}
