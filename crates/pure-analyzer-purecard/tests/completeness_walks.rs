//! Hermetic completeness self-test (`specs/m1-l1-grammar.md` T8, G3).
//!
//! The seeded walk generator ([`walker`]) samples the byte-PDA's accepting
//! language by clone-and-probe. This lane asserts, without a network, that every
//! generated walk is a genuine accepted query of the *shipped* recogniser — driven
//! through a fresh [`DecoderSession`], not the generator's internal clone — and
//! that generation is deterministic and non-trivial. It is the hermetic floor
//! under the opt-in `legend` engine lane, which POSTs the same walks to a live
//! Legend stack (see `tests/legend_completeness.rs`).
#![forbid(unsafe_code)]

#[path = "support/l1.rs"]
mod l1;
#[path = "support/walker.rs"]
mod walker;

use l1::l1_grammar;
use purecard::{ByteRecognizer, DecoderSession};
use walker::{WALK_COUNT, generate_walks};

/// Every generated walk must stream cleanly through an independent
/// [`DecoderSession`] and end complete — the walker's clone-and-probe must agree,
/// byte for byte, with the recogniser the crate ships.
#[test]
fn every_generated_walk_is_accepted_by_the_shipped_recognizer() {
    let walks = generate_walks();
    assert_eq!(
        walks.len(),
        WALK_COUNT,
        "the generator must produce a full walk set"
    );
    for walk in &walks {
        let rendered = String::from_utf8_lossy(walk);
        let grammar = l1_grammar();
        let mut session = DecoderSession::new(&grammar);
        for (offset, &byte) in walk.iter().enumerate() {
            session.accept_byte(byte).unwrap_or_else(|err| {
                panic!("generated walk rejected at byte {offset}: {rendered:?} — {err}")
            });
        }
        assert!(
            session.is_complete(),
            "generated walk did not end in an accepting state: {rendered:?}"
        );
        let opener = walk.iter().copied().find(|b| !b.is_ascii_whitespace());
        assert!(
            matches!(opener, Some(b'|') | Some(b'{')),
            "an emitted-Pure query opens with `|` or `{{` (past leading whitespace): {rendered:?}"
        );
    }
}

/// The generator is a committed-seed stream: two calls yield byte-identical walks,
/// so the engine lane and this self-test see the same corpus and a regression is
/// reproducible in CI (constitution §2, no local-only state).
#[test]
fn generation_is_deterministic() {
    assert_eq!(
        generate_walks(),
        generate_walks(),
        "committed seeds must make walk generation reproducible"
    );
}

/// The walks exercise real structure, not just a bare `|X `: across the set, at
/// least one opens a delimiter frame and at least one drives a `->` step, so the
/// self-test is not vacuously green on trivial output.
#[test]
fn the_walk_set_exercises_pipeline_structure() {
    let walks = generate_walks();
    assert!(
        walks
            .iter()
            .any(|w| w.contains(&b'(') || w.contains(&b'[') || w.contains(&b'{')),
        "no generated walk opened a delimiter frame"
    );
    assert!(
        walks.iter().any(|w| w.windows(2).any(|pair| pair == b"->")),
        "no generated walk drove a `->` pipeline step"
    );
}
