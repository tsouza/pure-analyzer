//! Fuzz `Schema::from_json` over arbitrary bytes.
//!
//! The L2 schema ingress is the only place the pure core parses untrusted host
//! input (JSON). Feeding it arbitrary bytes must always return a `Result` (a
//! `SchemaError` on malformed input), never panic.
#![no_main]

use libfuzzer_sys::fuzz_target;
use purecard::Schema;

fuzz_target!(|data: &[u8]| {
    if let Ok(text) = std::str::from_utf8(data) {
        // Any string in, a `Result` out — never a panic, on any input.
        let _ = Schema::from_json(text);
    }
});
