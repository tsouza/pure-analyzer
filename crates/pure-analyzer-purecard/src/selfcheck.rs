//! Tokenizer self-check: prove a host `Vocab` can *express* grammar-legal
//! queries (overview §11, the load-bearing "invisible soundness" risk).
//!
//! The pure core has no model tokenizer — the host supplies token → bytes via
//! [`Vocab`]. If the host's byte representation of a token disagrees
//! with the model's actual tokenization, the mask is computed over the wrong byte
//! stream and soundness breaks *invisibly*: the gold-soundness gate never observes
//! it, because it measures the core's own byte concatenation, not the host's. This
//! module closes that gap with an opt-in, side-effect-free check the host runs at
//! startup.
//!
//! [`self_check`] drives a set of canonical query byte-strings *through tokens* —
//! greedily longest-match-segmenting each sample against the host `Vocab`, then
//! asserting each segment is admissible ([`allowed_mask`] has its bit) and
//! accepted, and that the stream [`is_complete`] at end. A grammar-legal query the
//! host vocab **cannot segment**, that **dead-ends**, or that **never completes**
//! proves the declared token → bytes cannot express a legal query — i.e. host-vocab
//! vs model-tokenizer drift. It fails loud through a *distinct* [`SelfCheckError`]
//! (its own type, never a [`DecodeError`](crate::DecodeError) variant), so the
//! alarm is unmistakably "vocab drift," never a routine reject.
//!
//! [`self_check_smoke`] runs the embedded [`SMOKE`] set of ~4 canonical
//! gold-shaped queries, for a zero-argument startup assertion. The heavy
//! full-corpus round-trip (all 5034 gold queries) lives in `tests/` — no corpus is
//! compiled into the pure core; the smoke set is a compile-time constant.
//!
//! [`allowed_mask`]: crate::DecoderSession::allowed_mask
//! [`is_complete`]: crate::DecoderSession::is_complete

use crate::grammar::compiled::CompiledGrammar;
use crate::session::DecoderSession;
use crate::vocab::Vocab;

/// An embedded set of canonical, gold-shaped query byte-strings for
/// [`self_check_smoke`]: one arm-C class-navigation source, a filter, a project,
/// and an arm-A relational envelope. Deliberately tiny and inline — the pure core
/// ships no corpus file; the full 5034-query round-trip is a `tests/` integration
/// test.
pub const SMOKE: &[&[u8]] = &[
    b"|X.all()->take(1)",
    b"|X.all()->filter(x|$x.v > 1)",
    b"|X.all()->project([x|$x.name], ['n'])",
    b"|db::Db->tableReference('default', 'T')->tableToTDS()->limit(1)",
];

/// A distinct failure of the tokenizer self-check: the host `Vocab` cannot
/// express a grammar-legal query, so it drifts from the model's tokenization.
///
/// A *separate* type from [`DecodeError`](crate::DecodeError) on purpose — a
/// `SelfCheckError` is an alarm about the host's declared vocabulary, never a
/// routine per-step reject. Every variant names the offending query
/// (`query_index`) and byte position so the drift is locatable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SelfCheckError {
    /// No vocab token's bytes match the query's byte prefix at `pos`: the host
    /// vocabulary cannot cover a byte the query legally emits.
    #[error("query {query_index}: no vocab token matches the byte prefix at position {pos}")]
    Unsegmentable {
        /// Index of the failing sample in the checked set.
        query_index: usize,
        /// Byte offset into the query where segmentation stalled.
        pos: usize,
    },

    /// The token segmented at `pos` (`id`) is not admissible there — its bit is
    /// clear in the mask, or accepting it dead-ends the recognizer. A legal query
    /// the host vocab tokenizes into an illegal step.
    #[error(
        "query {query_index}: token {id} at position {pos} is not admissible \
         (masked out or dead-ends the recognizer)"
    )]
    DeadEnd {
        /// Index of the failing sample in the checked set.
        query_index: usize,
        /// Byte offset into the query where the offending token starts.
        pos: usize,
        /// The offending token id.
        id: u32,
    },

    /// Every byte segmented cleanly, but the stream ended in a non-accepting
    /// configuration after `consumed` bytes — the vocab expresses a *prefix* of
    /// the query but never reaches a complete parse.
    #[error("query {query_index}: stream ended incomplete after consuming {consumed} bytes")]
    Incomplete {
        /// Index of the failing sample in the checked set.
        query_index: usize,
        /// Bytes consumed before the stream was found incomplete.
        consumed: usize,
    },
}

/// The id of the longest vocab token whose bytes are a non-empty prefix of
/// `remaining`, or `None` if none is. Empty tokens are ignored so segmentation
/// always advances (an empty token is a prefix of everything but consumes no
/// bytes).
fn longest_match(vocab: &Vocab, remaining: &[u8]) -> Option<u32> {
    let mut best: Option<(u32, usize)> = None;
    for id in 0..vocab.len() as u32 {
        let token = vocab.bytes(id).unwrap_or(&[]);
        if !token.is_empty()
            && remaining.starts_with(token)
            && best.is_none_or(|(_, len)| token.len() > len)
        {
            best = Some((id, token.len()));
        }
    }
    best.map(|(id, _)| id)
}

/// Round-trip every sample in `samples` through the host `Vocab` and the byte-PDA,
/// proving the declared vocabulary can express each grammar-legal query.
///
/// For each sample: open a fresh [`DecoderSession`],
/// greedily longest-match-segment the remaining bytes against the grammar's
/// `Vocab` at the current offset, assert each segment's bit is set in
/// [`allowed_mask`](crate::DecoderSession::allowed_mask) then
/// [`accept_token`](crate::DecoderSession::accept_token) it, and at end assert
/// [`is_complete`](crate::DecoderSession::is_complete). This exercises the *token*
/// surface (not just the byte-PDA), so it observes host-vocab vs model-tokenizer
/// drift a byte-only replay could not.
///
/// # Errors
/// Returns the first [`SelfCheckError`] encountered: [`Unsegmentable`] if no token
/// covers the byte prefix, [`DeadEnd`] if a segmented token is inadmissible, or
/// [`Incomplete`] if the stream ends open.
///
/// [`Unsegmentable`]: SelfCheckError::Unsegmentable
/// [`DeadEnd`]: SelfCheckError::DeadEnd
/// [`Incomplete`]: SelfCheckError::Incomplete
pub fn self_check(grammar: &CompiledGrammar, samples: &[&[u8]]) -> Result<(), SelfCheckError> {
    let vocab = grammar.vocab();
    for (query_index, &query) in samples.iter().enumerate() {
        let mut session = DecoderSession::new(grammar);
        let mut consumed = 0;
        // Advance a shrinking cursor over the remaining bytes. `longest_match`
        // only ever returns a *non-empty* token that is a prefix of `rest`, so
        // `rest` strictly shrinks each iteration — termination does not depend on
        // the `consumed` accumulator (which is used only for error positions).
        let mut rest = query;
        while !rest.is_empty() {
            let id = longest_match(vocab, rest).ok_or(SelfCheckError::Unsegmentable {
                query_index,
                pos: consumed,
            })?;
            if !session.allowed_mask().test(id) {
                return Err(SelfCheckError::DeadEnd {
                    query_index,
                    pos: consumed,
                    id,
                });
            }
            if session.accept_token(id).is_err() {
                return Err(SelfCheckError::DeadEnd {
                    query_index,
                    pos: consumed,
                    id,
                });
            }
            let n = vocab.bytes(id).map_or(0, <[u8]>::len);
            rest = &rest[n..];
            consumed += n;
        }
        if !session.is_complete() {
            return Err(SelfCheckError::Incomplete {
                query_index,
                consumed,
            });
        }
    }
    Ok(())
}

/// Run [`self_check`] over the embedded [`SMOKE`] set — a zero-argument startup
/// assertion that the host `Vocab` can express the canonical gold-shaped queries.
///
/// # Errors
/// Returns the first [`SelfCheckError`] from [`self_check`] over [`SMOKE`].
pub fn self_check_smoke(grammar: &CompiledGrammar) -> Result<(), SelfCheckError> {
    self_check(grammar, SMOKE)
}

// The lexeme tokenizer has a single home in `tests/support/lex.rs` (constitution
// §4, DRY): the L2 lanes, the criterion bench, and the self-check corpus lane all
// `#[path]`-include that one file. These unit tests reuse it too, rather than carry
// a second, drift-prone copy. It stays test-only scaffolding — the shipped core has
// no model tokenizer (the host supplies token → bytes), so it cannot move into a
// shipped `src/` module without leaking a lexer into the pure core's public surface.
// The core's own byte-segmentation (`longest_match`) is a distinct algorithm — vocab
// longest-match, not lexeme tokenization — and stays in this file.
#[cfg(test)]
#[path = "../tests/support/lex.rs"]
mod lex_support;

#[cfg(test)]
mod tests {
    use super::{SMOKE, SelfCheckError, longest_match, self_check, self_check_smoke};
    use crate::grammar::compiled::CompiledGrammar;
    use crate::vocab::Vocab;

    /// Lexeme-tokenize `text` via the canonical [`lex_support::lex`](super::lex_support::lex).
    /// The self-check samples are all valid UTF-8, so the byte slice round-trips
    /// through `str`.
    fn lex(text: &[u8]) -> Vec<Vec<u8>> {
        super::lex_support::lex(
            std::str::from_utf8(text).expect("self-check samples are valid UTF-8"),
        )
    }

    /// Build a faithful `Vocab` from the lexemes of `queries`, mapped to dense
    /// ids, EOS one past the last id.
    fn vocab_of(queries: &[&[u8]]) -> Vocab {
        let mut tokens: Vec<Vec<u8>> = Vec::new();
        for &q in queries {
            for tok in lex(q) {
                if !tokens.contains(&tok) {
                    tokens.push(tok);
                }
            }
        }
        let eos = tokens.len() as u32;
        Vocab::from_byte_tokens(tokens, eos)
    }

    #[test]
    fn a_faithful_vocab_passes_the_smoke_check() {
        // A vocab lexed from exactly the smoke queries can express each of them.
        let grammar = CompiledGrammar::compile(vocab_of(SMOKE));
        assert_eq!(self_check_smoke(&grammar), Ok(()));
    }

    #[test]
    fn a_faithful_vocab_passes_an_explicit_sample_set() {
        let samples: &[&[u8]] = &[b"|X.all()->take(3)", b"|X.all()->name"];
        let grammar = CompiledGrammar::compile(vocab_of(samples));
        assert_eq!(self_check(&grammar, samples), Ok(()));
    }

    #[test]
    fn dropping_the_close_paren_token_is_unsegmentable_at_that_byte() {
        // Drop the `)` token: the query `|X.all()->take(1)` can no longer cover the
        // final `)`, so segmentation stalls exactly there — the guard catches the
        // drift, and names the offending byte position.
        let sample: &[u8] = b"|X.all()->take(1)";
        // Lex the sample, drop every `)` token, and de-duplicate preserving order.
        let mut seen: Vec<Vec<u8>> = Vec::new();
        for t in lex(sample) {
            if t.as_slice() != b")" && !seen.contains(&t) {
                seen.push(t);
            }
        }
        let eos = seen.len() as u32;
        let grammar = CompiledGrammar::compile(Vocab::from_byte_tokens(seen, eos));
        // The first `)` is inside `all()`, at byte offset 7.
        let expected_pos = sample.iter().position(|&b| b == b')').expect("a paren");
        assert_eq!(
            self_check(&grammar, &[sample]),
            Err(SelfCheckError::Unsegmentable {
                query_index: 0,
                pos: expected_pos,
            })
        );
    }

    #[test]
    fn a_tokenizable_but_grammar_illegal_segmentation_dead_ends() {
        // A vocab that tokenizes a byte string the grammar rejects: `|X.all()` is a
        // complete query (AfterValue, empty stack), and a trailing `]` closer is
        // dead there. The vocab segments it cleanly, so the failure is a DeadEnd at
        // the closer — proving the DeadEnd path fires with the offending pos+id.
        //
        // (A DeadEnd cannot arise from a *sound* sample: `accept_token(id)` folds
        // exactly the sample's own bytes, which stream soundly, so an inadmissible
        // segment requires the sample itself to be grammar-illegal.)
        let sample: &[u8] = b"|X.all()]";
        let vocab = Vocab::from_byte_tokens(vec![b"|X.all()".to_vec(), b"]".to_vec()], 2);
        let closer_id = 1;
        let grammar = CompiledGrammar::compile(vocab);
        assert_eq!(
            self_check(&grammar, &[sample]),
            Err(SelfCheckError::DeadEnd {
                query_index: 0,
                pos: b"|X.all()".len(),
                id: closer_id,
            })
        );
    }

    #[test]
    fn a_partial_query_reports_incomplete_with_the_consumed_count() {
        // A vocab that fully segments an *unclosed* query (`->take(1`, Paren still
        // open) reaches end-of-bytes in a non-accepting state.
        let sample: &[u8] = b"|X.all()->take(1";
        let grammar = CompiledGrammar::compile(vocab_of(&[sample]));
        assert_eq!(
            self_check(&grammar, &[sample]),
            Err(SelfCheckError::Incomplete {
                query_index: 0,
                consumed: sample.len(),
            })
        );
    }

    #[test]
    fn the_query_index_names_the_failing_sample() {
        // The second sample fails, so the reported index is 1 — the check reports
        // *which* sample drifted, not just that one did.
        let samples: &[&[u8]] = &[b"|X.all()->take(1)", b"|X.all()->take(1"];
        let grammar = CompiledGrammar::compile(vocab_of(samples));
        assert_eq!(
            self_check(&grammar, samples),
            Err(SelfCheckError::Incomplete {
                query_index: 1,
                consumed: samples[1].len(),
            })
        );
    }

    #[test]
    fn each_error_variant_renders_its_query_and_position() {
        // Every variant's Display names the offending query and position, so an
        // operator can locate the drift from the message alone.
        let unseg = SelfCheckError::Unsegmentable {
            query_index: 2,
            pos: 5,
        }
        .to_string();
        assert!(
            unseg.contains("query 2") && unseg.contains("position 5"),
            "{unseg}"
        );

        let dead = SelfCheckError::DeadEnd {
            query_index: 3,
            pos: 8,
            id: 42,
        }
        .to_string();
        assert!(
            dead.contains("query 3") && dead.contains("position 8") && dead.contains("42"),
            "{dead}"
        );

        let incomplete = SelfCheckError::Incomplete {
            query_index: 1,
            consumed: 16,
        }
        .to_string();
        assert!(
            incomplete.contains("query 1") && incomplete.contains("16"),
            "{incomplete}"
        );
    }

    #[test]
    fn longest_match_prefers_the_longest_token_and_skips_empty() {
        // `->take` must win over `->` at a shared prefix, and an empty token is
        // never selected (it would stall segmentation).
        let vocab =
            Vocab::from_byte_tokens(vec![b"->".to_vec(), b"->take".to_vec(), b"".to_vec()], 3);
        assert_eq!(longest_match(&vocab, b"->take(1)"), Some(1));
        assert_eq!(longest_match(&vocab, b"->x"), Some(0));
        assert_eq!(longest_match(&vocab, b"zzz"), None);
    }

    #[test]
    fn longest_match_breaks_length_ties_toward_the_first_id() {
        // Two distinct ids carrying identical bytes both match; the first-seen id
        // wins (the `>` keeps the earlier one on a tie, never replacing it).
        let vocab = Vocab::from_byte_tokens(vec![b"ab".to_vec(), b"ab".to_vec()], 2);
        assert_eq!(longest_match(&vocab, b"abc"), Some(0));
    }

    #[test]
    fn self_check_smoke_fails_on_a_vocab_that_cannot_express_the_smoke_set() {
        // An empty vocabulary cannot segment the first smoke query's very first
        // byte, so `self_check_smoke` must return an error — pinning that it
        // actually runs the check rather than trivially succeeding.
        let grammar = CompiledGrammar::compile(Vocab::from_byte_tokens(Vec::new(), 0));
        assert_eq!(
            self_check_smoke(&grammar),
            Err(SelfCheckError::Unsegmentable {
                query_index: 0,
                pos: 0,
            })
        );
    }
}
