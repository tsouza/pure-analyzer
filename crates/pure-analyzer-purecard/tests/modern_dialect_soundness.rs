//! Soundness lane for the **modern-dialect seed corpus**
//! (`corpus/modern_dialect_seeds.jsonl`).
//!
//! `corpus/gold_queries.jsonl` is the Spider-derived, execution-verified gold that
//! the L1 grammar was distilled from; it never exercised the modern Legend Pure
//! constructs the fine-tuned model also emits (the `%latest` milestoning literal —
//! gap report §5/G2; the `~` Relation/Function API — G1/arm-R). Those are seeded
//! here, in a *separate* file with distinct provenance (the pure-research gap
//! report, not the Spider pipeline), so the 5,034-query gold corpus and every doc
//! citation of its count stay untouched (ADR-0007, ADR-0008).
//!
//! The property is the same killer property as `soundness_replay.rs`: every seed
//! streams through the **real** shipped byte-PDA without a dead state and ends
//! accepting. Each seed also classifies to the envelope its `arm` field declares
//! (`A` → relational, `C` → class-navigation, `R` → relation-function API; an
//! unknown `arm` value is rejected), and the per-arm tallies are asserted against
//! exact named constants so a dropped or mis-labelled seed reddens the gate.
#![forbid(unsafe_code)]

use std::path::PathBuf;

#[path = "support/corpus.rs"]
mod corpus;
#[path = "support/error.rs"]
mod error;
#[path = "support/l1.rs"]
mod l1;

use corpus::load_gold;
use l1::l1_grammar;
use purecard::{ByteRecognizer, DecodeError, DecoderSession, Envelope};

/// Arm-A (relational) seed count in the modern-dialect corpus.
const SEED_ARM_A: usize = 0;
/// Arm-C (class-navigation) seed count — the `%latest` milestoning seeds (G2).
const SEED_ARM_C: usize = 5;
/// Arm-R (Relation/Function API) seed count — the `~` arm-R seeds (G1) plus the
/// three engine-validated nested-subquery shapes contributed for the gap report
/// (`join`/`extend` with a nested `Class.all()` and quoted-member access).
const SEED_ARM_R: usize = 14;
/// The full modern-dialect seed count — the sum of the three arms.
const EXPECTED_SEED_RECORDS: usize = SEED_ARM_A + SEED_ARM_C + SEED_ARM_R;

fn seed_corpus_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("corpus/modern_dialect_seeds.jsonl")
}

/// The envelope a record's declared `arm` field must classify to.
fn expected_envelope(arm: &str) -> Envelope {
    match arm {
        "A" => Envelope::Relational,
        "C" => Envelope::ClassNav,
        "R" => Envelope::RelationApi,
        other => panic!("modern-dialect seed has an unknown arm {other:?}"),
    }
}

/// Drive `bytes` through a fresh [`DecoderSession`] one byte at a time, returning
/// `Ok(())` iff every byte is accepted and the stream ends complete.
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
fn every_modern_dialect_seed_streams_soundly_through_the_real_pda() {
    let records = load_gold(&seed_corpus_path()).expect("open the modern-dialect seed corpus");
    let mut arm_a = 0usize;
    let mut arm_c = 0usize;
    let mut arm_r = 0usize;

    for item in records {
        let record = match item {
            Ok(record) => record,
            Err(err) => panic!("modern-dialect seed corpus failed to load: {err}"),
        };

        let want = expected_envelope(&record.arm);
        let got = Envelope::classify(&record.pure_text);
        assert_eq!(
            got,
            Some(want),
            "seed {} declares arm {:?} but classified as {:?}: {}",
            record.source_id,
            record.arm,
            got,
            record.pure_text
        );
        match want {
            Envelope::Relational => arm_a += 1,
            Envelope::ClassNav => arm_c += 1,
            Envelope::RelationApi => arm_r += 1,
        }

        if let Err(reason) = replay(record.pure_text.as_bytes()) {
            panic!(
                "SOUNDNESS: {} — {reason}\n  query: {}",
                record.source_id, record.pure_text
            );
        }
    }

    assert_eq!(arm_a, SEED_ARM_A, "modern-dialect arm-A seed count");
    assert_eq!(arm_c, SEED_ARM_C, "modern-dialect arm-C seed count");
    assert_eq!(arm_r, SEED_ARM_R, "modern-dialect arm-R seed count");
    assert_eq!(
        arm_a + arm_c + arm_r,
        EXPECTED_SEED_RECORDS,
        "total modern-dialect seed count"
    );
}
