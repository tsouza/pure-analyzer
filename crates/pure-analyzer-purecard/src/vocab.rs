//! The model vocabulary: token id → raw bytes.
//!
//! The host supplies this table at the boundary. [`CompiledGrammar`] consumes it
//! to build its per-state mask cache (see `docs/spec/architecture.md` §4),
//! and [`DecoderSession::accept_token`] folds `vocab.bytes(id)` through the
//! byte-PDA one byte at a time. Lookup is a direct index into the token table;
//! there is no separate trie.
//!
//! [`CompiledGrammar`]: crate::CompiledGrammar
//! [`DecoderSession::accept_token`]: crate::DecoderSession::accept_token

/// An indexed table mapping token ids to their raw bytes.
///
/// EOS is not one of these ids: the decoder reserves the bit one past the last
/// token (`CompiledGrammar::eos_bit`), so a host's own EOS id is neither part
/// of this table nor supplied to it.
#[derive(Debug, Clone)]
pub struct Vocab {
    tokens: Vec<Vec<u8>>,
}

impl Vocab {
    /// Build from a list of token byte-strings. The token id of `tokens[i]` is `i`.
    #[must_use]
    pub fn from_byte_tokens(tokens: Vec<Vec<u8>>) -> Self {
        Self { tokens }
    }

    /// Raw bytes for token `id`, or `None` if `id` is out of range.
    #[must_use]
    pub fn bytes(&self, id: u32) -> Option<&[u8]> {
        self.tokens.get(id as usize).map(Vec::as_slice)
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
        Vocab::from_byte_tokens(vec![b"->".to_vec(), b"filter".to_vec(), b"".to_vec()])
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
    fn reports_len() {
        let vocab = sample();
        assert_eq!(vocab.len(), 3);
        assert!(!vocab.is_empty());
    }

    #[test]
    fn empty_table_is_empty() {
        assert!(Vocab::from_byte_tokens(vec![]).is_empty());
    }
}
