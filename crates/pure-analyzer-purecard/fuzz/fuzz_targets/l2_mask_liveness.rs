//! Fuzz the two L2 **liveness** invariants (`docs/spec/schema.md` §6.7, issues
//! #275 and #296): over a vocabulary that can spell the continuations the overlay
//! leaves legal, the schema overlay may narrow the per-step mask but may never
//! empty it — nor, at a term it agrees is already whole, clear a way of ending
//! that term which L1 admits.
//!
//! A mask with no vocabulary bit *and* no EOS bit hands the host nothing to
//! sample and no way to stop — a decoder deadlock rather than a constraint. The
//! precondition is why the vocabulary below is byte-granular: a rule narrowing to
//! a set of names can only leave a live token if some token spells a legal name's
//! next bytes, and every real BPE vocabulary carries all 256 bytes.
//!
//! `l2_liveness.rs` already pins the invariant over seeded walks, but a seeded
//! generator explores only the paths its own PRNG happens to take — and two of
//! the three shipped witnesses were found only because a *different* draw
//! happened to reach them. Here the
//! fuzzer chooses each step: the input bytes are read as a sequence of *choices*
//! into the live set the L2 mask publishes, so libFuzzer's coverage feedback
//! steers the walk toward unexplored `(L2 rule, byte-PDA state, stack)` contexts
//! rather than resampling the easy ones. Every accepted token is one the schema
//! mask itself admitted, so any position reached is a position a real host would
//! genuinely arrive at.
//!
//! The vocabulary is one token per printable ASCII byte — the adversarial case
//! for an overlay whose rules classify *whole* lexemes, since only a byte-level
//! vocabulary can park a stream on a bare `$` sigil or the lone `.` that opens a
//! float. The schema is a committed fixture (`world_1`, the richest of the eight),
//! embedded at compile time so the target stays hermetic.
#![no_main]

use libfuzzer_sys::fuzz_target;
use purecard::{CompiledGrammar, DecoderSession, Schema, Vocab};

#[path = "../../tests/support/completed_term.rs"]
mod completed_term;

use completed_term::{TERM_END_BYTES, is_completed_term};

/// The printable ASCII range the vocabulary spans, one token per byte.
const FIRST_BYTE: u8 = 0x20;
const LAST_BYTE: u8 = 0x7e;

/// The token id of `byte` in the single-byte vocabulary above: one token per
/// printable ASCII byte, in order, so an id is a byte's offset from
/// [`FIRST_BYTE`]. Every [`TERM_END_BYTES`] entry is inside that range.
fn id_of(byte: u8) -> u32 {
    u32::from(byte - FIRST_BYTE)
}

/// The committed schema fixture the walk narrows against.
const SCHEMA_JSON: &str = include_str!("../../tests/fixtures/schemas/world_1.json");

thread_local! {
    /// The schema and the compiled grammar are input-independent, so they are
    /// built once per fuzzing process rather than per input: parsing the fixture
    /// and compiling the grammar otherwise dominate the per-run cost, and a fuzz
    /// target's value is in runs per second. Thread-local rather than a `static`
    /// because `CompiledGrammar` fills a lazy per-state mask cache and so is
    /// deliberately not `Sync`; libFuzzer drives one input at a time per thread.
    static FIXTURE: (CompiledGrammar, Schema, u32) = {
        let schema = Schema::from_json(SCHEMA_JSON).expect("the committed world_1 fixture parses");
        let tokens: Vec<Vec<u8>> = (FIRST_BYTE..=LAST_BYTE).map(|byte| vec![byte]).collect();
        let eos = tokens.len() as u32;
        let vocab = Vocab::from_byte_tokens(tokens);
        (CompiledGrammar::compile(vocab), schema, eos)
    };
}

fuzz_target!(|data: &[u8]| {
    FIXTURE.with(|(grammar, schema, eos)| {
        let eos = *eos;
        let mut session = DecoderSession::with_schema(grammar, schema.clone())
            .expect("a fixed-engine grammar always accepts a schema overlay");
        // Driven in lockstep so the subset half of the contract is checked at the
        // same positions as the liveness half: `l2_properties.rs` iterates the L2
        // mask's set bits, which is vacuous on the very masks this target hunts.
        let mut plain = DecoderSession::new(grammar);

        for &choice in data {
            let completed_term = session
                .active_l2_position()
                .as_ref()
                .is_some_and(is_completed_term);
            let complete = session.is_complete();
            let mask = session.allowed_mask();
            let live: Vec<u32> = mask.iter_ones().collect();
            // The invariant. An empty mask is a deadlock: no token to sample, no EOS
            // to stop on.
            assert!(
                !live.is_empty(),
                "the L2 mask is empty: the host has no legal token and no way to stop"
            );
            // The overlay's own completion verdict must match the EOS bit it
            // publishes, or a host reading one disagrees with a host reading the other.
            assert_eq!(
                complete,
                mask.test(eos),
                "the published EOS bit and `is_complete` disagree"
            );

            // The subset half, over the *whole* set rather than only the token the
            // fuzzer goes on to pick — matching `tests/l2_liveness.rs`'s in-tree
            // sibling. Checking one token would leave a rule that widens L1 on a
            // bit the fuzzer never happens to choose invisible.
            let l1_mask = plain.allowed_mask();
            assert!(
                live.iter().all(|id| l1_mask.test(*id)),
                "L2 widened L1: the overlay admitted a token the grammar does not"
            );

            // The completed-term half (issue #296): a rule that permits only the
            // *continuations* of a whole term leaves the mask non-empty and still
            // strands the stream, which the assertion above cannot see. Nothing is
            // outstanding at such a position, so ending the term is L1's call.
            if completed_term {
                for id in TERM_END_BYTES.iter().map(|byte| id_of(*byte)).chain([eos]) {
                    assert!(
                        !l1_mask.test(id) || mask.test(id),
                        "the term is whole and L1 ends it with token {id}, but L2 cleared it: \
                         the stream can be extended here and never ended"
                    );
                }
            }

            // The fuzzer's byte picks the next token *out of the set L2 allows*, so
            // the walk can only ever reach positions the overlay itself endorsed.
            let id = live[usize::from(choice) % live.len()];
            if id == eos {
                break;
            }
            session
                .accept_token(id)
                .expect("a token the mask admitted must be admissible");
            plain
                .accept_token(id)
                .expect("L1 admits every token L2 admits");
        }
    });
});
