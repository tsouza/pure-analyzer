//! The shared token-replay soundness oracle for the BPE lanes.
//!
//! The shipped core has no model tokenizer (a host supplies one). The BPE lanes
//! feed a whole token-id stream through a [`DecoderSession`] and assert the
//! killer property directly on the *token* mask the host consumes: before each
//! [`accept_token`](DecoderSession::accept_token) the gold id must be admissible
//! ([`allowed_mask`](DecoderSession::allowed_mask)`.test(id)`), and at
//! end-of-stream the query is complete with the reserved EOS bit set.
//!
//! Both the synthetic-split Tier-1 lane and the real-Qwen Tier-2 lane drive this
//! one helper (constitution §4, DRY): the property is identical, only the id stream
//! differs. Folding a real tokenizer's token strings back to bytes is a separate
//! concern, in the `byte_decode` support module.

use purecard::DecoderSession;

/// Replay `ids` through `session`, asserting the gold token is admissible at
/// every step and that the stream completes with `eos` set.
///
/// Returns `Err` with the step and id at the first violation, so a lane can
/// attribute a masked or rejected chunk to its exact query. `eos` is the
/// reserved EOS bit (`grammar.vocab().len()`), passed in rather than re-derived
/// here so the caller owns the one `V + 1` convention.
pub fn replay_tokens(
    session: &mut DecoderSession<'_>,
    ids: &[u32],
    eos: u32,
) -> Result<(), String> {
    for (step, &id) in ids.iter().enumerate() {
        if !session.allowed_mask().test(id) {
            return Err(format!("masked gold token id={id} at step {step}"));
        }
        session
            .accept_token(id)
            .map_err(|err| format!("rejected gold token id={id} at step {step}: {err}"))?;
    }
    if !session.is_complete() {
        return Err("stream ended in a non-accepting state".to_owned());
    }
    if !session.allowed_mask().test(eos) {
        return Err("EOS bit cleared at completion".to_owned());
    }
    Ok(())
}
