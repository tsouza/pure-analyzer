//! Corpus-loader error type for the oracle harness.
//!
//! [`CorpusError`] is harness-only: loading the gold corpus is not decoder API,
//! so it stays under `tests/support/` (ADR-0003) rather than shipping in the
//! `purecard` crate. The decoder's own `DecodeError` moved into the published
//! core at M1 (`src/error.rs`); test binaries drive it via `use purecard::DecodeError`.

/// An error from loading the gold corpus.
#[derive(Debug, thiserror::Error)]
pub enum CorpusError {
    /// Underlying I/O failure reading the corpus file.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// A corpus line failed to parse as a [`crate::corpus::GoldRecord`].
    #[error("corpus json parse error at line {line}")]
    Json {
        /// 1-based line number of the offending record.
        line: usize,
        /// The underlying `serde_json` parse error.
        #[source]
        source: serde_json::Error,
    },
}

#[cfg(test)]
mod tests {
    use super::CorpusError;

    #[test]
    fn corpus_json_error_reports_line_number() {
        let source = serde_json::from_str::<serde_json::Value>("{").expect_err("malformed json");
        let err = CorpusError::Json { line: 42, source };
        assert!(err.to_string().contains("line 42"), "{err}");
    }
}
