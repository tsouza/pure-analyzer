//! The decode session: the byte-PDA, the per-step token mask, and the offset
//! bookkeeping the recognizer contract needs.
//!
//! [`DecoderSession`] is the shipped implementation of [`ByteRecognizer`]. It
//! wraps a [`Pda`] and a byte-offset counter, folding each byte through the
//! automaton and translating a dead state into a [`DecodeError::DeadState`]
//! carrying the offset at which the stream ran out of continuations.
//!
//! It borrows a [`CompiledGrammar`] and exposes the masking surface
//! (`docs/spec/architecture.md` §4, §9): [`allowed_mask`](DecoderSession::allowed_mask)
//! returns tokens that keep the stream inside the emitted-subset recognizer, and
//! [`accept_token`](DecoderSession::accept_token) advances by a whole token,
//! rolling back untouched if the token is inadmissible. The L2 schema overlay
//! is installed by [`with_schema`](DecoderSession::with_schema), which takes a
//! [`Schema`], and [`allowed_mask`](DecoderSession::allowed_mask) intersects the
//! syntactic L1 mask with the schema-legal set at the identifier and operand
//! positions covered by the shipped L2 rules (§3.1) — N3, N1/N2, N6, T2, T3,
//! and the numeric/string levers of T1; other operand-type rules pass through.
//! This is an additive narrowing that leaves the `schema`-is-`None` (L1-only)
//! path untouched.

use crate::error::DecodeError;
use crate::grammar::compile::RtnPda;
use crate::grammar::compiled::{CompiledGrammar, Engine};
use crate::grammar::pda::{Frame, Pda};
use crate::mask::BitMask;
use crate::recognizer::ByteRecognizer;
use crate::schema::Schema;
use crate::schema::narrow::{
    NarrowCache, admits_eos, narrow_fused_into, narrow_into, narrow_sigil_into,
};
use crate::schema::scope::{L2Position, ScopeTracker};

/// Which automaton a [`DecoderSession`] drives.
///
/// [`Cursor::Spec`] carries no [`ScopeTracker`]-visible position information —
/// the L2 schema overlay is implemented against [`Cursor::Fixed`]'s named
/// [`State`](crate::grammar::pda::State)s only (see
/// `docs/decisions/0010-declarative-transition-table-spec.md`), so
/// [`DecoderSession::with_schema`] refuses a [`Cursor::Spec`] grammar rather
/// than silently skipping the overlay.
#[derive(Debug, Clone)]
enum Cursor<'g> {
    /// The hand-written `grammar::pda` automaton.
    Fixed {
        /// The live automaton.
        pda: Pda,
        /// A reused scratch stack for the per-step deferred-token re-probe.
        scratch: Vec<Frame>,
    },
    /// An automaton lowered from a supplied grammar spec (L1-only).
    Spec {
        /// The live automaton.
        pda: RtnPda<'g>,
        /// A reused scratch stack for the per-step deferred-token re-probe.
        scratch: Vec<u32>,
    },
}

impl<'g> Cursor<'g> {
    fn new(grammar: &'g CompiledGrammar) -> Self {
        match grammar.engine() {
            Engine::Fixed => Cursor::Fixed {
                pda: Pda::new(),
                scratch: Vec::new(),
            },
            Engine::Spec(automaton) => Cursor::Spec {
                pda: RtnPda::new(automaton),
                scratch: Vec::new(),
            },
        }
    }

    /// The memoized vocabulary partition for the current state.
    fn cached<'a>(&self, grammar: &'a CompiledGrammar) -> &'a crate::grammar::compiled::Cached {
        match self {
            Cursor::Fixed { pda, .. } => grammar.cached(pda.state()),
            Cursor::Spec { pda, .. } => grammar.cached_spec(pda.automaton(), pda.state()),
        }
    }

    fn stack_top_present(&self) -> bool {
        match self {
            Cursor::Fixed { pda, .. } => pda.stack_top().is_some(),
            Cursor::Spec { pda, .. } => pda.stack_top().is_some(),
        }
    }

    /// Whether `bytes` keeps the live configuration alive, reusing this
    /// cursor's own scratch stack.
    fn admits(&mut self, bytes: &[u8]) -> bool {
        match self {
            Cursor::Fixed { pda, scratch } => pda.admits(bytes, scratch),
            Cursor::Spec { pda, scratch } => pda.admits(bytes, scratch),
        }
    }

    fn is_accepting(&self) -> bool {
        match self {
            Cursor::Fixed { pda, .. } => pda.is_accepting(),
            Cursor::Spec { pda, .. } => pda.is_accepting(),
        }
    }

    fn reset(&mut self) {
        match self {
            Cursor::Fixed { pda, .. } => pda.reset(),
            Cursor::Spec { pda, .. } => pda.reset(),
        }
    }

    /// Feed one byte, advancing the live configuration. On rejection, returns
    /// the state/stack-top names for [`DecodeError::DeadState`] and leaves
    /// the cursor unchanged (both engines' `advance` already guarantee this).
    fn advance_byte(&mut self, byte: u8) -> Result<(), (String, String)> {
        match self {
            Cursor::Fixed { pda, .. } => pda
                .advance(byte)
                .map_err(|dead| (dead.state.to_string(), dead.stack_top.to_string())),
            Cursor::Spec { pda, .. } => {
                let automaton = pda.automaton();
                if pda.advance(byte) {
                    Ok(())
                } else {
                    Err((
                        automaton.state_name(pda.state()).to_string(),
                        automaton.frame_name(pda.stack_top()).to_string(),
                    ))
                }
            }
        }
    }

    /// Fold `bytes` through a clone of the live configuration, committing
    /// only on full success — the whole-token analogue of `advance_byte`.
    fn try_accept_token(&mut self, bytes: &[u8]) -> bool {
        match self {
            Cursor::Fixed { pda, .. } => {
                let mut probe = pda.clone();
                for &byte in bytes {
                    if probe.advance(byte).is_err() {
                        return false;
                    }
                }
                *pda = probe;
                true
            }
            Cursor::Spec { pda, .. } => {
                let mut probe = pda.clone();
                for &byte in bytes {
                    if !probe.advance(byte) {
                        return false;
                    }
                }
                *pda = probe;
                true
            }
        }
    }
}

/// A byte-at-a-time decode session over the emitted-Pure grammar, bound to a
/// [`CompiledGrammar`].
///
/// Construct one with [`DecoderSession::new`], then either drive it byte-wise
/// through [`ByteRecognizer`] or token-wise through
/// [`accept_token`](DecoderSession::accept_token), reading the legal next-token
/// set from [`allowed_mask`](DecoderSession::allowed_mask) at each step.
/// [`reset`](DecoderSession::reset) returns it to a fresh stream while keeping
/// the automaton's stack and the mask buffer allocated.
#[derive(Debug, Clone)]
pub struct DecoderSession<'g> {
    cursor: Cursor<'g>,
    offset: usize,
    grammar: &'g CompiledGrammar,
    /// The owned, reused mask buffer `allowed_mask` refills each step — sized to
    /// [`CompiledGrammar::mask_len`] (EOS bit included) so no per-step allocation
    /// is needed.
    mask: BitMask,
    /// A second reused buffer the L2 overlay refills in place with the
    /// schema-legal set, then intersects into `mask` — so narrowing allocates no
    /// per-step mask (§4.3). Sized, like `mask`, to
    /// [`CompiledGrammar::mask_len`]. Left untouched on the L1-only (`schema` is
    /// `None`) path.
    narrow_buf: BitMask,
    /// The optional L2 schema overlay. `None` is L1-only: the
    /// schema-narrowing block in [`allowed_mask`](DecoderSession::allowed_mask) is
    /// skipped entirely, so there is zero added per-step cost.
    schema: Option<Schema>,
    /// The §6.4 scope machine, advanced in lockstep with
    /// [`accept_token`](DecoderSession::accept_token). Inert (never consulted)
    /// when `schema` is `None`.
    tracker: ScopeTracker,
    /// The L2 per-`(schema, rule)` mask memo (§4.5): the anchor scan for a source
    /// or member set is a constant, computed once and copied thereafter. Empty and
    /// untouched on the L1-only (`schema` is `None`) path.
    narrow_cache: NarrowCache,
    /// Whether the L2 overlay permits the stream to end here — the EOS half of
    /// the narrow, refreshed in lockstep with `tracker` by
    /// [`accept_token`](DecoderSession::accept_token) so
    /// [`is_complete`](DecoderSession::is_complete) can stay `&self` while still
    /// reading it. `true` (unconstrained) on the L1-only path and before the
    /// first token, which is what keeps the byte-wise [`ByteRecognizer`] surface
    /// — which never advances `tracker` — on exactly its previous L1 semantics.
    l2_eos: bool,
}

impl<'g> DecoderSession<'g> {
    /// A fresh session at the start of a stream, masking against `grammar`, with an
    /// optional L2 `schema` overlay — the one place the session's field layout and
    /// buffer sizing live, so [`new`](DecoderSession::new) and
    /// [`with_schema`](DecoderSession::with_schema) cannot drift apart.
    fn build(grammar: &'g CompiledGrammar, schema: Option<Schema>) -> Self {
        Self {
            cursor: Cursor::new(grammar),
            offset: 0,
            grammar,
            mask: BitMask::with_len(grammar.mask_len()),
            narrow_buf: BitMask::with_len(grammar.mask_len()),
            schema,
            tracker: ScopeTracker::new(),
            narrow_cache: NarrowCache::new(),
            l2_eos: true,
        }
    }

    /// A fresh session at the start of a stream, masking against `grammar`.
    ///
    /// L1-only: no schema, so the mask is the pure syntactic next-token set.
    /// Works for a grammar built with either
    /// [`CompiledGrammar::compile`](crate::grammar::compiled::CompiledGrammar::compile)
    /// or
    /// [`CompiledGrammar::from_spec`](crate::grammar::compiled::CompiledGrammar::from_spec).
    #[must_use]
    pub fn new(grammar: &'g CompiledGrammar) -> Self {
        Self::build(grammar, None)
    }

    /// A fresh session that also applies the implemented L2 schema-overlay rules
    /// against `schema` (`docs/spec/schema.md` §6). At covered identifier and
    /// operand positions, the emitted-subset mask is intersected with the
    /// schema-derived set; deferred positions pass through. The overlay only ever
    /// *narrows* — the additive counterpart to [`new`](DecoderSession::new), which
    /// stays L1-only and byte-compatible. It is not a full-query
    /// schema-validity or compiler-success guarantee.
    ///
    /// # Errors
    /// Returns [`DecodeError::SchemaRequiresFixedGrammar`] if `grammar` was
    /// built with
    /// [`CompiledGrammar::from_spec`](crate::grammar::compiled::CompiledGrammar::from_spec) —
    /// the L2 overlay is implemented against the fixed built-in grammar's
    /// named states only.
    pub fn with_schema(grammar: &'g CompiledGrammar, schema: Schema) -> Result<Self, DecodeError> {
        if matches!(grammar.engine(), Engine::Spec(_)) {
            return Err(DecodeError::SchemaRequiresFixedGrammar);
        }
        Ok(Self::build(grammar, Some(schema)))
    }

    /// The number of bytes consumed since the last [`reset`](DecoderSession::reset).
    #[must_use]
    pub fn offset(&self) -> usize {
        self.offset
    }

    /// The set of token ids that keep the stream on a non-dead path through the
    /// fixed emitted-subset PDA at the current position: every token whose raw
    /// bytes leave the byte-PDA
    /// non-dead (§4.4), with the reserved EOS bit set iff
    /// [`is_complete`](DecoderSession::is_complete).
    ///
    /// **This is the sole point that applies the implemented L2 rules** (§6.7 —
    /// N1/N2/N3/N6/T1; deferred rules pass through). Under an active [`Schema`] the
    /// fixed-PDA mask is intersected with that schema-derived set (§6), so a
    /// token rejected by an implemented rule is cleared here.
    /// [`accept_token`](DecoderSession::accept_token) does *not* re-apply that
    /// narrow — the mask-first contract is that the host samples only from this set,
    /// then commits. When no schema is set, this is the L1 mask alone.
    ///
    /// Cost is one word-wise copy of the state's cached context-independent mask
    /// plus a re-probe of the small deferred (stack-dependent) token set against
    /// the live stack — the per-step performance core (§4.3). It fills the
    /// grammar's lazy cache for the current state on first visit.
    ///
    /// Takes `&mut self` because it refills the session's reused mask buffer in
    /// place (and fills the lazy per-state cache); a safe `&self` returning
    /// `&BitMask` is impossible without handing out a reference into an owned
    /// buffer it must first mutate, and `unsafe` is forbidden (constitution §1).
    pub fn allowed_mask(&mut self) -> &BitMask {
        let cached = self.cursor.cached(self.grammar);
        self.mask.copy_from(&cached.indep);
        // Every deferred token is context-dependent *because* it needs an
        // enclosing frame (it died consulting the ambient stack during the
        // empty-scratch build). With an empty live stack there is nothing to
        // consult, so all of them stay dead — skip the whole re-probe.
        if self.cursor.stack_top_present() {
            for &id in &cached.deferred {
                let bytes = self.grammar.vocab().bytes(id).unwrap_or(&[]);
                if self.cursor.admits(bytes) {
                    self.mask.set(id);
                }
            }
        }
        let eos = self.grammar.eos_bit();
        if self.cursor.is_accepting() {
            self.mask.set(eos);
        } else {
            self.mask.clear(eos);
        }
        // L2 (§6): narrow the syntactic mask to the schema-legal set at exactly
        // this point. A pure `intersect` can only clear bits, so `L2 ⊆ L1` is
        // structural. The set is built into the reused `narrow_buf` (no per-step
        // alloc); when `schema` is `None` the block is skipped entirely, so the
        // L1-only path keeps its zero added per-step cost. `with_schema` already
        // refused a spec-compiled grammar, so `schema.is_some()` implies `Fixed` —
        // the `Cursor::Spec` arm below is unreachable in practice, not merely
        // unimplemented, so it is a no-op rather than a postponed-work marker.
        if let (Some(schema), Cursor::Fixed { pda, .. }) = (&self.schema, &self.cursor) {
            let pos = self.tracker.position(pda.state());
            if narrow_into(
                &mut self.narrow_buf,
                &mut self.narrow_cache,
                schema,
                &pos,
                self.tracker.narrow_prefix(),
                self.tracker.emitted_columns(),
                self.tracker.bound_variables(),
                self.grammar.vocab(),
                self.grammar.eos_bit(),
            ) {
                self.mask.intersect(&self.narrow_buf);
            }
            // S2's sigil half: before the stream has bound anything, a `$` opens a
            // variable reference no name can satisfy. Read here rather than at the
            // identifier the sigil opens, where the rule would clear every token
            // `AfterDollar` admits and deadlock the decoder (§6.5 S2, issue #275).
            if self.tracker.masks_unbound_sigil(pda.state())
                && narrow_sigil_into(
                    &mut self.narrow_buf,
                    &mut self.narrow_cache,
                    self.grammar.vocab(),
                    self.grammar.eos_bit(),
                )
            {
                self.mask.intersect(&self.narrow_buf);
            }
            // Byte-level BPE fuses the navigation `.` with the property/column's first
            // byte into one token (`.theme`, `.zzz`), which the anchor-read narrow
            // above cannot see (it sits before the dot). Apply a second, subtractive
            // narrow that clears exactly the `.`-led tokens whose post-dot identifier
            // begins no legal member/column — the fused-token gap (§6.5).
            if let Some(fused) = self.tracker.fused_nav_position(schema)
                && narrow_fused_into(
                    &mut self.narrow_buf,
                    &mut self.narrow_cache,
                    schema,
                    &fused,
                    self.tracker.emitted_columns(),
                    self.grammar.vocab(),
                    self.grammar.eos_bit(),
                )
            {
                self.mask.intersect(&self.narrow_buf);
            }
        }
        &self.mask
    }

    /// The [`L2Position`] [`allowed_mask`](Self::allowed_mask) would narrow
    /// against at the current step, or `None` when no schema is attached or no
    /// covered rule applies here — for coverage-testing purposes only (issue
    /// #59's per-named-rule/per-scope-transition coverage bullet).
    ///
    /// `#[doc(hidden)]`: test-support surface, not part of the crate's
    /// documented public contract (excluded from the `cargo public-api`
    /// snapshot). Recomputes the position read-only; it does not affect
    /// `allowed_mask`'s own hot path.
    #[doc(hidden)]
    pub fn active_l2_position(&self) -> Option<L2Position> {
        let (Some(_), Cursor::Fixed { pda, .. }) = (&self.schema, &self.cursor) else {
            return None;
        };
        match self.tracker.position(pda.state()) {
            L2Position::None => None,
            pos => Some(pos),
        }
    }

    /// Advance the session by one whole token, or reject it leaving the session
    /// **untouched** (§8.5 — the invariant that makes speculative masking sound).
    ///
    /// The reserved EOS id (one past the last vocab token) is accepted iff the
    /// stream is already complete; an unknown (out-of-range) id, or one whose
    /// bytes dead-end the recognizer, is rejected. The token is folded through a
    /// **clone** of the byte-PDA and the clone is committed only if every byte
    /// survives; a mid-token dead byte discards the clone, so the live automaton
    /// — its state *and* the full contents of its frame stack — is provably
    /// unchanged. (Restoring only a saved `(state, stack_len)` could not rebuild
    /// a frame an interior `Pop` had removed.) On acceptance it is byte-for-byte
    /// equivalent to folding [`accept_byte`](ByteRecognizer::accept_byte) over
    /// `vocab.bytes(id)`.
    ///
    /// **L1-only admission (mask-first contract).** This rejects a token only when
    /// its bytes dead-end the **byte-PDA** (the L1 grammar). It does *not* re-apply
    /// the L2 schema narrow: a token that is grammar-legal but schema-masked (absent
    /// from [`allowed_mask`](DecoderSession::allowed_mask) under an active [`Schema`])
    /// is still accepted. `allowed_mask` is the sole point that applies the implemented
    /// L2 rules (§6.7) — the host samples only from the masked set, then commits with
    /// `accept_token`. Do not treat `accept_token` as a schema-validation backstop;
    /// accepting an unmasked token yields schema-invalid output. (The L2 tracker
    /// still advances in lockstep so the *next* mask is correct.)
    ///
    /// # Errors
    /// Returns [`DecodeError::UnexpectedEos`] if EOS is signalled before the
    /// stream is complete, [`DecodeError::UnknownToken`] if `id` is out of range
    /// (a host-contract violation — no `Vocab` entry), or
    /// [`DecodeError::InadmissibleToken`] if an in-range token's bytes dead-end
    /// the recognizer (a **grammar**-respecting reject; the schema narrow is not
    /// re-checked here — see the mask-first note above).
    pub fn accept_token(&mut self, id: u32) -> Result<(), DecodeError> {
        if id == self.grammar.eos_bit() {
            return if self.cursor.is_accepting() {
                Ok(())
            } else {
                Err(DecodeError::UnexpectedEos)
            };
        }
        // An id with no `Vocab` entry (out of range) is a host-contract violation,
        // reported distinctly from an in-range token the mask legitimately clears.
        let Some(bytes) = self.grammar.vocab().bytes(id) else {
            return Err(DecodeError::UnknownToken { id });
        };
        // `with_schema` already refused a spec-compiled grammar, so
        // `schema.is_some()` implies `Cursor::Fixed`; the fall-through `else`
        // below is unreachable in practice for that case, not merely
        // unimplemented, so a schema-tagged `Cursor::Spec` (impossible today)
        // degrades to the plain L1 path rather than panicking.
        if let (Some(schema), Cursor::Fixed { pda, .. }) = (&self.schema, &mut self.cursor) {
            // Fold into a clone and commit only on full success: a rejection never
            // touches `pda`, so no stack contents can be corrupted by a
            // Pop-then-fail. One small stack clone per call, off the per-candidate
            // mask hot path.
            let mut probe = pda.clone();
            for &byte in bytes {
                if probe.advance(byte).is_err() {
                    return Err(DecodeError::InadmissibleToken { id });
                }
            }
            // Advance the L2 scope machine in lockstep, so the next `allowed_mask`
            // narrows against the scope this token established. The tracker
            // re-drives the byte-PDA over the token from its **pre-fold**
            // configuration (state *and* stack, still live in `pda` here),
            // splitting a lexeme-straddling token at its interior boundaries.
            self.tracker.observe(bytes, pda, schema);
            *pda = probe;
            self.refresh_l2_eos();
        } else if !self.cursor.try_accept_token(bytes) {
            return Err(DecodeError::InadmissibleToken { id });
        }
        self.offset += bytes.len();
        Ok(())
    }

    /// Re-read the L2 overlay's end-of-stream verdict for the position the last
    /// accepted token left the session in, caching it in `l2_eos`.
    ///
    /// Called only from the schema-active token path, right after the tracker
    /// advances, so the flag and the tracker never disagree. It walks the rule's
    /// trie over the open lexeme's prefix — no vocabulary scan — and shares the
    /// memoized trie with [`allowed_mask`](Self::allowed_mask)'s own narrow.
    fn refresh_l2_eos(&mut self) {
        let (Some(schema), Cursor::Fixed { pda, .. }) = (&self.schema, &self.cursor) else {
            return;
        };
        let pos = self.tracker.position(pda.state());
        self.l2_eos = admits_eos(
            &mut self.narrow_cache,
            schema,
            &pos,
            self.tracker.narrow_prefix(),
            self.tracker.emitted_columns(),
            self.tracker.bound_variables(),
        );
    }

    /// The underlying byte-PDA at its full `(state, stack)` configuration, or
    /// `None` if this session was built from a spec-compiled grammar
    /// ([`CompiledGrammar::from_spec`](crate::grammar::compiled::CompiledGrammar::from_spec)).
    ///
    /// Exposed so a caller — or a test — can compare two sessions for a
    /// byte-identical automaton configuration, which the derived
    /// [`allowed_mask`](DecoderSession::allowed_mask) view cannot prove on its
    /// own: two different `(state, stack)` configurations can share a mask.
    #[must_use]
    pub fn pda(&self) -> Option<&Pda> {
        match &self.cursor {
            Cursor::Fixed { pda, .. } => Some(pda),
            Cursor::Spec { .. } => None,
        }
    }

    /// Whether the stream so far is a complete query: an accepting L1
    /// configuration **that the L2 overlay also permits ending on**.
    ///
    /// Re-exposed inherently so callers need not import [`ByteRecognizer`]; it
    /// mirrors [`ByteRecognizer::is_complete`].
    ///
    /// **Mask-aware (§6.5).** L1 acceptance is a lookahead fact — "would a
    /// value-boundary byte from here reach a value-terminal state?" — and an
    /// identifier has no self-terminating byte, so every partial name satisfies
    /// it. Under an active [`Schema`] this therefore also asks the overlay
    /// whether the position is one a query may *end* in, which is exactly the
    /// EOS bit [`allowed_mask`](Self::allowed_mask) publishes: the two now agree
    /// by construction, where before a stream could stop mid-identifier
    /// (`Class.a`) or on a bare niladic method name (`Class.all`) and call
    /// itself complete while the mask said otherwise. With no schema — and on
    /// the byte-wise [`ByteRecognizer`] path, which never advances the scope
    /// machine — this is L1 acceptance alone, unchanged.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.cursor.is_accepting() && self.l2_eos
    }

    /// Return to a fresh stream, keeping the automaton's stack and the mask
    /// buffer allocated for reuse (§9.1). Mirrors [`ByteRecognizer::reset`].
    pub fn reset(&mut self) {
        self.cursor.reset();
        self.offset = 0;
        self.tracker = ScopeTracker::new();
        // The N6 column memo is keyed on the emitted-column count, which resets
        // with the stream — a stale entry could otherwise be re-hit at a repeated
        // count over a different column set.
        self.narrow_cache.clear();
        self.l2_eos = true;
    }
}

impl ByteRecognizer for DecoderSession<'_> {
    fn accept_byte(&mut self, byte: u8) -> Result<(), DecodeError> {
        let offset = self.offset;
        match self.cursor.advance_byte(byte) {
            Ok(()) => {
                self.offset += 1;
                Ok(())
            }
            Err((state, stack_top)) => Err(DecodeError::DeadState {
                offset,
                byte,
                state,
                stack_top,
            }),
        }
    }

    fn is_complete(&self) -> bool {
        DecoderSession::is_complete(self)
    }

    fn reset(&mut self) {
        DecoderSession::reset(self);
    }
}

#[cfg(test)]
mod tests {
    use super::{Cursor, DecoderSession};
    use crate::error::DecodeError;
    use crate::grammar::compiled::CompiledGrammar;
    use crate::recognizer::ByteRecognizer;
    use crate::schema::Schema;
    use crate::vocab::Vocab;

    /// An L1-only grammar over an empty vocabulary — enough to drive the
    /// byte-recognizer surface, which does not consult the vocab.
    fn l1_grammar() -> CompiledGrammar {
        CompiledGrammar::compile(Vocab::from_byte_tokens(Vec::new()))
    }

    #[test]
    fn cursor_stack_top_present_tracks_the_live_stack_not_a_constant() {
        let grammar = l1_grammar();
        let mut cursor = Cursor::new(&grammar);
        assert!(
            !cursor.stack_top_present(),
            "a fresh cursor has an empty stack"
        );
        // `(` opens a `Paren` frame from `ExpectValue` after a source.
        for &byte in b"|X.all()->take(" {
            assert!(matches!(cursor.advance_byte(byte), Ok(())));
        }
        assert!(
            cursor.stack_top_present(),
            "a pushed frame must be reported present"
        );
    }

    /// A byte-token grammar: one single-byte token per value, so token id == byte.
    /// The EOS bit is one past the last byte. Lets a test drive the token surface
    /// (`accept_token`/`allowed_mask`) byte-by-byte under a schema.
    fn byte_grammar() -> CompiledGrammar {
        let tokens: Vec<Vec<u8>> = (0..=u8::MAX).map(|b| vec![b]).collect();
        CompiledGrammar::compile(Vocab::from_byte_tokens(tokens))
    }

    #[test]
    fn accept_token_enforces_only_l1_not_the_l2_schema_mask() {
        // Mask-first contract (GAP 2): with a schema active, `allowed_mask` is the
        // sole L2 enforcement point; `accept_token` checks only the grammar. Drive to
        // `$x.` on a class with member `n` (no member `z`), then at the member
        // position: `z` is schema-masked (not a member) yet grammar-legal, so
        // `accept_token('z')` still succeeds — proving accept is not a schema backstop.
        const SCHEMA: &str = r#"{"db_id":"d","db_path":"demo::Db","classes":{
            "demo::Reading":{"simple_name":"Reading","properties":[
              {"name":"n","type":{"kind":"primitive","name":"Integer"},"mult":{"lower":1,"upper":1}}]}},
            "associations":[]}"#;
        let grammar = byte_grammar();
        let schema = Schema::from_json(SCHEMA).expect("schema parses");
        let mut session =
            DecoderSession::with_schema(&grammar, schema).expect("grammar is fixed-engine");
        for &byte in b"|demo::Reading.all()->filter(x|$x." {
            session
                .accept_token(u32::from(byte))
                .expect("prefix byte is grammar- and schema-legal");
        }
        let z = u32::from(b'z');
        // `z` is grammar-legal (an identifier byte) but schema-masked: the L2 narrow
        // clears it because the class has no member starting `z`.
        assert!(
            session.allowed_mask().test(u32::from(b'n')),
            "the real member byte `n` is admissible",
        );
        assert!(
            !session.allowed_mask().test(z),
            "`z` is schema-masked at the member position (no member `z`)",
        );
        // Yet `accept_token` admits it — L1-only, mask-first contract.
        assert!(
            session.accept_token(z).is_ok(),
            "accept_token enforces only the grammar, not the schema mask",
        );
    }

    fn drive(text: &str) -> (Result<(), DecodeError>, bool, usize) {
        let grammar = l1_grammar();
        let mut session = DecoderSession::new(&grammar);
        let mut result = Ok(());
        for &byte in text.as_bytes() {
            if let Err(err) = session.accept_byte(byte) {
                result = Err(err);
                break;
            }
        }
        (result, session.is_complete(), session.offset())
    }

    #[test]
    fn a_complete_query_streams_and_is_complete() {
        let (result, complete, offset) = drive("|X.all()->take(3)");
        assert!(result.is_ok());
        assert!(complete);
        assert_eq!(offset, "|X.all()->take(3)".len());
    }

    #[test]
    fn a_partial_query_is_not_complete() {
        let (result, complete, _) = drive("|X.all()->take(3");
        assert!(result.is_ok());
        assert!(!complete);
    }

    #[test]
    fn a_dead_byte_reports_offset_and_state() {
        let grammar = l1_grammar();
        let mut session = DecoderSession::new(&grammar);
        let mut err = None;
        for &byte in "|X.all())".as_bytes() {
            if let Err(e) = session.accept_byte(byte) {
                err = Some(e);
                break;
            }
        }
        let DecodeError::DeadState {
            offset,
            byte,
            stack_top,
            ..
        } = err.expect("extra ')' must dead-end")
        else {
            panic!("expected a dead state");
        };
        assert_eq!(byte, b')');
        assert_eq!(offset, "|X.all()".len());
        assert_eq!(stack_top, "none");
    }

    #[test]
    fn reset_rewinds_offset_and_state() {
        let grammar = l1_grammar();
        let mut session = DecoderSession::new(&grammar);
        for &byte in b"|X.all()" {
            session.accept_byte(byte).expect("live");
        }
        session.reset();
        assert_eq!(session.offset(), 0);
        assert!(!session.is_complete());
        assert!(session.accept_byte(b'x').is_err());
    }

    /// A vocabulary of whole tokens for the token-level surface: an opener/source
    /// prefix, a step, closers, and the empty token.
    fn token_vocab() -> Vocab {
        Vocab::from_byte_tokens(vec![
            b"|X.all()".to_vec(), // 0: a complete source expression
            b"->take(".to_vec(),  // 1: a step opening a Paren
            b"1".to_vec(),        // 2: a digit
            b")".to_vec(),        // 3: a closer
            b"".to_vec(),         // 4: the empty token
        ])
    }

    #[test]
    fn pda_exposes_the_live_automaton_configuration() {
        use crate::grammar::pda::{Frame, State};
        let grammar = CompiledGrammar::compile(token_vocab());
        let mut session = DecoderSession::new(&grammar);
        // A fresh session sits at the initial configuration…
        assert_eq!(
            session.pda().expect("fixed-engine grammar").state(),
            State::Start
        );
        assert_eq!(
            session.pda().expect("fixed-engine grammar").stack_top(),
            None
        );
        // …and after opening a call the accessor reflects the *real* live state
        // and stack, so it cannot be a constant / default value.
        session.accept_token(0).expect("source is admissible");
        session
            .accept_token(1)
            .expect("a step opener is admissible");
        assert_eq!(
            session.pda().expect("fixed-engine grammar").state(),
            State::ExpectValue
        );
        assert_eq!(
            session.pda().expect("fixed-engine grammar").stack_top(),
            Some(Frame::Paren)
        );
    }

    #[test]
    fn accept_token_streams_a_query_token_by_token() {
        let grammar = CompiledGrammar::compile(token_vocab());
        let mut session = DecoderSession::new(&grammar);
        for id in [0u32, 1, 2, 3] {
            session.accept_token(id).expect("admissible token");
        }
        assert!(session.is_complete());
        assert_eq!(session.offset(), "|X.all()->take(1)".len());
    }

    #[test]
    fn an_inadmissible_token_is_rejected_and_leaves_the_session_untouched() {
        let grammar = CompiledGrammar::compile(token_vocab());
        let mut session = DecoderSession::new(&grammar);
        session.accept_token(0).expect("source is admissible");
        // `|X.all()` is itself a complete query (AfterValue, empty stack).
        assert!(session.is_complete());
        let before_offset = session.offset();
        // A lone closer `)` cannot follow a completed value with an empty stack.
        let err = session
            .accept_token(3)
            .expect_err("closer must be rejected");
        assert!(matches!(err, DecodeError::InadmissibleToken { id: 3 }));
        // The rejected token left the session byte-identical: same offset, and
        // still complete.
        assert_eq!(session.offset(), before_offset);
        assert_eq!(session.offset(), "|X.all()".len());
        assert!(session.is_complete());
    }

    #[test]
    fn an_out_of_range_token_id_is_unknown_not_inadmissible() {
        // An id with no `Vocab` entry is a host-contract violation — the distinct
        // `UnknownToken`, not the mask-respecting `InadmissibleToken` an in-range
        // dead-ending token raises.
        let grammar = CompiledGrammar::compile(token_vocab());
        let mut session = DecoderSession::new(&grammar);
        let err = session.accept_token(999).expect_err("no such token");
        assert!(matches!(err, DecodeError::UnknownToken { id: 999 }));
        // The reserved EOS id (== vocab.len()) is the boundary: one past it is the
        // first unknown id.
        let first_unknown = grammar.eos_bit() + 1;
        assert!(matches!(
            session.accept_token(first_unknown),
            Err(DecodeError::UnknownToken { id }) if id == first_unknown
        ));
        // An in-range closer that dead-ends stays `InadmissibleToken`.
        assert!(matches!(
            session.accept_token(3),
            Err(DecodeError::InadmissibleToken { id: 3 })
        ));
    }

    #[test]
    fn eos_is_accepted_only_when_the_stream_is_complete() {
        let grammar = CompiledGrammar::compile(token_vocab());
        let eos = grammar.eos_bit();
        let mut session = DecoderSession::new(&grammar);
        // Premature EOS on an empty stream is rejected.
        assert!(matches!(
            session.accept_token(eos),
            Err(DecodeError::UnexpectedEos)
        ));
        for id in [0u32, 1, 2, 3] {
            session.accept_token(id).expect("admissible");
        }
        // Now the query is complete, EOS is legal.
        assert!(session.accept_token(eos).is_ok());
    }

    #[test]
    fn the_recognizer_trait_reports_completeness() {
        let grammar = CompiledGrammar::compile(token_vocab());
        let mut session = DecoderSession::new(&grammar);
        session.accept_token(0).expect("source is admissible");
        session
            .accept_token(1)
            .expect("a step opener is admissible");
        // Inside the still-open `->take(` call: not an accepting configuration,
        // read through the trait method (not the inherent one).
        assert!(!ByteRecognizer::is_complete(&session));
        session.reset();
        for id in [0u32, 1, 2, 3] {
            session.accept_token(id).expect("admissible");
        }
        // A closed, completed query: accepting, read through the trait.
        assert!(ByteRecognizer::is_complete(&session));
    }

    #[test]
    fn the_recognizer_trait_reset_restores_the_initial_configuration() {
        let grammar = CompiledGrammar::compile(token_vocab());
        let fresh = DecoderSession::new(&grammar);
        let mut session = DecoderSession::new(&grammar);
        for id in [0u32, 1, 2] {
            session.accept_token(id).expect("admissible");
        }
        // Reset through the trait must rewind the *full* configuration: offset,
        // automaton state, and the entire frame stack — not merely the offset.
        ByteRecognizer::reset(&mut session);
        assert_eq!(session.offset(), 0);
        assert_eq!(session.pda(), fresh.pda());
        // …and the per-step mask must equal a never-driven session's, bit for bit.
        let mut untouched = DecoderSession::new(&grammar);
        assert_eq!(session.allowed_mask(), untouched.allowed_mask());
    }

    #[test]
    fn allowed_mask_sets_the_eos_bit_iff_complete() {
        let grammar = CompiledGrammar::compile(token_vocab());
        let eos = grammar.eos_bit();
        let mut session = DecoderSession::new(&grammar);
        assert!(!session.allowed_mask().test(eos), "start is not complete");
        for id in [0u32, 1, 2, 3] {
            session.accept_token(id).expect("admissible");
        }
        assert!(
            session.allowed_mask().test(eos),
            "completed stream allows EOS"
        );
    }

    /// Accepts exactly the literal `"ok"` — used to drive a spec-compiled
    /// (`Cursor::Spec`) session, mirroring `grammar::compile::tests`'
    /// `LITERAL_OK_SPEC` but kept local so this module doesn't depend on a
    /// `#[cfg(test)]`-only item from another module.
    const LITERAL_OK_SPEC: &str = r#"{
        "version": "1",
        "start": "start",
        "frames": [],
        "states": {
            "start": { "rules": [
                { "match": { "kind": "exact", "byte": 111 }, "action": { "kind": "next", "state": "saw_o" } }
            ] },
            "saw_o": { "rules": [
                { "match": { "kind": "exact", "byte": 107 }, "action": { "kind": "next", "state": "done" } }
            ] },
            "done": { "accepting": true, "rules": [] }
        }
    }"#;

    fn spec_vocab() -> Vocab {
        // 0: the whole valid literal; 1: a token that dies on its second byte
        // (alive on `o`, dead on `x` — never reaching `saw_o`'s `k` rule).
        Vocab::from_byte_tokens(vec![b"ok".to_vec(), b"ox".to_vec()])
    }

    #[test]
    fn a_spec_compiled_session_streams_its_literal_and_completes() {
        let grammar =
            crate::grammar::compiled::CompiledGrammar::from_spec(LITERAL_OK_SPEC, spec_vocab())
                .expect("valid spec");
        let mut session = DecoderSession::new(&grammar);
        session.accept_token(0).expect("the literal is admissible");
        assert!(session.is_complete());
    }

    #[test]
    fn a_spec_compiled_session_rejects_a_token_that_dies_partway_through() {
        let grammar =
            crate::grammar::compiled::CompiledGrammar::from_spec(LITERAL_OK_SPEC, spec_vocab())
                .expect("valid spec");
        let mut session = DecoderSession::new(&grammar);
        let err = session
            .accept_token(1)
            .expect_err("the second byte dead-ends the spec-compiled automaton");
        assert!(matches!(err, DecodeError::InadmissibleToken { id: 1 }));
        // The rejected token must leave the session untouched (§8.5 rollback),
        // exactly like the fixed-engine contract.
        assert_eq!(session.offset(), 0);
        assert!(!session.is_complete());
    }
}
