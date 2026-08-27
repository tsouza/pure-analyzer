#![no_main]
#![forbid(unsafe_code)]

//! Fuzz target for lossless, recovery-safe Pure Domain parsing.
//!
//! Run with `just fuzz domain_parser` from the repository root.

use libfuzzer_sys::fuzz_target;
use pure_analyzer_diagnostics::FileId;
use pure_analyzer_parser::parse_domain;

fuzz_target!(|source: &str| {
    let parsed = match parse_domain(source, FileId::new(0)) {
        Ok(parsed) => parsed,
        Err(error) => {
            assert!(false, "small fuzz inputs must build a tree: {error}");
            return;
        }
    };
    assert_eq!(parsed.green.text(), source);
    for token in parsed.green.tokens() {
        assert!(range_is_valid(source, token.text_range()));
    }
    for diagnostic in &parsed.diagnostics {
        assert_eq!(diagnostic.primary.file, FileId::new(0));
        assert!(range_is_valid(source, diagnostic.primary.span));
    }
    for gap in &parsed.coverage_gaps {
        assert!(range_is_valid(source, gap.span));
    }
});

fn range_is_valid(source: &str, range: pure_analyzer_diagnostics::TextRange) -> bool {
    let start = usize::from(range.start());
    let end = usize::from(range.end());
    start <= end && end <= source.len() && source.is_char_boundary(start) && source.is_char_boundary(end)
}
