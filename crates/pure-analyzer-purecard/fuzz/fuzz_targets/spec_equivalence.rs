//! Fuzz differential equivalence between the fixed hand-written PDA
//! (`CompiledGrammar::compile`) and its declarative transcription
//! (`CompiledGrammar::from_spec(EMITTED_SUBSET_SPEC, ..)`, issue #57).
//!
//! `tests/spec_equivalence.rs` proves the two engines agree over the gold,
//! modern-dialect, and precision-reject corpora plus a fixed set of
//! deterministic random walks; this target explores byte sequences none of
//! those curated inputs cover, replaying each arbitrary byte string through
//! both engines and asserting they reach the identical dead/complete verdict
//! at *every* step, not just at the end — a divergence at byte `k` must be
//! caught even if a later byte would have brought both back into agreement.
#![no_main]

use libfuzzer_sys::fuzz_target;
use purecard::grammar::EMITTED_SUBSET_SPEC;
use purecard::{ByteRecognizer, CompiledGrammar, DecoderSession, Vocab};

fuzz_target!(|data: &[u8]| {
    let fixed = CompiledGrammar::compile(Vocab::from_byte_tokens(Vec::new()));
    let spec =
        CompiledGrammar::from_spec(EMITTED_SUBSET_SPEC, Vocab::from_byte_tokens(Vec::new()))
            .expect("the shipped emitted-subset spec always compiles");

    let mut fixed_session = DecoderSession::new(&fixed);
    let mut spec_session = DecoderSession::new(&spec);

    for &byte in data {
        let fixed_result = fixed_session.accept_byte(byte);
        let spec_result = spec_session.accept_byte(byte);
        assert_eq!(
            fixed_result.is_ok(),
            spec_result.is_ok(),
            "fixed and spec-compiled engines diverge on dead/alive for byte {byte:#04x} in {data:?}"
        );
        if fixed_result.is_err() {
            // Both engines are dead at this byte; a dead `DecoderSession` is
            // never fed further bytes in real use, so stop here — matching
            // `tests/spec_equivalence.rs`'s per-string (not per-prefix) check.
            return;
        }
        assert_eq!(
            fixed_session.is_complete(),
            spec_session.is_complete(),
            "fixed and spec-compiled engines diverge on completeness after byte {byte:#04x} in {data:?}"
        );
    }
});
