#![no_main]
#![forbid(unsafe_code)]

//! Fuzz target for `pure_analyzer_diagnostics::Diagnostic` serialization.
//!
//! Feeds arbitrary UTF-8 into a diagnostic's message/note fields and asserts
//! that serializing to JSON never panics and preserves the message verbatim
//! (no lossy escaping) — the one invariant that already has real code behind
//! it (the `pure-analyzer-lexer`/parser fuzz targets land once those crates
//! have logic to fuzz).
//!
//! Run with `cargo +nightly fuzz run diagnostics` from the `fuzz/` directory.

use libfuzzer_sys::fuzz_target;
use pure_analyzer_diagnostics::{Diagnostic, FileId, Label, Severity, TextRange};

fuzz_target!(|data: &str| {
    let label = Label::with_note(FileId::new(0), TextRange::new(0.into(), 0.into()), data);
    let diagnostic = Diagnostic::builder("PUR0000", Severity::Info, data, label).build();

    let value = serde_json::to_value(&diagnostic).expect("Diagnostic must always serialize");

    assert_eq!(value["message"], data, "message must round-trip through JSON verbatim");
    assert_eq!(value["primary"]["note"], data, "label note must round-trip through JSON verbatim");
});
