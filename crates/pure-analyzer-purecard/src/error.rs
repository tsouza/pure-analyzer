//! Error types for the shipped decoder core.
//!
//! [`DecodeError`] is the single channel by which the byte recognizer reports
//! that a stream has no valid continuation. It ships in the published `purecard`
//! crate (M1) so a consumer driving the recognizer can match on it; the
//! corpus-loader's `CorpusError` stays in the oracle harness (`tests/support/`),
//! since loading the gold corpus is not decoder API.

/// An error from driving the decoder core.
///
/// The byte-level variant, [`DecodeError::DeadState`], carries the full
/// oracle-tightening tuple: the offset and byte that were rejected, plus the
/// automaton `state` and `stack_top` at the point of rejection. Those last two
/// name *why* the byte was rejected, so a soundness failure over the gold corpus
/// points at the exact production the grammar wrongly forbids (see
/// `specs/m1-l1-grammar.md`, G4). This is a backward-compatible superset of M0's
/// `{ offset, byte }`.
///
/// The remaining three variants are the token-level channel of
/// [`accept_token`](crate::DecoderSession::accept_token) (M5): a
/// [`InadmissibleToken`](DecodeError::InadmissibleToken) — an in-range token the
/// mask legitimately cleared — a [`UnknownToken`](DecodeError::UnknownToken) for
/// an out-of-range id (a host-contract violation), and a
/// [`UnexpectedEos`](DecodeError::UnexpectedEos) for a premature end-of-stream.
#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    /// The recognizer had no valid continuation for `byte` at `offset`.
    #[error(
        "recognizer reached a dead state at offset {offset} (byte {byte:#04x}) \
         in state {state} with stack top {stack_top}"
    )]
    DeadState {
        /// Byte offset, taken from the recognizer's own consumed counter, at
        /// which deadness was reached.
        offset: usize,
        /// The byte that had no valid continuation.
        byte: u8,
        /// The automaton state the recognizer was in when the byte was rejected.
        state: &'static str,
        /// The frame on top of the recognizer's stack (or a sentinel for an
        /// empty stack) when the byte was rejected.
        stack_top: &'static str,
    },

    /// An **in-range** (`id < eos`), non-EOS token was rejected by
    /// [`accept_token`](crate::DecoderSession::accept_token) because its raw bytes
    /// dead-end the recognizer, so it cannot extend the stream. Every such id is
    /// cleared in [`allowed_mask`](crate::DecoderSession::allowed_mask): a host
    /// that respects the mask never triggers this — it is the *legitimate,
    /// mask-respecting* rejection, distinct from a host-contract violation. The
    /// session is left untouched (§8.5 rollback), so the host resamples. An
    /// **out-of-range** id is the separate [`UnknownToken`](DecodeError::UnknownToken)
    /// case, and a cleared **EOS** bit the separate
    /// [`UnexpectedEos`](DecodeError::UnexpectedEos) case.
    #[error("token id {id} is inadmissible: its bytes dead-end the recognizer")]
    InadmissibleToken {
        /// The rejected token id.
        id: u32,
    },

    /// A token `id` with **no entry in the host `Vocab`** (out of range: at or
    /// past the reserved EOS id) was submitted to
    /// [`accept_token`](crate::DecoderSession::accept_token). Unlike
    /// [`InadmissibleToken`](DecodeError::InadmissibleToken) — an in-range token
    /// the mask legitimately cleared — this signals a **host-contract
    /// violation**: the host passed an id its own vocabulary cannot name, so it
    /// ignored the mask or mis-sized it. Distinguishing it lets a host tell its
    /// own bug apart from routine masking. The session is left untouched.
    #[error("token id {id} is unknown: no entry in the host vocabulary")]
    UnknownToken {
        /// The out-of-range token id.
        id: u32,
    },

    /// End-of-stream was signalled (the reserved EOS id) while the query is not
    /// yet a complete parse — a premature stop. Raised by
    /// [`accept_token`](crate::DecoderSession::accept_token); the EOS bit is set
    /// in [`allowed_mask`](crate::DecoderSession::allowed_mask) only when the
    /// stream is in fact complete.
    #[error("end-of-stream is not legal here: the query is not yet complete")]
    UnexpectedEos,
}

#[cfg(test)]
mod tests {
    use super::DecodeError;

    #[test]
    fn dead_state_display_reports_offset_hex_byte_state_and_stack() {
        let err = DecodeError::DeadState {
            offset: 7,
            byte: 0x2c,
            state: "AfterArrow",
            stack_top: "Paren",
        };
        let shown = err.to_string();
        assert!(shown.contains("offset 7"), "{shown}");
        assert!(shown.contains("0x2c"), "{shown}");
        assert!(shown.contains("AfterArrow"), "{shown}");
        assert!(shown.contains("Paren"), "{shown}");
    }

    #[test]
    fn unknown_and_inadmissible_tokens_render_distinctly() {
        // The two id-rejection variants must carry the id and read differently,
        // so a host can tell its own contract violation (unknown) from a routine
        // masked reject (inadmissible).
        let unknown = DecodeError::UnknownToken { id: 42 }.to_string();
        let inadmissible = DecodeError::InadmissibleToken { id: 42 }.to_string();
        assert!(
            unknown.contains("42") && unknown.contains("unknown"),
            "{unknown}"
        );
        assert!(
            inadmissible.contains("42") && inadmissible.contains("inadmissible"),
            "{inadmissible}"
        );
        assert_ne!(unknown, inadmissible);
    }
}
