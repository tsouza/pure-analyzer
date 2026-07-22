//! L2 test harness: a per-database schema loader and lexeme vocabulary shared by
//! the L2 soundness and precision lanes.
//!
//! The shipped core has no model tokenizer (a host supplies one). To exercise the
//! schema overlay the way a host would — narrowing a *token* mask at each step —
//! these lanes build a per-database vocabulary at **lexeme granularity** via the
//! shared [`lex`](crate::lex::lex) tokenizer, exactly the granularity
//! [`purecard::schema`]'s scope machine classifies.
//!
//! Shared via `#[path]` by the L2 lanes (soundness, properties, precision), which
//! must also include the `support/lex.rs` module this depends on.

use std::collections::BTreeMap;
use std::path::PathBuf;

use purecard::{Schema, Vocab};

/// The shared tokenizer, re-exported so the L2 lanes keep calling `l2::lex` and
/// `TokenVocab::build` can lex in-module.
pub use crate::lex::lex;

/// Load the committed JSON schema fixture for `db_id` and parse it through the
/// shipped `Schema::from_json` (so the §6.3 ingress is itself under test).
#[must_use]
pub fn load_schema(db_id: &str) -> Schema {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/schemas")
        .join(format!("{db_id}.json"));
    let json = std::fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("read schema fixture {}: {err}", path.display()));
    Schema::from_json(&json).unwrap_or_else(|err| panic!("parse schema fixture {db_id}: {err}"))
}

/// A per-database vocabulary: distinct token byte-strings mapped to dense ids,
/// with the reserved EOS bit one past the last id.
///
/// Shared scaffolding: the whole-lexeme-proxy L2 targets (`l2_soundness`,
/// `l2_precision`, `l2_properties`) drive it, while the BPE-split target reuses
/// only `lex`/`load_schema` from this module — so `dead_code` fires per-target
/// even though it is live across the suite.
#[allow(dead_code)]
pub struct TokenVocab {
    ids: BTreeMap<Vec<u8>, u32>,
    vocab: Vocab,
}

#[allow(dead_code)]
impl TokenVocab {
    /// Build a vocabulary from `extra` token byte-strings plus every token of
    /// every query in `queries`. `extra` lets a precision lane inject phantom
    /// identifiers/literals so they have ids to assert masked.
    #[must_use]
    pub fn build(queries: &[&str], extra: &[Vec<u8>]) -> Self {
        let mut ids = BTreeMap::new();
        let mut tokens: Vec<Vec<u8>> = Vec::new();
        let add = |tok: Vec<u8>, ids: &mut BTreeMap<Vec<u8>, u32>, tokens: &mut Vec<Vec<u8>>| {
            if !ids.contains_key(&tok) {
                ids.insert(tok.clone(), tokens.len() as u32);
                tokens.push(tok);
            }
        };
        for q in queries {
            for tok in lex(q) {
                add(tok, &mut ids, &mut tokens);
            }
        }
        for tok in extra {
            add(tok.clone(), &mut ids, &mut tokens);
        }
        let eos = tokens.len() as u32;
        Self {
            ids,
            vocab: Vocab::from_byte_tokens(tokens, eos),
        }
    }

    /// The built [`Vocab`].
    #[must_use]
    pub fn vocab(&self) -> Vocab {
        self.vocab.clone()
    }

    /// The id of a token's exact bytes, if present.
    #[must_use]
    pub fn id_of(&self, token: &[u8]) -> Option<u32> {
        self.ids.get(token).copied()
    }
}
