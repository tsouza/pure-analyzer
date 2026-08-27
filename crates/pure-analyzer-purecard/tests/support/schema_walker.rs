//! Seeded, schema-aware accepting-walk generator (issue #59).
//!
//! Sibling to [`walker`](super::walker), not a rewrite of it: `walker`'s
//! clone-and-probe operates on the raw byte [`Pda`](purecard::Pda) and is the
//! seed corpus for the L1-only lanes. This generator instead drives a real
//! [`DecoderSession::with_schema`], choosing among **vocabulary token ids**
//! via `allowed_mask()`/`accept_token()` at every step — so the schema's L2
//! scope machine and mask constrain the walk exactly the way a host's decode
//! loop would, not just L1 byte admissibility.
//!
//! Same overall shape as `walker`'s algorithm: grow to a per-walk length
//! target, then bias hard toward whichever candidate looks likely to finish
//! the walk, so every attempt that doesn't die converges to an accepting
//! walk in bounded steps. Unlike `walker`, candidates here are read directly
//! from `allowed_mask()` rather than clone-and-probed: the admissibility
//! `walker` needs a probe to establish is already guaranteed by the
//! mask/accept invariant (`mask_properties.rs`), and the completion bias
//! uses a cheap byte-content heuristic ([`ends_with_closer`]) instead of a
//! simulated look-ahead, since a vocabulary here can hold hundreds of
//! lexemes versus `walker`'s 31-byte alphabet — see `attempt`'s docs. The
//! SplitMix64 PRNG is a second copy of `walker`'s (not shared): the two
//! generators pick among different candidate spaces with different
//! weighting rules, so factoring out just the RNG would couple two
//! otherwise-independent modules for a ~20-line, fully-specified algorithm
//! neither module needs from the other.

use purecard::{CompiledGrammar, DecoderSession, Schema, Vocab};

/// Number of accepting walks a full generation produces per schema.
pub const WALK_COUNT: usize = 64;

/// Upper bound on generation attempts — a safety valve so a bug can never spin
/// forever. Comfortably above [`WALK_COUNT`], since the biased close-out lands
/// an accepting walk on nearly every seed.
const ATTEMPT_LIMIT: usize = WALK_COUNT * 64;

/// The base seed; walk `i` derives from `BASE_SEED` advanced past every seed a
/// prior walk consumed, so the set is one deterministic stream, not
/// [`WALK_COUNT`] correlated low seeds. Distinct from `walker`'s base seed so
/// the two generators' streams never accidentally coincide.
///
/// `dead_code`-allowed: `live_legend_schema_walk_compile.rs`'s compilation
/// unit includes this module via `#[path]` but only calls the eager
/// [`generate_first_complete_schema_walks`], not [`generate_schema_walks`].
#[allow(dead_code)]
const BASE_SEED: u64 = 0x5363_6865_6d61_5761; // "SchemaWa" as ASCII bytes.

/// Shortest accepted walk kept, in tokens.
const MIN_LEN: usize = 1;

/// The per-walk growth target, in tokens, is drawn from `[GROW_MIN, GROW_MAX)`;
/// until reached, every admissible candidate is eligible. Past it, the walk
/// closes toward completion. Tokens are whole lexemes (often several bytes),
/// so this target is smaller than `walker`'s byte-count target for a
/// comparably shaped walk.
const GROW_MIN: u64 = 2;
const GROW_MAX: u64 = 12;

/// Hard cap on emitted tokens per attempt — a safety bound so a pathological
/// walk terminates rather than spins.
const HARD_CAP: usize = 64;

/// Weight added, in the closing phase, to a candidate whose result is a
/// completed session — biases each closing step toward finishing the walk.
const ACCEPT_BONUS: u32 = 10;

/// Uniform per-candidate weight outside the accept bonus.
const DEFAULT_WEIGHT: u32 = 1;

/// SplitMix64 — see [`walker`](super::walker)'s copy for the algorithm's
/// provenance and citation; duplicated here rather than shared (see module
/// docs).
struct SplitMix64 {
    state: u64,
}

const SPLITMIX_GAMMA: u64 = 0x9E37_79B9_7F4A_7C15;
const SPLITMIX_MIX_A: u64 = 0xBF58_476D_1CE4_E5B9;
const SPLITMIX_MIX_B: u64 = 0x94D0_49BB_1331_11EB;

impl SplitMix64 {
    fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(SPLITMIX_GAMMA);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(SPLITMIX_MIX_A);
        z = (z ^ (z >> 27)).wrapping_mul(SPLITMIX_MIX_B);
        z ^ (z >> 31)
    }

    /// A uniform `u64` in `[0, bound)`; `bound` is always a small positive
    /// candidate-set size here, so a modulo bias is negligible.
    fn below(&mut self, bound: u64) -> u64 {
        self.next_u64() % bound
    }
}

/// Pick an admissible token id by weight. `cands` is non-empty with at least
/// one positive weight whenever this is called.
fn weighted_pick(cands: &[(u32, u32)], rng: &mut SplitMix64) -> u32 {
    let total: u32 = cands.iter().map(|&(_, w)| w).sum();
    let mut target = rng.below(u64::from(total)) as u32;
    for &(id, w) in cands {
        if target < w {
            return id;
        }
        target -= w;
    }
    // Unreachable: weights sum to `total` and `target < total`. Fall back to
    // the last candidate rather than panic.
    cands[cands.len() - 1].0
}

/// Bytes that plausibly end a construct (mirroring
/// [`walker`](super::walker)'s `is_closer`, extended to the punctuation that
/// can end a *statement*, not only a bracket) — a token ending in one of
/// these is weighted toward finishing the walk. This is a cheap heuristic on
/// the token's own bytes, not a simulated look-ahead: unlike `walker`'s tiny
/// byte alphabet (31 candidates, cheap to clone-and-probe each one), a
/// vocabulary here can hold hundreds of lexemes, and probing every candidate
/// by cloning the whole schema-aware session at every step was the dominant
/// cost in this generator — correctness never depended on the probe (`attempt`
/// already re-checks `is_complete()` every iteration), only convergence speed,
/// so a byte-content heuristic buys the same bias at O(1) instead of O(vocab).
const CLOSER_BYTES: &[u8] = b")]}";

fn ends_with_closer(bytes: &[u8]) -> bool {
    bytes.last().is_some_and(|&b| CLOSER_BYTES.contains(&b))
}

/// The two-byte arrow lexeme, as emitted by this vocabulary: `TokenVocab`
/// tokenizes `-` and `>` as separate single-byte tokens (confirmed against a
/// live walk dump), so an arrow is only ever visible as a byte-level suffix
/// across token boundaries, never as one token.
const ARROW_BYTES: &[u8] = b"->";

/// A byte that can continue a bare identifier or number lexeme.
fn is_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Whether appending a token starting with `next_first` right after one
/// ending in `prev_last` would silently *fuse* the two into one lexeme the
/// walker never intended, rather than two adjacent ones — see [`attempt`]'s
/// docs for why the byte-level PDA can't tell the difference itself.
fn would_fuse(prev_last: u8, next_first: u8) -> bool {
    (is_word_byte(prev_last) && is_word_byte(next_first))
        || (prev_last == b'\'' && next_first == b'\'')
}

/// State tracked between steps of [`attempt`] to force a `->name` hop through
/// its mandatory call parens — see that function's docs for why the PDA
/// itself can't be trusted to require this.
enum PendingCall {
    /// No `->` in flight.
    None,
    /// Just accepted the `>` closing a `->`; the next token is forced to be
    /// an identifier by `AfterArrow`'s own rules, so no walker action is
    /// needed until it lands.
    JustArrowed,
    /// Just accepted the identifier immediately after a `->`; the *next*
    /// token must be forced to `(`, since nothing else in the grammar's
    /// accepting state distinguishes this from an already-complete value.
    MustOpen,
}

/// Attempt one accepting walk from `seed` over `grammar`/`schema`. Returns the
/// token-id sequence and the PRNG's final state (so the next walk resumes the
/// same SplitMix64 stream), or `None` if the attempt did not reach a completed
/// session within [`HARD_CAP`] steps.
///
/// Tracks [`PendingCall`]: every grammar production that follows a `->` is a
/// call, always closed with `(`/`)` even when niladic (`->tableToTDS()`,
/// `->count()`, …) per `docs/spec/grammar.md` §5.2-§5.3's `term`/`reducer`
/// productions — no `->name` in the grammar is ever exempt. The compiled L1
/// PDA doesn't encode that: it reaches the same accepting state
/// (`AfterValue`) whether the last hop was `->` (a pending call) or `.` (a
/// property access, already complete on its own — verified live:
/// `|Class.'prop'` and `|Db` alone both parse) — a deliberate L1
/// over-approximation (`docs/spec/grammar.md` §5.10: the live compiler oracle
/// is the documented backstop for exactly this residue). Trusting
/// `is_complete()` alone here reproduced that residue as a walker bug: a real
/// walk generated for `world_1` was `|spider::world_1::Db->tableToTDS` with no
/// trailing `()`, which `grammarToJson/lambda` rejects
/// (`no viable alternative at input '->tableToTDS}'`).
///
/// A first fix tried a simple debt *counter*, incremented on `->` and
/// decremented on the next `(` seen anywhere — too weak: a `(` opened by an
/// unrelated *later* construct (e.g. a subquery's own call) paid off a debt
/// that `tableToTDS` itself never did, letting the same class of walk through
/// again. `PendingCall` instead forces the *very next* token once a `->name`
/// lands — a hard mask override, not a bias — so the call and its parens can
/// never be separated by anything else.
///
/// Also excludes ever *fusing* two tokens (`would_fuse`/`last_byte` below).
/// The compiled PDA is purely byte-level and vocabulary tokens are whole
/// multi-byte lexemes, so nothing stops the walker from picking, say, `"Db"`
/// then `"min"` back to back: from the PDA's view that's just the byte run
/// `D`,`b`,`m`,`i`,`n` — indistinguishable from one identifier `Dbmin`, since
/// an identifier has no self-terminating byte and `InIdent`/`InSourceIdent`
/// happily keep consuming letters regardless of which vocab token they came
/// from. A live walk hit exactly this (`"72"` + `"1"` fused into the numeral
/// `721`, several bare words fused into one 40-byte nonsense identifier) —
/// invisible to `schema_walk_completeness.rs`/`schema_walk_properties.rs`
/// because a fused identifier is still trivially L1/L2-admissible; only a
/// real compiler cares. Quoted literals have the analogous hazard through
/// `InStrLitPending`'s escape rule (see the retired `just_closed_quote` case,
/// now folded into `would_fuse`): Pure escapes embedded quotes with a
/// backslash, not by doubling them (verified live: `|'a\'b'` parses,
/// `|'a''b'` is rejected as `no viable alternative`), so two adjacent literal
/// tokens are never valid regardless of arity. Same class of fix as
/// `PendingCall`, generalized to every fusion-prone byte class.
///
/// `grow_target` is `None` for the normal, varied-length mode (drawn from the
/// same RNG stream as before, preserving every existing caller's exact
/// sequence) or `Some(n)` to fix it — `Some(0)` is the eager mode used by
/// [`generate_first_complete_schema_walks`], stopping at the first token
/// sequence that is genuinely complete instead of forcing further growth into
/// grammar territory the byte-PDA doesn't validate semantically (operator and
/// predicate chaining — see that function's docs).
fn attempt(
    grammar: &CompiledGrammar,
    schema: &Schema,
    seed: u64,
    grow_target: Option<u64>,
) -> (Option<Vec<u32>>, u64) {
    let mut rng = SplitMix64::new(seed);
    let grow_target = grow_target.unwrap_or_else(|| GROW_MIN + rng.below(GROW_MAX - GROW_MIN));
    let mut session = DecoderSession::with_schema(grammar, schema.clone())
        .expect("a fixed-engine grammar always accepts a schema overlay");
    let vocab = grammar.vocab();
    let mut out: Vec<u32> = Vec::new();
    let mut emitted: Vec<u8> = Vec::new();
    let mut pending = PendingCall::None;
    let mut last_byte: Option<u8> = None;

    for _ in 0..HARD_CAP {
        let growing = (out.len() as u64) < grow_target;
        if !growing && walk_is_done(&pending, &session, out.len()) {
            return (Some(out), rng.state);
        }
        let cands = build_candidates(&mut session, vocab, &pending, growing, last_byte);
        if cands.is_empty() {
            // Under `MustOpen`, an empty `cands` means `(` was not
            // admissible here despite every `->name` production requiring
            // it — a real grammar contradiction, not a dead end to accept
            // silently as complete.
            return if walk_is_done(&pending, &session, out.len()) {
                (Some(out), rng.state)
            } else {
                (None, rng.state)
            };
        }
        let id = weighted_pick(&cands, &mut rng);
        // The id came from `allowed_mask()`, so `accept_token` is expected to
        // succeed; the `Err` arm is a defensive guard that abandons the
        // attempt rather than trusting the invariant blindly.
        if session.accept_token(id).is_err() {
            return (None, rng.state);
        }
        out.push(id);
        let bytes = vocab
            .bytes(id)
            .expect("the id just accepted is a real token");
        emitted.extend_from_slice(bytes);
        pending = match pending {
            PendingCall::MustOpen => PendingCall::None, // the forced `(` just landed.
            PendingCall::JustArrowed => PendingCall::MustOpen, // the fn name just landed.
            PendingCall::None if emitted.ends_with(ARROW_BYTES) => PendingCall::JustArrowed,
            PendingCall::None => PendingCall::None,
        };
        last_byte = bytes.last().copied();
    }
    if walk_is_done(&pending, &session, out.len()) {
        (Some(out), rng.state)
    } else {
        (None, rng.state)
    }
}

/// Whether `attempt` may stop here: no `->name` call is still owed, the
/// session is genuinely complete, and the walk has met [`MIN_LEN`].
fn walk_is_done(pending: &PendingCall, session: &DecoderSession, out_len: usize) -> bool {
    matches!(pending, PendingCall::None) && session.is_complete() && out_len >= MIN_LEN
}

/// Build this step's weighted candidate set, applying (in order) the
/// `MustOpen` hard override, the fusion exclusion, and the closing-phase
/// bias — see [`attempt`]'s docs for what each guards against. Every id here
/// comes from `allowed_mask()`, so a later `accept_token` is guaranteed to
/// succeed (the mask/accept invariant `mask_properties.rs` proves) — no
/// per-candidate probe needed to confirm admissibility.
fn build_candidates(
    session: &mut DecoderSession,
    vocab: &Vocab,
    pending: &PendingCall,
    growing: bool,
    last_byte: Option<u8>,
) -> Vec<(u32, u32)> {
    let ids: Vec<u32> = session.allowed_mask().iter_ones().collect();
    let eos = vocab.len() as u32;
    let mut cands: Vec<(u32, u32)> = Vec::new();
    for id in ids {
        if id == eos {
            // is_complete() is false whenever this is called (the caller
            // already returned otherwise), and EOS is admissible only when
            // complete, but skip defensively rather than trust that pairing
            // blindly.
            continue;
        }
        let bytes = vocab
            .bytes(id)
            .expect("a non-EOS admissible id is a real token");
        if matches!(pending, PendingCall::MustOpen) {
            // Hard override: only `(` is a legal next step while a call is
            // owed, dropping every other admissible candidate.
            if bytes == b"(" {
                cands.push((id, DEFAULT_WEIGHT));
            }
            continue;
        }
        if let (Some(prev), Some(&next)) = (last_byte, bytes.first())
            && would_fuse(prev, next)
        {
            // Never let this candidate fuse with the last token — see
            // `attempt`'s doc comment above.
            continue;
        }
        let w = if !growing && ends_with_closer(bytes) {
            DEFAULT_WEIGHT + ACCEPT_BONUS
        } else {
            DEFAULT_WEIGHT
        };
        cands.push((id, w));
    }
    cands
}

/// Generate exactly [`WALK_COUNT`] deterministic accepting walks (as token-id
/// sequences) over `grammar` under `schema`'s L2 overlay. Each attempt resumes
/// the previous one's final PRNG state, so successful and failed attempts
/// alike form one reproducible SplitMix64 stream.
///
/// The count is a guarantee, not a target: the loop runs until [`WALK_COUNT`]
/// walks are collected, bounded by [`ATTEMPT_LIMIT`] purely so a bug can never
/// spin forever, and a final assertion turns any shortfall into a failure at
/// this source rather than a confusing mismatch downstream.
///
/// `dead_code`-allowed: `live_legend_schema_walk_compile.rs`'s compilation
/// unit includes this module via `#[path]` but only calls the eager
/// [`generate_first_complete_schema_walks`].
#[allow(dead_code)]
#[must_use]
pub fn generate_schema_walks(grammar: &CompiledGrammar, schema: &Schema) -> Vec<Vec<u32>> {
    let mut walks = Vec::with_capacity(WALK_COUNT);
    let mut seed = BASE_SEED;
    let mut attempts = 0usize;
    while walks.len() < WALK_COUNT && attempts < ATTEMPT_LIMIT {
        attempts += 1;
        let (walk, next_state) = attempt(grammar, schema, seed, None);
        seed = next_state;
        if let Some(ids) = walk {
            walks.push(ids);
        }
    }
    assert_eq!(
        walks.len(),
        WALK_COUNT,
        "generate_schema_walks fell short of WALK_COUNT within ATTEMPT_LIMIT attempts"
    );
    walks
}

/// Distinct from [`BASE_SEED`] so the eager stream below never coincides with
/// the varied-length one.
///
/// `dead_code`-allowed: `schema_walk_completeness.rs`/`schema_walk_properties.rs`
/// include this module via `#[path]` but only call [`generate_schema_walks`],
/// not the eager generator below.
#[allow(dead_code)]
const EAGER_BASE_SEED: u64 = 0x4561_6765_7257_616B; // "EagerWak" as ASCII bytes.

/// Generate exactly [`WALK_COUNT`] deterministic accepting walks that stop at
/// the *first* point the schema-aware session is genuinely complete
/// (`grow_target = 0`, `MIN_LEN = 1`), rather than [`generate_schema_walks`]'s
/// forced further growth.
///
/// Built for the live-engine compile-rate proof (issue #55), not as a
/// replacement for `generate_schema_walks`'s broader mask/L2-coverage role
/// (`schema_walk_completeness.rs`, `schema_walk_properties.rs` keep using
/// that one unchanged). Forced growth deliberately pushes those consumers
/// into longer, more varied constructs — valuable for exercising the mask —
/// but every one of those longer constructs also has to *compile*, and L1's
/// documented over-approximation (`docs/spec/grammar.md` §5.10) means longer
/// walks monotonically increase the odds of wandering into a residue the
/// byte-PDA doesn't track semantically (operator/predicate chaining such as
/// `.'a'|'b'`, which `PendingCall`/`would_fuse` above don't and shouldn't try
/// to cover — unlike a `->name` call's parens or a literal's closing quote,
/// "is this token position semantically a predicate" is not a structural,
/// bracket-balance fact L1 can decide). Stopping at first completion is also
/// the only mode a *real* decode loop ever exercises: an
/// actual sampler stops the moment `is_complete()` admits EOS and it's
/// sampled, so this walk set is a closer proxy for real decode behavior than
/// `generate_schema_walks`'s forced-growth one — though not, on its own,
/// sufficient to reach issue #55's 100% compile-rate target: replaying it
/// live still surfaces residue this generator has no way to close (missing
/// L2 property-narrowing coverage, bare-class-vs-instance-typed navigation) —
/// see `live_legend_schema_walk_compile.rs`'s doc comment for the full
/// breakdown.
///
/// `dead_code`-allowed: `schema_walk_completeness.rs`/`schema_walk_properties.rs`
/// include this module via `#[path]` but only call [`generate_schema_walks`].
#[allow(dead_code)]
#[must_use]
pub fn generate_first_complete_schema_walks(
    grammar: &CompiledGrammar,
    schema: &Schema,
) -> Vec<Vec<u32>> {
    let mut walks = Vec::with_capacity(WALK_COUNT);
    let mut seed = EAGER_BASE_SEED;
    let mut attempts = 0usize;
    while walks.len() < WALK_COUNT && attempts < ATTEMPT_LIMIT {
        attempts += 1;
        let (walk, next_state) = attempt(grammar, schema, seed, Some(0));
        seed = next_state;
        if let Some(ids) = walk {
            walks.push(ids);
        }
    }
    assert_eq!(
        walks.len(),
        WALK_COUNT,
        "generate_first_complete_schema_walks fell short of WALK_COUNT within ATTEMPT_LIMIT attempts"
    );
    walks
}
