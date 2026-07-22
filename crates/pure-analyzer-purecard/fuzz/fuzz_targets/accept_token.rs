//! Fuzz `DecoderSession::accept_token` over an arbitrary vocabulary and id stream.
//!
//! Builds a `CompiledGrammar` from arbitrary token bytes, then folds an arbitrary
//! sequence of token ids (including out-of-range ones) through `accept_token`.
//! Asserts the no-panic invariant and, after every step, the mask bounds
//! invariant: its bit-length is `vocab.len() + 1` (EOS included) and no set bit
//! exceeds the reserved EOS position — a set bit is either a real token id
//! (`< vocab.len()`) or exactly the EOS id.
#![no_main]

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;
use purecard::{CompiledGrammar, DecoderSession, Vocab};

#[derive(Arbitrary, Debug)]
struct Input {
    vocab_bytes: Vec<Vec<u8>>,
    ids: Vec<u32>,
}

fuzz_target!(|input: Input| {
    let vocab_len = input.vocab_bytes.len();
    let eos = vocab_len as u32;
    let vocab = Vocab::from_byte_tokens(input.vocab_bytes, eos);
    let grammar = CompiledGrammar::compile(vocab);
    let mut session = DecoderSession::new(&grammar);

    check_mask_bounds(&mut session, vocab_len, eos);
    for id in input.ids {
        // Any id — in-range, EOS, or out-of-range — must be handled without a
        // panic. Out-of-range ids now surface as `UnknownToken`.
        let _ = session.accept_token(id);
        check_mask_bounds(&mut session, vocab_len, eos);
    }
});

fn check_mask_bounds(session: &mut DecoderSession, vocab_len: usize, eos: u32) {
    let mask = session.allowed_mask();
    assert_eq!(
        mask.len(),
        vocab_len + 1,
        "mask spans V + 1 bits (EOS included)"
    );
    for id in mask.iter_ones() {
        assert!(
            (id as usize) < vocab_len || id == eos,
            "set bit {id} is neither a real token (< {vocab_len}) nor the EOS id {eos}"
        );
    }
}
