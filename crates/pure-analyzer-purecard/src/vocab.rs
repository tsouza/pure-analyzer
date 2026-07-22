//! The model vocabulary: token id → raw bytes.
//!
//! The host supplies this table at the boundary. [`CompiledGrammar`] consumes it
//! to build its per-state mask cache (M2, see `docs/spec/architecture.md` §4),
//! and [`DecoderSession::accept_token`] folds `vocab.bytes(id)` through the
//! byte-PDA one byte at a time. Lookup is a direct index into the token table;
//! there is no separate trie.
//!
//! [`CompiledGrammar`]: crate::CompiledGrammar
//! [`DecoderSession::accept_token`]: crate::DecoderSession::accept_token

/// An indexed table mapping token ids to their raw bytes, plus the EOS token id.
#[derive(Debug, Clone)]
pub struct Vocab {
    tokens: Vec<Vec<u8>>,
    eos: u32,
}

impl Vocab {
    /// Build from a list of token byte-strings and the EOS token id. The token
    /// id of `tokens[i]` is `i`.
    #[must_use]
    pub fn from_byte_tokens(tokens: Vec<Vec<u8>>, eos: u32) -> Self {
        Self { tokens, eos }
    }

    /// Raw bytes for token `id`, or `None` if `id` is out of range.
    #[must_use]
    pub fn bytes(&self, id: u32) -> Option<&[u8]> {
        self.tokens.get(id as usize).map(Vec::as_slice)
    }

    /// The EOS token id.
    #[must_use]
    pub fn eos(&self) -> u32 {
        self.eos
    }

    /// The number of tokens in the table.
    #[must_use]
    pub fn len(&self) -> usize {
        self.tokens.len()
    }

    /// Whether the table is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tokens.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::Vocab;

    fn sample() -> Vocab {
        Vocab::from_byte_tokens(vec![b"->".to_vec(), b"filter".to_vec(), b"".to_vec()], 2)
    }

    #[test]
    fn maps_ids_to_bytes() {
        let vocab = sample();
        assert_eq!(vocab.bytes(0), Some(b"->".as_slice()));
        assert_eq!(vocab.bytes(1), Some(b"filter".as_slice()));
    }

    #[test]
    fn out_of_range_id_is_none() {
        assert_eq!(sample().bytes(99), None);
    }

    #[test]
    fn reports_eos_and_len() {
        let vocab = sample();
        assert_eq!(vocab.eos(), 2);
        assert_eq!(vocab.len(), 3);
        assert!(!vocab.is_empty());
    }

    #[test]
    fn empty_table_is_empty() {
        assert!(Vocab::from_byte_tokens(vec![], 0).is_empty());
    }
}
