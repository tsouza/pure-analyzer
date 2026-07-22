//! The shared L1 test fixture: a grammar over an **empty** vocabulary, for the
//! lanes that drive only the byte-recognizer surface (which never consults the
//! vocab). Centralised here so the fixture — and its EOS sentinel — is defined
//! once, not copied across every byte-level test target (constitution §4, DRY).

use purecard::{CompiledGrammar, Vocab};

/// The `Vocab` EOS-token id an empty test vocabulary is built with. The
/// byte-recognizer lanes never consult the vocab, so the value is inert; naming
/// it documents the otherwise-unexplained sentinel the fixture passes.
pub const EMPTY_VOCAB_EOS: u32 = 0;

/// An L1-only grammar over an empty vocabulary — enough to drive the
/// byte-recognizer surface, which never consults the vocab.
#[must_use]
pub fn l1_grammar() -> CompiledGrammar {
    CompiledGrammar::compile(Vocab::from_byte_tokens(Vec::new(), EMPTY_VOCAB_EOS))
}
