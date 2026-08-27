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
//! Same shape as `walker`'s algorithm: clone-and-probe from the live session,
//! grow to a per-walk length target while structure (openers) is favoured,
//! then disable openers and bias hard toward whichever candidate reaches
//! `is_complete()`, so every attempt that doesn't die converges to an
//! accepting walk in bounded steps. The SplitMix64 PRNG is a second copy of
//! `walker`'s (not shared): the two generators pick among different candidate
//! spaces (bytes vs. token ids) with different weighting rules, so factoring
//! out just the RNG would couple two otherwise-independent modules for a
//! ~20-line, fully-specified algorithm neither module needs from the other.

use purecard::{CompiledGrammar, DecoderSession, Schema};

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

/// Attempt one accepting walk from `seed` over `grammar`/`schema`. Returns the
/// token-id sequence and the PRNG's final state (so the next walk resumes the
/// same SplitMix64 stream), or `None` if the attempt did not reach a completed
/// session within [`HARD_CAP`] steps.
fn attempt(grammar: &CompiledGrammar, schema: &Schema, seed: u64) -> (Option<Vec<u32>>, u64) {
    let mut rng = SplitMix64::new(seed);
    let grow_target = GROW_MIN + rng.below(GROW_MAX - GROW_MIN);
    let mut session = DecoderSession::with_schema(grammar, schema.clone())
        .expect("a fixed-engine grammar always accepts a schema overlay");
    let mut out: Vec<u32> = Vec::new();

    for _ in 0..HARD_CAP {
        let growing = (out.len() as u64) < grow_target;
        if !growing && session.is_complete() && out.len() >= MIN_LEN {
            return (Some(out), rng.state);
        }
        let ids: Vec<u32> = session.allowed_mask().iter_ones().collect();
        let eos = grammar.vocab().len() as u32;
        let mut cands: Vec<(u32, u32)> = Vec::new();
        for id in ids {
            if growing && id == eos {
                // EOS never grows the walk; only offered once closing.
                continue;
            }
            let mut probe = session.clone();
            if probe.accept_token(id).is_ok() {
                let base = DEFAULT_WEIGHT;
                let w = if !growing && probe.is_complete() {
                    base + ACCEPT_BONUS
                } else {
                    base
                };
                cands.push((id, w));
            }
        }
        if cands.is_empty() {
            return if session.is_complete() && out.len() >= MIN_LEN {
                (Some(out), rng.state)
            } else {
                (None, rng.state)
            };
        }
        let id = weighted_pick(&cands, &mut rng);
        // The id was chosen from probed-live candidates, so `accept_token` is
        // expected to succeed; the `Err` arm is a defensive guard that
        // abandons the attempt rather than trusting the invariant blindly.
        if session.accept_token(id).is_err() {
            return (None, rng.state);
        }
        out.push(id);
    }
    if session.is_complete() && out.len() >= MIN_LEN {
        (Some(out), rng.state)
    } else {
        (None, rng.state)
    }
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
#[must_use]
pub fn generate_schema_walks(grammar: &CompiledGrammar, schema: &Schema) -> Vec<Vec<u32>> {
    let mut walks = Vec::with_capacity(WALK_COUNT);
    let mut seed = BASE_SEED;
    let mut attempts = 0usize;
    while walks.len() < WALK_COUNT && attempts < ATTEMPT_LIMIT {
        attempts += 1;
        let (walk, next_state) = attempt(grammar, schema, seed);
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
