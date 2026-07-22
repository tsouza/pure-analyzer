//! Byte-level-BPE token-string → raw-byte decoding, shared by the real-tokenizer
//! lanes.
//!
//! The shipped core has no model tokenizer (a host supplies one). A byte-level BPE
//! tokenizer (Qwen2.5-Coder, GPT-4's cl100k_base) emits token *strings* in GPT-2's
//! `bytes_to_unicode` alphabet; these helpers fold such a string back to the raw
//! bytes the model actually emitted, so a lane can drive the true byte stream
//! through a `DecoderSession`. Qwen and cl100k share the *same* byte-unicode table
//! (it is GPT-2's), so one decoder serves every byte-level tokenizer lane — the
//! real-Qwen soundness oracle and the hermetic fused-precision replay both fold
//! token strings through this one module (constitution §4, DRY).

use std::collections::HashMap;

/// GPT-2's `bytes_to_unicode` keeps three byte ranges as their own code point and
/// remaps the rest; these are the exact range bounds from the reference
/// implementation. The Latin-1 span is split because `0xAD` (soft hyphen) is the one
/// byte in `0xA1..=0xFF` the reference drops from the identity set.
const LATIN1_PRINTABLE_LO: u8 = 0xA1;
const LATIN1_PRINTABLE_HI: u8 = 0xAC;
const LATIN1_SYMBOLS_LO: u8 = 0xAE;
const LATIN1_SYMBOLS_HI: u8 = 0xFF;
/// Bytes not kept as themselves are assigned code points starting just past the
/// 256-value byte range, so a remapped byte never collides with a kept one.
const REMAP_BASE: u32 = 256;
const BYTE_MAX: u16 = 255;

/// The inverse of GPT-2's `bytes_to_unicode`: map each byte-level-BPE token-string
/// char back to the raw byte the model actually emits (this also undoes the
/// `Ġ`→space and other whitespace conventions, since they live inside the byte
/// table). Every byte-level-BPE token string is composed only of chars in this
/// table.
pub fn gpt2_byte_decoder() -> HashMap<char, u8> {
    let mut bs: Vec<u8> = (b'!'..=b'~')
        .chain(LATIN1_PRINTABLE_LO..=LATIN1_PRINTABLE_HI)
        .chain(LATIN1_SYMBOLS_LO..=LATIN1_SYMBOLS_HI)
        .collect();
    let mut cs: Vec<u32> = bs.iter().map(|&b| u32::from(b)).collect();
    let mut n = 0u32;
    for b in 0u16..=BYTE_MAX {
        let byte = b as u8;
        if !bs.contains(&byte) {
            bs.push(byte);
            cs.push(REMAP_BASE + n);
            n += 1;
        }
    }
    bs.into_iter()
        .zip(cs)
        .map(|(b, c)| (char::from_u32(c).expect("valid scalar"), b))
        .collect()
}

/// The true emitted bytes of one byte-level-BPE token string, decoded through
/// [`gpt2_byte_decoder`]. A special token (`<|im_end|>`, FIM) is stored as a
/// literal ASCII string whose chars map to themselves, so its "bytes" are the
/// literal `<|...|>` — never valid Pure, so the byte-PDA rejects it and it is
/// inadmissible mid-query (M2), exactly as required.
pub fn true_bytes(tok: &str, dec: &HashMap<char, u8>) -> Vec<u8> {
    tok.chars()
        .map(|c| {
            *dec.get(&c).unwrap_or_else(|| {
                panic!("token char {c:?} is outside the byte-level table; cannot recover its true bytes")
            })
        })
        .collect()
}
