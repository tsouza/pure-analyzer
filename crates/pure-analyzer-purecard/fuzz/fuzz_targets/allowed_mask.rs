//! Fuzz the byte-recognizer + `allowed_mask` surface over an arbitrary byte
//! prefix.
//!
//! Feeds arbitrary bytes through `accept_byte` (each rejected byte leaves the
//! session untouched) and, at every step, recomputes `allowed_mask` and checks
//! the bounds invariant plus the totality of `is_complete` — neither may panic on
//! any byte sequence. A small fixed vocabulary (openers, a step, closers, a
//! literal) gives the mask real context-independent and context-dependent tokens
//! to partition, so the deferred re-probe path is exercised.
#![no_main]

use libfuzzer_sys::fuzz_target;
use purecard::{ByteRecognizer, CompiledGrammar, DecoderSession, Vocab};

fuzz_target!(|data: &[u8]| {
    let vocab = Vocab::from_byte_tokens(
        vec![
            b"|X.all()".to_vec(),
            b"->take(".to_vec(),
            b"1".to_vec(),
            b")".to_vec(),
            b"]".to_vec(),
            b",".to_vec(),
            b"".to_vec(),
        ],
        7,
    );
    let vocab_len = vocab.len();
    let eos = vocab_len as u32;
    let grammar = CompiledGrammar::compile(vocab);
    let mut session = DecoderSession::new(&grammar);

    for &byte in data {
        // A dead byte is rejected and leaves the session untouched; a live one
        // advances it. Either way, no panic.
        let _ = session.accept_byte(byte);
        let complete = session.is_complete();
        let mask = session.allowed_mask();
        assert_eq!(mask.len(), vocab_len + 1, "mask spans V + 1 bits");
        for id in mask.iter_ones() {
            assert!(
                (id as usize) < vocab_len || id == eos,
                "set bit {id} out of range"
            );
        }
        // `is_complete` is total (always a bool) and must agree with the EOS bit.
        assert_eq!(
            complete,
            mask.test(eos),
            "EOS bit set iff the stream is complete"
        );
    }
});
