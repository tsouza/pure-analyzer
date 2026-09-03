//! A single-byte vocabulary plus the "drive text through L1 and L2 in lockstep"
//! primitive, shared by every lane that walks a *specific* byte string through
//! the overlay and asserts each byte stays L2-admissible along the way —
//! `l2_liveness.rs`'s seeded/gold-anchored witnesses and
//! `l2_value_shape_matrix.rs`'s value-shape sweep (`docs/spec/schema.md` §6.7's
//! third invariant).
//!
//! Factored out of `l2_liveness.rs` (issue #367/#377/#385/#391's shared
//! infrastructure ask): both lanes needed the identical byte-granular vocabulary
//! and the identical "walk this text, fail loudly at the first masked byte"
//! driver, and duplicating either would drift the two lanes' notion of what
//! "L2-admissible" means (constitution §4, DRY).

use std::sync::OnceLock;

use purecard::{BitMask, CompiledGrammar, DecoderSession, Schema, Vocab};

/// The vocabulary: one token per printable ASCII byte, `' '` (0x20) through `'~'`
/// (0x7e), plus the non-printing whitespace bytes. Together that is exactly the
/// grammar's own alphabet — printable ASCII and `WS` (`b" \t\n\r"`, `pda.rs`) —
/// so every continuation the byte-PDA admits is spellable by some token. A
/// byte-granular vocabulary is also the adversarial case for an overlay whose
/// rules read whole lexemes: a whole-token vocabulary can never leave the stream
/// parked on a lone `$` sigil, a lone `.` that opens a float, or — issue #391 —
/// the second digit of a multi-digit date literal.
const FIRST_BYTE: u8 = 0x20;
const LAST_BYTE: u8 = 0x7e;
const LAYOUT_BYTES: &[u8] = b"\t\n\r";

/// Every byte the vocabulary carries, in id order. Built once: callers that walk
/// many witnesses call [`id_of`] once per byte, and rebuilding the table per call
/// would allocate per step.
fn vocab_bytes() -> &'static [u8] {
    static BYTES: OnceLock<Vec<u8>> = OnceLock::new();
    BYTES.get_or_init(|| {
        (FIRST_BYTE..=LAST_BYTE)
            .chain(LAYOUT_BYTES.iter().copied())
            .collect()
    })
}

/// The single-byte vocabulary plus its reserved EOS id.
pub fn byte_vocab() -> (Vocab, u32) {
    let tokens: Vec<Vec<u8>> = vocab_bytes().iter().map(|byte| vec![*byte]).collect();
    let eos = tokens.len() as u32;
    (Vocab::from_byte_tokens(tokens), eos)
}

/// The token id of `byte` in the single-byte vocabulary built by [`byte_vocab`].
pub fn id_of(byte: u8) -> u32 {
    let id = vocab_bytes()
        .iter()
        .position(|candidate| *candidate == byte)
        .unwrap_or_else(|| panic!("byte {byte:#04x} is outside the vocabulary"));
    id as u32
}

/// Drive both an L2 and an L1 session over `text`, asserting every byte is
/// **L2-admissible** on the way (so the position reached is genuinely
/// reachable, and — the property this exists for — that the schema overlay
/// never masks a byte a legal witness owes), and return the two sessions parked
/// at the position after the last byte.
///
/// Panics with the exact witness text and the offset of the first masked byte,
/// which is the failure message a witness like issue #391's
/// `%2026-01-15` needs to be locatable without re-deriving it from a bisection.
pub fn drive<'g>(
    grammar: &'g CompiledGrammar,
    schema: &Schema,
    text: &str,
) -> (DecoderSession<'g>, DecoderSession<'g>) {
    let mut l2 =
        DecoderSession::with_schema(grammar, schema.clone()).expect("grammar is fixed-engine");
    let mut l1 = DecoderSession::new(grammar);
    for (offset, byte) in text.bytes().enumerate() {
        let id = id_of(byte);
        assert!(
            l2.allowed_mask().test(id),
            "witness {text:?} is not an L2-admissible walk: byte {:?} at {offset} is masked",
            byte as char
        );
        l2.accept_token(id).expect("L2-admitted token accepts");
        l1.accept_token(id).expect("L1 admits what L2 admits");
    }
    (l2, l1)
}

/// The ids set in `mask`, for a legible assertion message. `#[allow(dead_code)]`
/// because not every lane that pulls in this module reads a mask's live set
/// directly (`l2_value_shape_matrix.rs` asserts entirely through [`drive`]'s own
/// panic), the same per-target allowance `tests/support/l2_rules.rs` documents
/// for its own registry.
#[allow(dead_code)]
pub fn live(mask: &BitMask) -> Vec<u32> {
    mask.iter_ones().collect()
}
