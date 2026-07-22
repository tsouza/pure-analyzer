//! The synthetic vocabulary shared by the M2 mask tests and the criterion
//! benchmark (`docs/spec/testing.md` §8.5).
//!
//! There is no real model tokenizer in the repo (non-goal, as at M0/M1), so the
//! mask is exercised against a *synthetic* byte-token vocabulary: every short
//! byte string over the walker's representative [`ALPHABET`], enumerated
//! deterministically.
//!
//! Shared via `#[path]` into every target that needs a vocabulary — the oracle,
//! the property lane, and the criterion bench — all of which use
//! [`synthetic_vocab`], so no target carries a dead helper. (The brute-force
//! reference mask lives with its sole user, `tests/mask_oracle.rs`.)

use purecard::Vocab;

/// The representative byte alphabet the synthetic tokens are drawn from — the
/// same one the walk generator probes (`tests/support/walker.rs`), so tokens
/// cover every character class the byte-PDA distinguishes.
pub const ALPHABET: &[u8] = b"abXY1_ |{}()[].,;:$%'-><=!&+*/";

/// A deterministic synthetic vocabulary of `count` distinct byte-tokens: every
/// string over [`ALPHABET`] in ascending length, then ascending index within a
/// length, until `count` are produced. The reserved EOS bit is `count` (one past
/// the last id), so `Vocab`'s own eos is set to `0` and is irrelevant here.
#[must_use]
pub fn synthetic_vocab(count: usize) -> Vocab {
    let base = ALPHABET.len();
    let mut tokens: Vec<Vec<u8>> = Vec::with_capacity(count);
    let mut len = 1u32;
    while tokens.len() < count {
        let total = base.pow(len);
        for idx in 0..total {
            if tokens.len() == count {
                break;
            }
            let mut token = Vec::with_capacity(len as usize);
            let mut rem = idx;
            for _ in 0..len {
                token.push(ALPHABET[rem % base]);
                rem /= base;
            }
            tokens.push(token);
        }
        len += 1;
    }
    Vocab::from_byte_tokens(tokens, 0)
}
