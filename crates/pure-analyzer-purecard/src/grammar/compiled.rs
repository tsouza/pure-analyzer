//! The compiled grammar: a [`Vocab`] plus the lazy per-state token-mask cache
//! that makes the per-step allowed set cheap (§4).
//!
//! A naive allowed-mask recomputes, at every decode step, which of ~150k vocab
//! tokens keep the byte-PDA alive — millions of [`step`](super::pda::step) calls,
//! far over the few-hundred-µs budget. [`CompiledGrammar`] follows the
//! xgrammar-style split (§4.2): at each reachable [`State`] it partitions the
//! vocabulary into
//!
//! - **context-independent survivors** — tokens whose admissibility depends only
//!   on the state, cached once as a [`BitMask`] (`indep`); and
//! - **context-dependent** tokens — bare closers `)]}`  and `,`/`;`/`*` whose
//!   admissibility depends on the live stack, kept as a small `deferred` id list
//!   the session re-probes per step against the real stack.
//!
//! The cache is **lazy** (§4.5): a [`State`]'s partition is built on first visit
//! and only for states a decode actually reaches. It is interior-mutable
//! ([`OnceCell`]) so a shared `&CompiledGrammar` can fill it as
//! [`DecoderSession`](crate::DecoderSession) drives.

use std::cell::OnceCell;

use crate::grammar::compile::{CompiledAutomaton, RtnPda};
use crate::grammar::pda::{Pda, State};
use crate::grammar::spec::{GrammarSpec, SpecError};
use crate::mask::BitMask;
use crate::vocab::Vocab;

/// Which automaton a [`CompiledGrammar`] wraps: the hand-written, fixed §5 PDA,
/// or a [`CompiledAutomaton`] lowered from a supplied [`GrammarSpec`].
///
/// The L2 schema overlay (`schema::scope::ScopeTracker`) is implemented
/// against the fixed PDA's named [`State`] positions and is not available for
/// a [`Spec`](Engine::Spec)-backed grammar — a supplied grammar gets L1
/// syntactic recognition only (see
/// `docs/decisions/0010-declarative-transition-table-spec.md`, Consequences).
#[derive(Debug)]
pub(crate) enum Engine {
    /// The hand-written `grammar::pda` automaton.
    Fixed,
    /// An automaton lowered from a supplied [`GrammarSpec`].
    Spec(CompiledAutomaton),
}

/// A grammar compiled against a specific model vocabulary: the vocab itself plus
/// the lazy per-state mask cache (§4).
///
/// Wraps either the fixed hand-written byte-PDA ([`compile`](CompiledGrammar::compile))
/// or an automaton lowered from a supplied grammar spec
/// ([`from_spec`](CompiledGrammar::from_spec)). Build one per `(model, grammar)`
/// pair and share it across sessions.
#[derive(Debug)]
pub struct CompiledGrammar {
    vocab: Vocab,
    pub(crate) engine: Engine,
    /// One lazily-filled partition per automaton state, indexed by
    /// [`State::index`] for [`Engine::Fixed`] or the dense automaton state id
    /// for [`Engine::Spec`]. `OnceCell` gives the interior mutability that lets
    /// a shared `&self` fill a state's entry on first visit.
    cache: Vec<OnceCell<Cached>>,
}

/// A single state's memoized vocabulary partition (§4.2).
#[derive(Debug)]
pub(crate) struct Cached {
    /// The context-independent survivors at this state — admissible regardless of
    /// the stack, so cacheable and copied wholesale into the per-step mask.
    pub(crate) indep: BitMask,
    /// Token ids whose admissibility at this state depends on the live stack;
    /// the session re-probes each against the real stack per step. `|deferred|`
    /// is tiny next to the vocabulary.
    pub(crate) deferred: Box<[u32]>,
}

impl CompiledGrammar {
    /// Compile the fixed byte-PDA against `vocab`, sizing the (empty) lazy
    /// per-state cache. No token is probed here — every state's partition is
    /// built on first visit (§4.5).
    #[must_use]
    pub fn compile(vocab: Vocab) -> Self {
        let cache = (0..State::COUNT).map(|_| OnceCell::new()).collect();
        Self {
            vocab,
            engine: Engine::Fixed,
            cache,
        }
    }

    /// Compile a grammar for `vocab` by lowering `spec` (JSON, see
    /// [`GrammarSpec`]) into a runtime automaton.
    ///
    /// The compiled grammar supports L1 syntactic recognition
    /// ([`DecoderSession::new`](crate::DecoderSession::new)); the L2 schema
    /// overlay ([`DecoderSession::with_schema`](crate::DecoderSession::with_schema))
    /// remains available only for a grammar built with
    /// [`compile`](CompiledGrammar::compile) — see
    /// `docs/decisions/0010-declarative-transition-table-spec.md`.
    ///
    /// # Errors
    /// Returns [`SpecError`] if `spec` is not valid JSON, does not match the
    /// versioned schema, or fails validation (unknown state/frame reference,
    /// ambiguous or unreachable rule, unguarded pop, a `Goto` cycle, no
    /// reachable accepting state, or an explosive state/rule count). No
    /// [`DecoderSession`](crate::DecoderSession) can be built from a spec this
    /// rejects.
    pub fn from_spec(spec: &str, vocab: Vocab) -> Result<Self, SpecError> {
        let parsed = GrammarSpec::parse(spec)?;
        let automaton = CompiledAutomaton::compile(&parsed)?;
        let cache = (0..automaton.state_count())
            .map(|_| OnceCell::new())
            .collect();
        Ok(Self {
            vocab,
            engine: Engine::Spec(automaton),
            cache,
        })
    }

    /// The vocabulary this grammar was compiled against.
    #[must_use]
    pub fn vocab(&self) -> &Vocab {
        &self.vocab
    }

    /// Which automaton this grammar wraps.
    pub(crate) fn engine(&self) -> &Engine {
        &self.engine
    }

    /// The reserved EOS bit position: the id one past the last real token
    /// (Decision D3). The per-step mask spans `vocab.len() + 1` bits.
    pub(crate) fn eos_bit(&self) -> u32 {
        self.vocab.len() as u32
    }

    /// The EOS-inclusive mask length (`eos_bit() + 1`): the **single** source of
    /// the `V + 1` sizing every per-step mask and cached partition shares, so the
    /// session buffer and a state's `indep` mask can never disagree on length and
    /// trip [`BitMask::copy_from`](crate::mask::BitMask::copy_from) (constitution
    /// §4, DRY).
    pub(crate) fn mask_len(&self) -> usize {
        self.eos_bit() as usize + 1
    }

    /// The memoized partition for `state`, built on first access (§4.5).
    ///
    /// Only ever called by a `Fixed`-backed session's cursor — the fixed
    /// [`State`] alphabet and this grammar's cache sizing (`State::COUNT`)
    /// only correspond to each other for [`Engine::Fixed`].
    pub(crate) fn cached(&self, state: State) -> &Cached {
        self.cache[state.index()].get_or_init(|| build_fixed(state, &self.vocab, self.mask_len()))
    }

    /// The memoized partition for `automaton`'s state `id`, built on first
    /// access (§4.5). `automaton` is passed in (rather than re-derived from
    /// `self.engine`) so a `Spec`-backed session's cursor — which already
    /// owns the automaton via its [`RtnPda`] — is the single source of truth
    /// for which automaton it is driving.
    pub(crate) fn cached_spec(&self, automaton: &CompiledAutomaton, id: u32) -> &Cached {
        self.cache[id as usize]
            .get_or_init(|| build_spec(automaton, id, &self.vocab, self.mask_len()))
    }
}

/// Build a state's vocabulary partition by probing every token from the state
/// over an **empty** stack (§4.2). A token that stays alive is a
/// context-independent survivor; one that dies consulting the ambient stack is
/// deferred; one that dies outright is admissible from no stack and is dropped.
fn build_fixed(state: State, vocab: &Vocab, mask_len: usize) -> Cached {
    let base = Pda::at(state);
    let mut indep = BitMask::with_len(mask_len);
    let mut deferred = Vec::new();
    let mut scratch = Vec::new();
    for id in 0..vocab.len() as u32 {
        let bytes = vocab.bytes(id).unwrap_or(&[]);
        let probe = base.probe(bytes, &mut scratch);
        if probe.consulted_ambient {
            deferred.push(id);
        } else if probe.alive {
            indep.set(id);
        }
    }
    Cached {
        indep,
        deferred: deferred.into_boxed_slice(),
    }
}

/// The [`Engine::Spec`] analogue of [`build_fixed`], probing via
/// [`RtnPda::at`]/[`RtnPda::probe`] instead of the fixed PDA.
fn build_spec(automaton: &CompiledAutomaton, id: u32, vocab: &Vocab, mask_len: usize) -> Cached {
    let base = RtnPda::at(automaton, id);
    let mut indep = BitMask::with_len(mask_len);
    let mut deferred = Vec::new();
    let mut scratch = Vec::new();
    for token_id in 0..vocab.len() as u32 {
        let bytes = vocab.bytes(token_id).unwrap_or(&[]);
        let probe = base.probe(bytes, &mut scratch);
        if probe.consulted_ambient {
            deferred.push(token_id);
        } else if probe.alive {
            indep.set(token_id);
        }
    }
    Cached {
        indep,
        deferred: deferred.into_boxed_slice(),
    }
}

#[cfg(test)]
mod tests {
    use super::{CompiledGrammar, Engine};
    use crate::grammar::pda::State;
    use crate::vocab::Vocab;

    fn vocab() -> Vocab {
        // A closer (context-dependent), an identifier (independent survivor from
        // a value position), and a bare `,` (dead in a value position — a
        // separator is not a value — regardless of any enclosing frame).
        Vocab::from_byte_tokens(vec![
            b")".to_vec(),
            b"name".to_vec(),
            b",".to_vec(),
            b"".to_vec(),
        ])
    }

    #[test]
    fn compile_sizes_a_lazy_cache_per_state() {
        let grammar = CompiledGrammar::compile(vocab());
        assert_eq!(grammar.vocab().len(), 4);
        assert_eq!(grammar.eos_bit(), 4);
    }

    #[test]
    fn mask_len_is_one_past_the_last_token() {
        // The single EOS-inclusive length: `V + 1`, equivalently `eos_bit + 1`.
        // Pins the `+ 1` so an arithmetic slip (a `-`/`*`) reddens the gate.
        let grammar = CompiledGrammar::compile(vocab());
        assert_eq!(grammar.mask_len(), grammar.vocab().len() + 1);
        assert_eq!(grammar.mask_len(), grammar.eos_bit() as usize + 1);
        // A state's cached survivor mask is sized by exactly this derivation.
        let cached = grammar.cached(State::ExpectValue);
        assert_eq!(cached.indep.len(), grammar.mask_len());
    }

    /// Accepts exactly the literal "ok".
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

    #[test]
    fn from_spec_rejects_malformed_json_before_compiling_anything() {
        let error = CompiledGrammar::from_spec("not json", vocab())
            .expect_err("malformed spec must not silently fall back to a fixed grammar");
        assert!(matches!(
            error,
            crate::grammar::spec::SpecError::Malformed { .. }
        ));
    }

    #[test]
    fn from_spec_lowers_a_real_spec_into_a_working_cache() {
        let grammar =
            CompiledGrammar::from_spec(LITERAL_OK_SPEC, vocab()).expect("valid spec compiles");
        let Engine::Spec(automaton) = grammar.engine() else {
            panic!("from_spec always builds a Spec-backed grammar")
        };
        // `id`s are dense automaton state ids here, not `pda::State` — probe
        // the start state (id 0, the first key in the spec's sorted map).
        let cached = grammar.cached_spec(automaton, 0);
        assert_eq!(cached.indep.len(), grammar.mask_len());
    }

    #[test]
    fn build_partitions_closers_survivors_and_dead_tokens() {
        let grammar = CompiledGrammar::compile(vocab());
        let cached = grammar.cached(State::ExpectValue);
        // `name` (id 1) and the empty token (id 3) survive independently…
        assert!(cached.indep.test(1));
        assert!(cached.indep.test(3));
        // …a bare `,` (id 2) is dead in a value position and is dropped…
        assert!(!cached.indep.test(2));
        assert!(!cached.deferred.contains(&2));
        // …and the closer `)` (id 0) is deferred to the live-stack re-probe.
        assert!(!cached.indep.test(0));
        assert!(cached.deferred.contains(&0));
    }

    #[test]
    fn cached_is_memoized_across_visits() {
        let grammar = CompiledGrammar::compile(vocab());
        let first = grammar.cached(State::AfterValue) as *const _;
        let second = grammar.cached(State::AfterValue) as *const _;
        assert_eq!(
            first, second,
            "a state's partition is built once and reused"
        );
    }
}
