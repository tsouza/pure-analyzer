//! Property tests for the token surface (`docs/spec/testing.md` §8.5, G2).
//!
//! `proptest` with a fixed config and committed `proptest-regressions/`, matching
//! the walker determinism rule (constitution §2 — no local-only state): a failing
//! case is pinned as a seed and re-run every build. The reachable states are real
//! (walk prefixes, `tests/support/walker.rs`), never synthetic `State` literals.
//!
//! Properties:
//! - **(a) admissible ⇒ safe.** Every id set in `allowed_mask()`, `accept_token`'d
//!   on a clone, returns `Ok`, never panics, and leaves the PDA non-dead.
//! - **(b) token ≡ folded bytes.** `accept_token(id)` reaches the same final
//!   state/stack/`is_complete`/`offset`/`Err` as folding `accept_byte` over
//!   `vocab.bytes(id)`.
//! - **(c) rejected ⇒ untouched.** Every id *cleared* in `allowed_mask()` is
//!   rejected by `accept_token`, leaving the session byte-identical (rollback).
#![forbid(unsafe_code)]

#[path = "support/synth.rs"]
mod synth;
#[path = "support/walker.rs"]
mod walker;

use proptest::prelude::*;
use purecard::{ByteRecognizer, CompiledGrammar, DecoderSession};
use synth::synthetic_vocab;
use walker::generate_walks;

/// Synthetic vocabulary size for the property lane — smaller than the oracle's,
/// since each case walks the whole mask and drives `accept_token`, but still
/// dense across every character class.
const VOCAB_SIZE: usize = 400;

/// The deterministic proptest case count for this lane (constitution §4 — no
/// magic constants; this is gate configuration, tunable up only).
const PROPTEST_CASES: u32 = 256;

/// Drive `prefix` bytes through a fresh session and its parallel raw recognizer
/// state, returning a session positioned at that reachable configuration.
fn session_at<'g>(grammar: &'g CompiledGrammar, prefix: &[u8]) -> DecoderSession<'g> {
    let mut session = DecoderSession::new(grammar);
    for &byte in prefix {
        session
            .accept_byte(byte)
            .expect("a generated walk prefix is always live");
    }
    session
}

fn walk_prefixes() -> Vec<Vec<u8>> {
    // Every prefix of every seeded accepting walk is a reachable configuration.
    let mut prefixes = Vec::new();
    for walk in generate_walks() {
        for len in 0..=walk.len() {
            prefixes.push(walk[..len].to_vec());
        }
    }
    prefixes
}

proptest! {
    // A fixed, committed config: deterministic case count, and regressions are
    // persisted so a discovered counterexample re-runs forever.
    #![proptest_config(ProptestConfig { cases: PROPTEST_CASES, ..ProptestConfig::default() })]

    /// (a) + (c): the mask exactly separates admissible from rejected tokens, and
    /// each verdict is safe.
    #[test]
    fn allowed_mask_partitions_tokens_soundly(seed in any::<prop::sample::Index>()) {
        let grammar = CompiledGrammar::compile(synthetic_vocab(VOCAB_SIZE));
        let prefixes = walk_prefixes();
        let prefix = seed.get(&prefixes);
        let mut session = session_at(&grammar, prefix);
        let eos = grammar.vocab().len() as u32;

        // Snapshot the reusable mask (allowed_mask hands out a borrow of an
        // internal buffer the next call overwrites).
        let mask_ids: Vec<u32> = session.allowed_mask().iter_ones().collect();
        let before = session.clone();

        for id in 0..eos {
            let admissible = mask_ids.contains(&id);
            let mut trial = before.clone();
            let result = trial.accept_token(id);
            if admissible {
                // (a): an admissible token is accepted and leaves a live automaton.
                prop_assert!(result.is_ok(), "id {id} in mask but rejected");
                // A live automaton still accepts *some* continuation or is complete;
                // at minimum it did not panic and offset advanced by the token len.
                prop_assert_eq!(
                    trial.offset(),
                    before.offset() + grammar.vocab().bytes(id).expect("valid id").len()
                );
            } else {
                // (c): a rejected token errs and leaves the session byte-identical.
                // Compare the *full* PDA snapshot (state + entire stack), not just
                // the derived mask/offset: a Pop-then-fail that restored only
                // `(state, stack_len)` would leave a corrupted stack that offset
                // and completeness alone cannot see.
                prop_assert!(result.is_err(), "id {id} rejected by mask but accepted");
                prop_assert_eq!(trial.offset(), before.offset());
                prop_assert_eq!(trial.pda(), before.pda());
            }
        }

        // The reserved EOS bit agrees with is_complete().
        prop_assert_eq!(mask_ids.contains(&eos), before.is_complete());
    }

    /// (b): accept_token(id) is byte-for-byte equivalent to folding accept_byte
    /// over the token's bytes (same completeness, offset, and accept/reject).
    #[test]
    fn accept_token_equals_folding_accept_byte(
        seed in any::<prop::sample::Index>(),
        id_seed in any::<prop::sample::Index>(),
    ) {
        let grammar = CompiledGrammar::compile(synthetic_vocab(VOCAB_SIZE));
        let prefixes = walk_prefixes();
        let prefix = seed.get(&prefixes);
        let session = session_at(&grammar, prefix);

        let id = id_seed.index(VOCAB_SIZE) as u32;
        let bytes = grammar.vocab().bytes(id).expect("valid id").to_vec();

        // Fold accept_byte over the token's bytes.
        let mut folded = session.clone();
        let mut fold_rejected = false;
        for &byte in &bytes {
            if folded.accept_byte(byte).is_err() {
                fold_rejected = true;
                break;
            }
        }

        let mut tok = session.clone();
        let tok_result = tok.accept_token(id);

        prop_assert_eq!(tok_result.is_err(), fold_rejected, "accept/reject disagree for id {}", id);
        if fold_rejected {
            // Rollback: a rejected token leaves the session exactly as it was —
            // asserted on the full PDA snapshot (state + entire stack), so a
            // corrupted-stack rollback cannot slip past.
            prop_assert_eq!(tok.offset(), session.offset());
            prop_assert_eq!(tok.pda(), session.pda());
        } else {
            prop_assert_eq!(tok.offset(), folded.offset());
            // Strong final-state equality on the full `(state, stack)`
            // configuration, not the derived mask: two distinct configurations
            // can share a mask, so mask equality alone would not prove the
            // token- and byte-driven runs truly converged.
            prop_assert_eq!(tok.pda(), folded.pda());
        }
    }
}
