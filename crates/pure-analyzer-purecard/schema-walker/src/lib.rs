#![forbid(unsafe_code)]
#![deny(missing_docs)]

//! Seeded, schema-aware accepting-walk generator for PureCARD (issue #59).
//!
//! Not part of the published `purecard` crate: this is test/fuzz support
//! code, `publish = false`, kept as its own crate rather than a
//! `tests/support/*.rs` file so both `pure-analyzer-purecard`'s integration
//! tests (`tests/schema_walk_*.rs`, `tests/live_legend_schema_walk_compile.rs`)
//! and its separate, workspace-excluded `fuzz/` crate can depend on it — a
//! loose `#[path]`-included test file cannot cross that boundary, since
//! `fuzz/` only ever sees `purecard`'s own published API.
//!
//! Sibling to `tests/support/walker.rs`'s L1-only clone-and-probe generator
//! (the seed corpus for the byte-PDA-only lanes), not a rewrite of it: this
//! generator instead drives a real
//! [`DecoderSession::with_schema`](purecard::DecoderSession::with_schema),
//! choosing among **vocabulary token ids** via `allowed_mask()`/
//! `accept_token()` at every step — so the schema's L2 scope machine and mask
//! constrain the walk exactly the way a host's decode loop would, not just L1
//! byte admissibility.
//!
//! Same overall shape as that L1-only generator's algorithm: grow to a
//! per-walk length target, then bias hard toward whichever candidate looks
//! likely to finish the walk, so every attempt that doesn't die converges to
//! an accepting walk in bounded steps. Candidates here are read directly from
//! `allowed_mask()` rather than clone-and-probed: the admissibility a
//! byte-level walker needs a probe to establish is already guaranteed by the
//! mask/accept invariant, and the completion bias uses a cheap byte-content
//! heuristic (`ends_with_closer`) instead of a simulated look-ahead, since a
//! vocabulary here can hold hundreds of lexemes versus a byte alphabet's few
//! dozen — see `attempt`'s docs. The SplitMix64 PRNG here is its own copy,
//! not shared with the L1-only generator's: the two pick among different
//! candidate spaces with different weighting rules, so factoring out just the
//! RNG would couple two otherwise-independent modules for a ~20-line,
//! fully-specified algorithm neither needs from the other.

use purecard::{CompiledGrammar, DecoderSession, Schema, Vocab};

/// The single name a pipeline-source dot is ever narrowed to (`SOURCE_METHOD`
/// in `pure-analyzer-purecard`'s `src/schema/scope.rs`) — kept as its own
/// literal since that constant is `pub(crate)` to that crate and this is a
/// separate crate.
const SOURCE_METHOD: &str = "all";

/// Number of accepting walks a full generation produces per schema.
pub const WALK_COUNT: usize = 64;

/// Upper bound on generation attempts — a safety valve so a bug can never spin
/// forever. Comfortably above [`WALK_COUNT`], since the biased close-out lands
/// an accepting walk on nearly every seed.
const ATTEMPT_LIMIT: usize = WALK_COUNT * 64;

/// The base seed; walk `i` derives from `BASE_SEED` advanced past every seed a
/// prior walk consumed, so the set is one deterministic stream, not
/// [`WALK_COUNT`] correlated low seeds. Distinct from
/// [`EAGER_BASE_SEED`] so the two generators' streams never accidentally
/// coincide.
const BASE_SEED: u64 = 0x5363_6865_6d61_5761; // "SchemaWa" as ASCII bytes.

/// Shortest accepted walk kept, in tokens.
const MIN_LEN: usize = 1;

/// The per-walk growth target, in tokens, is drawn from `[GROW_MIN, GROW_MAX)`;
/// until reached, every admissible candidate is eligible. Past it, the walk
/// closes toward completion. Tokens are whole lexemes (often several bytes),
/// so this target is smaller than a byte-count target for a comparably shaped
/// walk.
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

/// SplitMix64 — see the module docs for why this is its own copy rather than
/// shared with the L1-only generator's.
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

/// Resolve `attempt`'s per-walk growth target: the caller's explicit value if
/// given, otherwise a fresh draw from `[GROW_MIN, GROW_MAX)` off `rng`'s own
/// stream — factored out of `attempt` so the draw's exact bounds are directly
/// unit-testable without needing a full grammar/schema/session fixture.
fn resolve_grow_target(grow_target: Option<u64>, rng: &mut SplitMix64) -> u64 {
    grow_target.unwrap_or_else(|| GROW_MIN + rng.below(GROW_MAX - GROW_MIN))
}

/// Whether `attempt` is still in its forced-growth phase at `out_len` tokens
/// emitted so far — factored out so the exact boundary (`out_len ==
/// grow_target` switches it off) is directly unit-testable.
fn is_growing(out_len: usize, grow_target: u64) -> bool {
    (out_len as u64) < grow_target
}

/// Pick an admissible token id by weight. `cands` is non-empty with at least
/// one positive weight whenever this is called; `None` signals that the
/// precondition didn't hold, so the caller abandons the attempt rather than
/// trusting an invariant it cannot itself verify.
fn weighted_pick(cands: &[(u32, u32)], rng: &mut SplitMix64) -> Option<u32> {
    let total: u32 = cands.iter().map(|&(_, w)| w).sum();
    let mut target = rng.below(u64::from(total)) as u32;
    for &(id, w) in cands {
        if target < w {
            return Some(id);
        }
        target -= w;
    }
    // Provably unreachable given the precondition above: weights sum to
    // `total`, and `target = rng.below(total) < total`, so the last
    // candidate's own `target < w` check always fires before the loop can
    // exit normally.
    None
}

/// Bytes that plausibly end a construct — a token ending in one of these is
/// weighted toward finishing the walk. This is a cheap heuristic on the
/// token's own bytes, not a simulated look-ahead: unlike a tiny byte alphabet
/// (cheap to clone-and-probe each candidate), a vocabulary here can hold
/// hundreds of lexemes, and probing every candidate by cloning the whole
/// schema-aware session at every step was the dominant cost in this
/// generator — correctness never depended on the probe ([`attempt`] already
/// re-checks `is_complete()` every iteration), only convergence speed, so a
/// byte-content heuristic buys the same bias at O(1) instead of O(vocab).
const CLOSER_BYTES: &[u8] = b")]}";

fn ends_with_closer(bytes: &[u8]) -> bool {
    bytes.last().is_some_and(|&b| CLOSER_BYTES.contains(&b))
}

/// The two-byte arrow lexeme, as emitted by this vocabulary: a token vocab
/// tokenizes `-` and `>` as separate single-byte tokens, so an arrow is only
/// ever visible as a byte-level suffix across token boundaries, never as one
/// token.
const ARROW_BYTES: &[u8] = b"->";

/// Whether the token just emitted (`bytes`) is a bound-variable reference
/// (`$`), or completes a `->` arrow hop (`emitted`, the full byte run so
/// far, ends in [`ARROW_BYTES`]) — either marks that the pipeline's source
/// dot has already been passed, so a later `.` is an ordinary property
/// access, never the source method's own dot again.
fn marks_arrow_or_dollar(bytes: &[u8], emitted: &[u8]) -> bool {
    bytes == b"$" || emitted.ends_with(ARROW_BYTES)
}

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
/// invisible to soundness/property tests because a fused identifier is still
/// trivially L1/L2-admissible; only a real compiler cares. Quoted literals
/// have the analogous hazard through `InStrLitPending`'s escape rule: Pure
/// escapes embedded quotes with a backslash, not by doubling them (verified
/// live: `|'a\'b'` parses, `|'a''b'` is rejected as `no viable alternative`),
/// so two adjacent literal tokens are never valid regardless of arity. Same
/// class of fix as `PendingCall`, generalized to every fusion-prone byte
/// class.
///
/// `grow_target` is `None` for the normal, varied-length mode (drawn from the
/// same RNG stream as before, preserving every existing caller's exact
/// sequence) or `Some(n)` to fix it — `Some(0)` is the eager mode used by
/// [`generate_first_complete_schema_walks`], stopping at the first token
/// sequence that is genuinely complete instead of forcing further growth into
/// grammar territory the byte-PDA doesn't validate semantically (operator and
/// predicate chaining — see that function's docs).
///
/// `pending_source_method` guards a third residue, found live against issue
/// #56's S1 narrowing: `DecoderSession::is_complete()` is
/// `Pda::is_accepting()` — a pure L1 *lookahead* fact (does a value-boundary
/// byte from here reach `AfterValue`?) that never consults the L2-narrowed
/// mask. Any partial identifier is trivially "completable" under that
/// definition, because an identifier has no self-terminating byte —
/// `InIdent`'s own rule `goto AfterValue` on any non-continuation byte fires
/// regardless of how much of the identifier is actually typed. So the moment
/// the vocabulary happens to hold a standalone token that is *also* a strict
/// byte-prefix of the one name S1 forces (`"a"` next to `"all"`), the walker
/// can stop there and call it done — confirmed live: a real walk ended in
/// `Class.a`, which the engine correctly rejects (`can't find property 'a'`).
/// No L2 change can fix this (`is_complete()` doesn't read the mask at all);
/// the walker has to stop trusting it at the one position it's known to be
/// forced. The first `.` before any `->`/`$` can only be the pipeline
/// source's own dot (`pipeline = source , { "->" step }`, `source = classpath
/// ".all()"` — structurally nothing else precedes it), so it needs no L2
/// visibility to detect. `source_method_progress` accumulates the
/// non-whitespace bytes emitted since that dot (whitespace before/inside the
/// identifier is legal Pure and carries no identifier content); once it
/// exactly matches `SOURCE_METHOD`, the identifier is genuinely done — S1's
/// narrowing only ever lets an exact match through (anything else diverges
/// the trie and is excluded from the mask) — and `pending` is armed to
/// `PendingCall::MustOpen`: `all` is itself a niladic call (`.all()`), the
/// same "mandatory parens" fact `PendingCall` already enforces for `->` hops
/// (confirmed live: bare `Class.all` parses as a property read and fails to
/// compile the same way `Db->tableToTDS` without `()` did), so the fix reuses
/// that machinery rather than inventing a second one. Matching by accumulated
/// byte content rather than by re-inspecting `Pda::state()` matters here: the
/// PDA doesn't reach a clean `AfterValue` boundary until the *next* byte is
/// processed (an identifier has no self-terminating byte), so checking the
/// state immediately after accepting `all` itself would still read `InIdent`
/// and never fire — the exact same lookahead gap `is_complete()` has, one
/// layer down.
fn attempt(
    grammar: &CompiledGrammar,
    schema: &Schema,
    seed: u64,
    grow_target: Option<u64>,
) -> (Option<Vec<u32>>, u64) {
    let mut rng = SplitMix64::new(seed);
    let grow_target = resolve_grow_target(grow_target, &mut rng);
    // A fixed-engine grammar always accepts a schema overlay; the `Err` arm
    // is a defensive guard that abandons the attempt rather than trusting
    // that invariant blindly.
    let Ok(mut session) = DecoderSession::with_schema(grammar, schema.clone()) else {
        return (None, rng.state);
    };
    let vocab = grammar.vocab();
    let mut out: Vec<u32> = Vec::new();
    let mut emitted: Vec<u8> = Vec::new();
    let mut pending = PendingCall::None;
    let mut last_byte: Option<u8> = None;
    let mut seen_arrow_or_dollar = false;
    let mut pending_source_method = false;
    let mut source_method_progress: Vec<u8> = Vec::new();

    for _ in 0..HARD_CAP {
        let growing = is_growing(out.len(), grow_target);
        if !growing && attempt_may_stop(pending_source_method, &pending, &session, out.len()) {
            return (Some(out), rng.state);
        }
        let cands = build_candidates(&mut session, vocab, &pending, growing, last_byte);
        if cands.is_empty() {
            // Under `MustOpen`, an empty `cands` means `(` was not
            // admissible here despite every `->name` production requiring
            // it — a real grammar contradiction, not a dead end to accept
            // silently as complete.
            return if attempt_may_stop(pending_source_method, &pending, &session, out.len()) {
                (Some(out), rng.state)
            } else {
                (None, rng.state)
            };
        }
        // `cands` is non-empty (checked above) with at least one positive
        // weight (`build_candidates` never pushes a zero weight), so
        // `weighted_pick` always returns `Some`; the `None` arm is a
        // defensive guard that abandons the attempt rather than trusting
        // that invariant blindly.
        let Some(id) = weighted_pick(&cands, &mut rng) else {
            return (None, rng.state);
        };
        // The id came from `allowed_mask()`, so `accept_token` is expected to
        // succeed; the `Err` arm is a defensive guard that abandons the
        // attempt rather than trusting the invariant blindly.
        if session.accept_token(id).is_err() {
            return (None, rng.state);
        }
        out.push(id);
        // `id` was just accepted, so it is a real vocabulary token; the
        // `None` arm is a defensive guard that abandons the attempt rather
        // than trusting that invariant blindly.
        let Some(bytes) = vocab.bytes(id) else {
            return (None, rng.state);
        };
        emitted.extend_from_slice(bytes);
        pending = match pending {
            PendingCall::MustOpen => PendingCall::None, // the forced `(` just landed.
            PendingCall::JustArrowed => PendingCall::MustOpen, // the fn name just landed.
            PendingCall::None if emitted.ends_with(ARROW_BYTES) => PendingCall::JustArrowed,
            PendingCall::None => PendingCall::None,
        };
        if marks_arrow_or_dollar(bytes, &emitted) {
            seen_arrow_or_dollar = true;
        }
        if bytes == b"." && !seen_arrow_or_dollar {
            // The first `.` before any `->`/`$` can only be the pipeline
            // source's own dot (`pipeline = source , { "->" step }`,
            // `source = classpath ".all()"` — nothing else precedes it
            // structurally) — see the doc comment above for why
            // `is_complete()` cannot be trusted here on its own.
            pending_source_method = true;
            source_method_progress.clear();
        } else if pending_source_method {
            // Whitespace between the dot and the identifier is legal Pure
            // and carries no identifier bytes — skip it rather than let it
            // break the exact-match check below.
            if !bytes.iter().all(u8::is_ascii_whitespace) {
                source_method_progress.extend_from_slice(bytes);
            }
            if source_method_progress == SOURCE_METHOD.as_bytes() {
                // The forced identifier exactly matches — S1's narrowing
                // only ever lets that happen on an exact match (anything
                // else diverges the trie and is excluded from the mask).
                // `SOURCE_METHOD` (`all`) is itself a niladic call
                // (`source = classpath ".all()"`) — the same "every
                // `->name`/`.name` this grammar admits is a call, parens
                // mandatory" fact `PendingCall` already enforces for `->`
                // hops (confirmed live: `Class.all` without `()` parses as
                // a bare property read and fails to compile the same way
                // `Db->tableToTDS` without `()` did) — so reuse it here
                // rather than a second, parallel "force `(`" mechanism.
                pending_source_method = false;
                pending = PendingCall::MustOpen;
            }
        }
        last_byte = bytes.last().copied();
    }
    if attempt_may_stop(pending_source_method, &pending, &session, out.len()) {
        (Some(out), rng.state)
    } else {
        (None, rng.state)
    }
}

/// Whether `attempt` may stop right now — the pipeline-source identifier
/// isn't mid-match (`pending_source_method`) and [`walk_is_done`] agrees.
/// Factored out of `attempt`'s three call sites (the top-of-loop early exit,
/// the dead-end `cands.is_empty()` branch, and the trailing post-loop check)
/// both to avoid repeating the same condition three times and so it is
/// directly unit-testable against a hand-driven session, without needing a
/// specific seed to land in a specific state.
fn attempt_may_stop(
    pending_source_method: bool,
    pending: &PendingCall,
    session: &DecoderSession,
    out_len: usize,
) -> bool {
    !pending_source_method && walk_is_done(pending, session, out_len)
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
/// succeed (the mask/accept invariant proves) — no per-candidate probe needed
/// to confirm admissibility.
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
        // A non-EOS admissible id is a real vocabulary token; skip
        // defensively rather than trust that invariant blindly.
        let Some(bytes) = vocab.bytes(id) else {
            continue;
        };
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
/// walks are collected, bounded by an internal attempt limit purely so a bug
/// can never spin forever, and a final assertion turns any shortfall into a
/// failure at this source rather than a confusing mismatch downstream.
///
/// # Panics
///
/// Panics if fewer than [`WALK_COUNT`] walks are collected within the
/// internal attempt limit.
#[must_use]
pub fn generate_schema_walks(grammar: &CompiledGrammar, schema: &Schema) -> Vec<Vec<u32>> {
    generate_walks(grammar, schema, BASE_SEED, None, "generate_schema_walks")
}

/// Whether the shared retry loop in [`generate_walks`] should keep going:
/// fewer than [`WALK_COUNT`] walks collected so far, and fewer than
/// [`ATTEMPT_LIMIT`] attempts made — factored out so both boundaries are
/// directly unit-testable without needing a full grammar/schema fixture.
fn keep_generating(walks_len: usize, attempts: usize) -> bool {
    walks_len < WALK_COUNT && attempts < ATTEMPT_LIMIT
}

/// Shared retry loop behind both [`generate_schema_walks`] and
/// [`generate_first_complete_schema_walks`]: gather exactly [`WALK_COUNT`]
/// accepting walks from `base_seed`'s SplitMix64 stream, retrying (via each
/// [`attempt`]'s own returned next-seed state) up to [`ATTEMPT_LIMIT`] times.
/// `label` names the caller in the panic message on shortfall.
fn generate_walks(
    grammar: &CompiledGrammar,
    schema: &Schema,
    base_seed: u64,
    grow_target: Option<u64>,
    label: &str,
) -> Vec<Vec<u32>> {
    let mut walks = Vec::with_capacity(WALK_COUNT);
    let mut seed = base_seed;
    let mut attempts = 0usize;
    while keep_generating(walks.len(), attempts) {
        attempts += 1;
        let (walk, next_state) = attempt(grammar, schema, seed, grow_target);
        seed = next_state;
        if let Some(ids) = walk {
            walks.push(ids);
        }
    }
    assert_eq!(
        walks.len(),
        WALK_COUNT,
        "{label} fell short of WALK_COUNT within ATTEMPT_LIMIT attempts"
    );
    walks
}

/// Distinct from [`BASE_SEED`] so the eager stream below never coincides with
/// the varied-length one.
const EAGER_BASE_SEED: u64 = 0x4561_6765_7257_616B; // "EagerWak" as ASCII bytes.

/// Generate exactly [`WALK_COUNT`] deterministic accepting walks that stop at
/// the *first* point the schema-aware session is genuinely complete
/// (`grow_target = 0`, `MIN_LEN = 1`), rather than [`generate_schema_walks`]'s
/// forced further growth.
///
/// Built for the live-engine compile-rate proof (issue #55), not as a
/// replacement for [`generate_schema_walks`]'s broader mask/L2-coverage role.
/// Forced growth deliberately pushes those consumers into longer, more varied
/// constructs — valuable for exercising the mask — but every one of those
/// longer constructs also has to *compile*, and L1's documented
/// over-approximation (`docs/spec/grammar.md` §5.10) means longer walks
/// monotonically increase the odds of wandering into a residue the byte-PDA
/// doesn't track semantically (operator/predicate chaining such as
/// `.'a'|'b'`, which `PendingCall`/`would_fuse` above don't and shouldn't try
/// to cover — unlike a `->name` call's parens or a literal's closing quote,
/// "is this token position semantically a predicate" is not a structural,
/// bracket-balance fact L1 can decide). Stopping at first completion is also
/// the only mode a *real* decode loop ever exercises: an actual sampler stops
/// the moment `is_complete()` admits EOS and it's sampled, so this walk set
/// is a closer proxy for real decode behavior than [`generate_schema_walks`]'s
/// forced-growth one — though not, on its own, sufficient to reach issue
/// #55's 100% compile-rate target for every construct shape: replaying it
/// live can still surface residue this generator has no way to close (e.g.
/// missing L2 property-narrowing coverage, bare-class-vs-instance-typed
/// navigation).
///
/// # Panics
///
/// Panics if fewer than [`WALK_COUNT`] walks are collected within the
/// internal attempt limit.
#[must_use]
pub fn generate_first_complete_schema_walks(
    grammar: &CompiledGrammar,
    schema: &Schema,
) -> Vec<Vec<u32>> {
    generate_walks(
        grammar,
        schema,
        EAGER_BASE_SEED,
        Some(0),
        "generate_first_complete_schema_walks",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
      "db_id": "d", "db_path": "spider::d::Db",
      "classes": { "A": { "simple_name": "A", "properties": [] } },
      "associations": [], "enums": {}
    }"#;

    fn schema() -> Schema {
        Schema::from_json(SAMPLE).expect("parses")
    }

    /// A vocabulary reproducing the exact ambiguity found live against a real
    /// schema: a standalone single-byte token (`"a"`) that is also a strict
    /// byte-prefix of the only name S1 ever forces after the pipeline-source
    /// dot (`SOURCE_METHOD`, `"all"`). Deliberately excludes any token that
    /// is *also* a byte-prefix of `LET_KEYWORD` (`"let"`, `narrow.rs`) — an
    /// `"l"` fragment was tried first and is a prefix of `"let"` too, which
    /// let the walker pick it as a (masked-admissible, since N3's own
    /// narrowing has the identical prefix-vs-lookahead gap this fix does not
    /// touch) *source classpath*, an unrelated confound this test isn't
    /// about.
    fn vocab_with_source_method_ambiguity() -> Vocab {
        let tokens: Vec<Vec<u8>> = ["|", "A", ".", "a", "all", "(", ")", "\n  "]
            .iter()
            .map(|s| s.as_bytes().to_vec())
            .collect();
        let eos = tokens.len() as u32;
        Vocab::from_byte_tokens(tokens, eos)
    }

    /// Decode a walk's token ids to text via `grammar`'s vocabulary.
    fn decode(grammar: &CompiledGrammar, walk: &[u32]) -> String {
        let mut text = Vec::new();
        for &id in walk {
            text.extend_from_slice(grammar.vocab().bytes(id).expect("real token"));
        }
        String::from_utf8(text).expect("ASCII vocabulary")
    }

    #[test]
    fn every_walk_opens_the_source_method_call_with_the_full_name_never_a_prefix() {
        // Checks exactly the two facts `pending_source_method`/`MustOpen`
        // fix: the identifier is never truncated (`a`/`al`), and it's always
        // followed by an opening `(` rather than left as a bare property
        // read (`Class.all` alone, confirmed live to fail to compile the
        // same way `Db->tableToTDS` without `()` did).
        //
        // Deliberately *not* asserted: that the call closes immediately with
        // no arguments. `.all()` is niladic in the real grammar, but nothing
        // here yet forces that — `MustOpen` only forces the opening `(`, the
        // same as it does for `->name(args)` calls that legitimately *do*
        // take arguments, so once inside, this minimal vocabulary's only
        // content-bearing token (`"all"`, reused as a generic value) can
        // fill the argument slot (confirmed live and reproduced here: a real
        // walk closed as `Class.all('French')`/`A.all(all)`). Closing that
        // gap needs a `.all()`-specific "must close immediately" state
        // distinct from `MustOpen`'s shared, argument-permitting one — out
        // of scope for this fix, which targets the vastly more common
        // truncated-identifier failure (over 40/64 walks in the original
        // live baseline) rather than this narrower one (1/64).
        let grammar = CompiledGrammar::compile(vocab_with_source_method_ambiguity());
        let schema = schema();
        let walks = generate_first_complete_schema_walks(&grammar, &schema);
        assert_eq!(walks.len(), WALK_COUNT);
        for (index, walk) in walks.iter().enumerate() {
            let text = decode(&grammar, walk);
            let stripped: String = text.chars().filter(|c| !c.is_whitespace()).collect();
            assert!(
                stripped.starts_with("|A.all("),
                "walk {index} did not open `A.all(` with the full name: {text:?}"
            );
        }
    }

    /// Drive `session` through `pieces` by scanning `vocab` for the
    /// byte-matching id at each step — a manual walk rather than
    /// `generate_schema_walks`, so the caller lands at an exact, chosen
    /// point in the grammar.
    fn drive(session: &mut DecoderSession, vocab: &Vocab, pieces: &[&str]) {
        for piece in pieces {
            let id = (0..vocab.len() as u32)
                .find(|&id| vocab.bytes(id) == Some(piece.as_bytes()))
                .expect("token present in vocab");
            session
                .accept_token(id)
                .expect("token admissible at this step");
        }
    }

    #[test]
    fn build_candidates_applies_the_closing_bias_only_to_a_closer_when_not_growing() {
        let grammar = CompiledGrammar::compile(vocab_with_source_method_ambiguity());
        let mut session = DecoderSession::with_schema(&grammar, schema()).expect("valid overlay");
        let vocab = grammar.vocab();
        drive(&mut session, vocab, &["|", "A", ".", "all", "("]);
        let pending = PendingCall::None;

        let weight_of = |cands: &[(u32, u32)], piece: &str| {
            cands
                .iter()
                .find(|&&(id, _)| vocab.bytes(id) == Some(piece.as_bytes()))
                .map(|&(_, w)| w)
        };

        // Still growing: no candidate gets the closing bonus, including `)`.
        let growing = build_candidates(&mut session, vocab, &pending, true, Some(b'('));
        assert_eq!(weight_of(&growing, ")"), Some(DEFAULT_WEIGHT));
        assert_eq!(weight_of(&growing, "\n  "), Some(DEFAULT_WEIGHT));

        // Done growing: only the closer `)` gets the bonus; a non-closer
        // candidate (the whitespace token) stays at the default weight.
        let closing = build_candidates(&mut session, vocab, &pending, false, Some(b'('));
        assert_eq!(
            weight_of(&closing, ")"),
            Some(DEFAULT_WEIGHT + ACCEPT_BONUS)
        );
        assert_eq!(weight_of(&closing, "\n  "), Some(DEFAULT_WEIGHT));
    }

    /// Adds a `->count()` step's tokens (`-`, `>`, `count`) to the
    /// ambiguity vocab above, so a walk can explore past the source method
    /// into an arrow-hop — reproducing the documented `Db->tableToTDS` (no
    /// trailing `()`) residue `PendingCall::JustArrowed`/`MustOpen` fixes.
    fn vocab_with_arrow_step() -> Vocab {
        let tokens: Vec<Vec<u8>> = ["|", "A", ".", "all", "(", ")", "-", ">", "count", "\n  "]
            .iter()
            .map(|s| s.as_bytes().to_vec())
            .collect();
        let eos = tokens.len() as u32;
        Vocab::from_byte_tokens(tokens, eos)
    }

    #[test]
    fn every_arrow_hop_forces_its_own_call_parens() {
        // `generate_schema_walks` (forced varied-length growth, unlike the
        // eager `generate_first_complete_schema_walks`) is the one that
        // actually explores past `.all()` into a `->name()` hop often
        // enough to exercise it here.
        let grammar = CompiledGrammar::compile(vocab_with_arrow_step());
        let sc = schema();
        let walks = generate_schema_walks(&grammar, &sc);
        assert_eq!(walks.len(), WALK_COUNT);
        let mut saw_arrow_hop = false;
        for (index, walk) in walks.iter().enumerate() {
            let text = decode(&grammar, walk);
            let stripped: String = text.chars().filter(|c| !c.is_whitespace()).collect();
            if let Some(after) = stripped.split("->").nth(1) {
                saw_arrow_hop = true;
                let name_end = after.find(|c: char| !c.is_ascii_alphanumeric());
                assert_eq!(
                    name_end.and_then(|end| after.as_bytes().get(end)),
                    Some(&b'('),
                    "walk {index} had a `->name` not immediately followed by `(`: {text:?}"
                );
            }
        }
        assert!(
            saw_arrow_hop,
            "no generated walk ever explored a `->` hop; the test proves nothing"
        );
    }

    #[test]
    fn attempt_may_stop_requires_both_an_unmatched_source_method_and_walk_is_done() {
        // A hand-driven session (rather than a specific seed happening to
        // land in a specific state) makes both halves of `attempt_may_stop`
        // directly controllable and deterministic.
        let grammar = CompiledGrammar::compile(vocab_with_source_method_ambiguity());
        let mut session = DecoderSession::with_schema(&grammar, schema()).expect("valid overlay");
        let vocab = grammar.vocab();
        drive(&mut session, vocab, &["|", "A", ".", "all", "(", ")"]);
        assert!(session.is_complete());

        // pending_source_method still true (an identifier match in
        // progress): never allowed to stop, regardless of walk_is_done.
        assert!(!attempt_may_stop(true, &PendingCall::None, &session, 1));

        // pending_source_method resolved and walk_is_done holds: stop.
        assert!(attempt_may_stop(false, &PendingCall::None, &session, 1));

        // pending_source_method resolved but a call is still owed
        // (`PendingCall::MustOpen`): walk_is_done is false, so no stop.
        assert!(!attempt_may_stop(
            false,
            &PendingCall::MustOpen,
            &session,
            1
        ));
    }

    #[test]
    fn split_mix64_next_u64_matches_its_golden_values() {
        // Pins the exact bit-mixing (not just "looks random"): a golden-value
        // check is the only way to catch a mutated `^`/`>>` that still
        // produces *some* well-distributed-looking output.
        let mut rng = SplitMix64::new(0);
        assert_eq!(rng.next_u64(), 0xe220_a839_7b1d_cdaf);
        assert_eq!(rng.next_u64(), 0x6e78_9e6a_a1b9_65f4);
    }

    #[test]
    fn weighted_pick_selects_by_cumulative_weight_boundary() {
        // cands = [(10, weight 2), (20, weight 3)], total = 5. Correct
        // weighted selection: target 0-1 -> id 10 (first bucket [0, 2)),
        // target 2-4 -> id 20 (second bucket [2, 5)). Trying every seed in
        // 0..60 hits every residue mod 5 at least once (verified), so this
        // exercises both buckets and the exact `target < w` boundary at
        // target == 2.
        let cands = [(10u32, 2u32), (20u32, 3u32)];
        for seed in 0u64..60 {
            let target = SplitMix64::new(seed).below(5);
            let expected = if target < 2 { 10 } else { 20 };
            let picked = weighted_pick(&cands, &mut SplitMix64::new(seed));
            assert_eq!(
                picked,
                Some(expected),
                "seed {seed}: target {target} should pick {expected}"
            );
        }
    }

    #[test]
    fn ends_with_closer_recognizes_exactly_the_closer_bytes() {
        assert!(ends_with_closer(b")"));
        assert!(ends_with_closer(b"]"));
        assert!(ends_with_closer(b"}"));
        assert!(ends_with_closer(b"foo)"));
        assert!(!ends_with_closer(b"foo"));
        assert!(!ends_with_closer(b""));
    }

    #[test]
    fn is_word_byte_admits_alphanumerics_and_underscore_only() {
        assert!(is_word_byte(b'a'));
        assert!(is_word_byte(b'Z'));
        assert!(is_word_byte(b'9'));
        assert!(is_word_byte(b'_'));
        assert!(!is_word_byte(b'.'));
        assert!(!is_word_byte(b' '));
    }

    #[test]
    fn would_fuse_detects_adjacent_word_bytes_and_adjacent_quotes() {
        // Two word bytes: fuses.
        assert!(would_fuse(b'a', b'b'));
        // A quote followed by a quote: fuses (Pure has no doubled-quote
        // escaping — see the module doc comment).
        assert!(would_fuse(b'\'', b'\''));
        // A word byte followed by a non-word, non-quote byte: no fusion.
        assert!(!would_fuse(b'a', b' '));
        // A quote followed by a non-quote byte: no fusion.
        assert!(!would_fuse(b'\'', b'x'));
        // Neither word nor quote: no fusion.
        assert!(!would_fuse(b' ', b' '));
    }

    #[test]
    fn is_growing_flips_exactly_at_the_target() {
        assert!(is_growing(1, 2));
        assert!(!is_growing(2, 2));
        assert!(!is_growing(3, 2));
    }

    #[test]
    fn marks_arrow_or_dollar_recognizes_either_signal_independently() {
        // `$` alone (no arrow in `emitted`): marks.
        assert!(marks_arrow_or_dollar(b"$", b"x"));
        // An arrow-completing `emitted` alone (`bytes` isn't `$`): marks.
        assert!(marks_arrow_or_dollar(b">", b"->"));
        // Neither: doesn't mark.
        assert!(!marks_arrow_or_dollar(b"x", b"x"));
    }

    /// A vocabulary with no content past the opening pipe: every attempt
    /// dies immediately (`cands.is_empty()` right after `|`), so generation
    /// can never converge — the only way to exercise `ATTEMPT_LIMIT` itself
    /// (never reached by any real, convergent schema/vocab).
    fn unconvergeable_vocab() -> Vocab {
        Vocab::from_byte_tokens(vec![b"|".to_vec()], 1)
    }

    #[test]
    #[should_panic(expected = "fell short of WALK_COUNT")]
    fn generate_schema_walks_gives_up_after_attempt_limit_when_it_cannot_converge() {
        let grammar = CompiledGrammar::compile(unconvergeable_vocab());
        let _ = generate_schema_walks(&grammar, &schema());
    }

    #[test]
    #[should_panic(expected = "fell short of WALK_COUNT")]
    fn generate_first_complete_schema_walks_gives_up_after_attempt_limit_when_it_cannot_converge() {
        let grammar = CompiledGrammar::compile(unconvergeable_vocab());
        let _ = generate_first_complete_schema_walks(&grammar, &schema());
    }

    #[test]
    fn keep_generating_stops_at_either_boundary() {
        // Below both boundaries: keep going.
        assert!(keep_generating(0, 0));
        assert!(keep_generating(WALK_COUNT - 1, ATTEMPT_LIMIT - 1));
        // The walk-count boundary: stop the instant it's reached, even with
        // attempts to spare.
        assert!(!keep_generating(WALK_COUNT, 0));
        // The attempt-limit boundary: stop the instant it's reached, even
        // with walks still short.
        assert!(!keep_generating(0, ATTEMPT_LIMIT));
    }

    #[test]
    fn resolve_grow_target_stays_within_its_documented_bounds() {
        // An explicit target is passed through untouched.
        assert_eq!(resolve_grow_target(Some(7), &mut SplitMix64::new(0)), 7);

        // The default draw always lands in [GROW_MIN, GROW_MAX), and across
        // enough seeds reaches both ends of that range (a narrowed or
        // shifted range would silently pass a bounds-only check).
        let mut saw_min = false;
        let mut saw_max_minus_one = false;
        for seed in 0u64..2000 {
            let g = resolve_grow_target(None, &mut SplitMix64::new(seed));
            assert!(
                (GROW_MIN..GROW_MAX).contains(&g),
                "seed {seed}: grow target {g} outside [{GROW_MIN}, {GROW_MAX})"
            );
            saw_min |= g == GROW_MIN;
            saw_max_minus_one |= g == GROW_MAX - 1;
        }
        assert!(
            saw_min,
            "never observed the minimum grow target over 2000 seeds"
        );
        assert!(
            saw_max_minus_one,
            "never observed the maximum grow target over 2000 seeds"
        );
    }
}
