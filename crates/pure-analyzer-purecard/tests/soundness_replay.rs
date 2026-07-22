//! The M1 soundness killer-test (`docs/spec/testing.md` §8.1, `specs/m1-l1-grammar.md` G2).
//!
//! This drives the **real** shipped byte-PDA — [`purecard::DecoderSession`] — over
//! every gold `pure_text` value one byte at a time, and asserts the killer
//! property: the recogniser never reaches a dead state on a byte a gold query
//! actually emits, and is in an accepting state at end-of-stream. Because the
//! corpus is execution-verified gold, a dead state is not a test failure to work
//! around — it names a construct the grammar wrongly forbids, to be fixed in the
//! PDA (§8.6, the oracle-driven tightening loop), never by weakening the
//! assertion.
//!
//! The corpus is partitioned by [`Envelope`], and each partition's record count is
//! asserted against an exact named constant so shrinkage, corruption, or a
//! mis-partitioned query all redden the gate.
#![forbid(unsafe_code)]

use std::path::PathBuf;

// The corpus loader lives under `tests/support/` (ADR-0003), not in the published
// crate. Pull it in as a crate-local sibling so it resolves `use crate::error::…`
// against this binary's own root. The recogniser, session, envelope classifier,
// and `DecodeError` all ship in the crate and come in via `use purecard::…`.
#[path = "support/corpus.rs"]
mod corpus;
#[path = "support/error.rs"]
mod error;
#[path = "support/l1.rs"]
mod l1;

use corpus::load_gold;
use l1::l1_grammar;
use purecard::{ByteRecognizer, DecodeError, DecoderSession, Envelope};

/// Arm-A (relational envelope) record count. An exact named constant, not a
/// threshold (constitution §4): a mis-partition must redden the gate.
const ARM_A: usize = 4639;
/// Arm-C (class-navigation envelope) record count.
const ARM_C: usize = 395;
/// The full committed corpus size — the sum of the two arms.
const EXPECTED_GOLD_RECORDS: usize = ARM_A + ARM_C;

fn corpus_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("corpus/gold_queries.jsonl")
}

/// Drive `bytes` through a fresh [`DecoderSession`] one byte at a time.
///
/// Returns `Ok(())` if every byte was accepted and the session is complete at
/// end-of-stream; `Err` with the full oracle-tightening tuple otherwise, so a
/// soundness failure names the exact byte/state/stack that rejected it.
fn replay(bytes: &[u8]) -> Result<(), String> {
    let grammar = l1_grammar();
    let mut session = DecoderSession::new(&grammar);
    for &byte in bytes {
        if let Err(DecodeError::DeadState {
            offset,
            byte,
            state,
            stack_top,
        }) = session.accept_byte(byte)
        {
            return Err(format!(
                "dead state at offset {offset} on byte {byte:#04x} ({:?}) \
                 in state {state} with stack top {stack_top}",
                byte as char
            ));
        }
    }
    if session.is_complete() {
        Ok(())
    } else {
        Err("stream ended in a non-accepting state (unclosed query)".to_owned())
    }
}

#[test]
fn every_gold_query_streams_soundly_through_the_real_pda() {
    let records = load_gold(&corpus_path()).expect("open the committed gold corpus");
    let mut arm_a = 0usize;
    let mut arm_c = 0usize;

    for item in records {
        // Every line must parse — silent corpus corruption reddens the gate.
        let record = match item {
            Ok(record) => record,
            Err(err) => panic!("gold corpus failed to load: {err}"),
        };

        match Envelope::classify(&record.pure_text) {
            Some(Envelope::Relational) => arm_a += 1,
            Some(Envelope::ClassNav) => arm_c += 1,
            // The Spider gold corpus predates the modern Relation/Function API; no
            // gold here carries a `~`, so an arm-R classification means a `~` leaked
            // into `gold_queries.jsonl` (arm-R seeds live in the separate
            // modern-dialect corpus, ADR-0007).
            Some(Envelope::RelationApi) => panic!(
                "gold query {} unexpectedly classified as arm-R (contains `~`): {}",
                record.source_id, record.pure_text
            ),
            None => panic!(
                "gold query {} matches no envelope marker: {}",
                record.source_id, record.pure_text
            ),
        }

        // The killer property, over the real PDA. On failure the message carries
        // the (source_id, offset, byte, state, stack_top) tuple so the exact
        // construct to fix is visible without re-deriving it.
        if let Err(reason) = replay(record.pure_text.as_bytes()) {
            panic!(
                "SOUNDNESS: {} — {reason}\n  query: {}",
                record.source_id, record.pure_text
            );
        }
    }

    // Exact per-arm and total tallies (named constants, not magic literals).
    assert_eq!(arm_a, ARM_A, "arm-A partition count");
    assert_eq!(arm_c, ARM_C, "arm-C partition count");
    assert_eq!(
        arm_a + arm_c,
        EXPECTED_GOLD_RECORDS,
        "total gold record count"
    );
}

#[test]
fn the_deadness_channel_fires_on_malformed_input_with_a_correct_offset() {
    // Drive the SAME real session over a byte string that is *not* valid emitted
    // Pure — an extra ')' with no matching opener — to prove the single deadness
    // channel fires at the offending offset on the real automaton (not a stub).
    let malformed = "|X.all())";
    let grammar = l1_grammar();
    let mut session = DecoderSession::new(&grammar);
    let mut error = None;
    for &byte in malformed.as_bytes() {
        if let Err(err) = session.accept_byte(byte) {
            error = Some(err);
            break;
        }
    }
    let err = error.expect("the unmatched ')' must dead-end");
    // The offending closer is the trailing one (the `)` in `all()` is matched).
    let expected_offset = malformed
        .as_bytes()
        .iter()
        .rposition(|&byte| byte == b')')
        .expect("target present");
    assert!(
        matches!(
            err,
            DecodeError::DeadState { offset, byte, .. }
                if offset == expected_offset && byte == b')'
        ),
        "{err}"
    );
}
