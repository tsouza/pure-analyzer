//! Seeded accepting-walk generator for the byte-PDA (`specs/m1-l1-grammar.md` T8,
//! G3; `docs/spec/testing.md` §8.2).
//!
//! The recognizer is `Clone` and leaves itself unchanged on a dead byte
//! ([`Pda::advance`] doc), so the *language* it accepts can be sampled by
//! clone-and-probe: at each position, try every byte of a representative alphabet
//! on a clone, keep the ones the automaton accepts, and pick one with a seeded
//! PRNG. Growing then closing (openers are disabled once a per-walk length target
//! is reached) drives every walk to an accepting configuration in bounded steps.
//!
//! The generator is fully deterministic — committed seeds, a committed
//! SplitMix64 — so a walk set is reproducible in CI (constitution §2, no
//! local-only state) and identical across runs. Its output feeds two lanes: the
//! hermetic `completeness_walks` self-test (every walk must stream through the
//! shipped [`DecoderSession`]) and the opt-in `legend` engine lane.

use purecard::Pda;

/// The representative byte alphabet the walk probes at each position. It is not
/// all 256 bytes: one or two witnesses per character class (identifier, digit,
/// each delimiter, each operator, string/date/var sigils, the `~` relation sigil)
/// reach every arm of the transition function while keeping generated walks short
/// and legible. Extra letters would only lengthen identifiers without exercising a
/// new transition.
const ALPHABET: &[u8] = b"abXY1_ |{}()[].,;:$%'-><=!&+*/~";

/// Number of accepting walks a full generation produces.
pub const WALK_COUNT: usize = 64;

/// Upper bound on generation attempts — a safety valve so a bug can never spin
/// forever. Comfortably above `WALK_COUNT`, since the biased close-out lands an
/// accepting walk on nearly every seed.
const ATTEMPT_LIMIT: usize = WALK_COUNT * 64;

/// The base seed; walk `i` derives from `BASE_SEED` advanced past every seed a
/// prior walk consumed, so the set is one deterministic stream, not 64 correlated
/// low seeds.
const BASE_SEED: u64 = 0x5075_7265_4361_7264; // "PureCard" as ASCII bytes.

/// Shortest accepted walk kept (a bare `|X ` is length 3); below this a walk is
/// too trivial to be worth an engine round-trip.
const MIN_LEN: usize = 4;

/// The per-walk growth target is drawn from `[GROW_MIN, GROW_MAX)`; until the walk
/// reaches it, openers are allowed and the walk explores. Past it, openers are
/// disabled and the walk closes toward an accepting state.
const GROW_MIN: u64 = 6;
const GROW_MAX: u64 = 44;

/// Hard cap on emitted bytes per attempt — a safety bound so a pathological walk
/// terminates rather than spins. Comfortably above `GROW_MAX` plus the deepest
/// close-out any reachable stack needs.
const HARD_CAP: usize = 400;

/// Weight added, in the closing phase, to a byte whose result is an accepting
/// configuration — biases each closing step toward finishing the walk.
const ACCEPT_BONUS: u32 = 10;

/// Sampling weights in the *growing* phase: structure (openers) is modestly
/// favoured over ever-longer identifiers (wordish bytes), with all other bytes
/// weighted the same as an opener.
const OPENER_WEIGHT_GROWING: u32 = 3;
const WORDISH_WEIGHT_GROWING: u32 = 4;
const DEFAULT_WEIGHT_GROWING: u32 = 3;

/// Sampling weights in the *closing* phase: openers are forbidden (they never
/// shrink the stack), closers are strongly favoured so the walk terminates, and
/// everything else keeps a small residual weight.
const OPENER_WEIGHT_CLOSING: u32 = 0;
const CLOSER_WEIGHT_CLOSING: u32 = 12;
const DEFAULT_WEIGHT_CLOSING: u32 = 2;

/// SplitMix64 — a tiny, well-known, fully specified PRNG. Committed here rather
/// than pulling a `rand` dependency for a deterministic test generator: the whole
/// state is one `u64`, so a seed pins the exact stream (constitution §4 — bespoke
/// over a dependency where the bespoke code is trivial and total).
struct SplitMix64 {
    state: u64,
}

/// SplitMix64's golden-ratio increment and two avalanche multipliers (Steele et
/// al., 2014) — named, not magic (constitution §4).
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

    /// A uniform `u64` in `[0, bound)`; `bound` is always a small positive alphabet
    /// or range size here, so a modulo bias is negligible.
    fn below(&mut self, bound: u64) -> u64 {
        self.next_u64() % bound
    }
}

const fn is_opener(byte: u8) -> bool {
    matches!(byte, b'(' | b'[' | b'{')
}

const fn is_closer(byte: u8) -> bool {
    matches!(byte, b')' | b']' | b'}')
}

const fn is_wordish(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// The sampling weight of an enabled `byte`, given whether the walk is still
/// growing and whether taking the byte lands in an accepting state.
fn weight(byte: u8, growing: bool, leads_accept: bool) -> u32 {
    let base = if growing {
        if is_opener(byte) {
            OPENER_WEIGHT_GROWING
        } else if is_wordish(byte) {
            WORDISH_WEIGHT_GROWING
        } else {
            DEFAULT_WEIGHT_GROWING
        }
    } else if is_opener(byte) {
        OPENER_WEIGHT_CLOSING
    } else if is_closer(byte) {
        CLOSER_WEIGHT_CLOSING
    } else {
        DEFAULT_WEIGHT_CLOSING
    };
    if !growing && leads_accept {
        base + ACCEPT_BONUS
    } else {
        base
    }
}

/// Pick an enabled byte by weight. `cands` is non-empty with at least one positive
/// weight whenever this is called.
fn weighted_pick(cands: &[(u8, u32)], rng: &mut SplitMix64) -> u8 {
    let total: u32 = cands.iter().map(|&(_, w)| w).sum();
    let mut target = rng.below(u64::from(total)) as u32;
    for &(byte, w) in cands {
        if target < w {
            return byte;
        }
        target -= w;
    }
    // Unreachable: weights sum to `total` and `target < total`. Fall back to the
    // last candidate rather than panic.
    cands[cands.len() - 1].0
}

/// Attempt one accepting walk from `seed`. Returns the byte string and the PRNG's
/// final state (so the next walk resumes the same SplitMix64 stream), or `None` if
/// the attempt did not reach an accepting state within [`HARD_CAP`]. The grow-target
/// draw always advances the state at least once, so the returned state never equals
/// `seed`.
fn attempt(seed: u64) -> (Option<Vec<u8>>, u64) {
    let mut rng = SplitMix64::new(seed);
    let grow_target = GROW_MIN + rng.below(GROW_MAX - GROW_MIN);
    let mut pda = Pda::new();
    let mut out: Vec<u8> = Vec::new();

    for _ in 0..HARD_CAP {
        let growing = (out.len() as u64) < grow_target;
        if !growing && pda.is_accepting() && out.len() >= MIN_LEN {
            return (Some(out), rng.state);
        }
        let mut cands: Vec<(u8, u32)> = Vec::new();
        for &byte in ALPHABET {
            let mut probe = pda.clone();
            if probe.advance(byte).is_ok() {
                let w = weight(byte, growing, probe.is_accepting());
                if w > 0 {
                    cands.push((byte, w));
                }
            }
        }
        if cands.is_empty() {
            return if pda.is_accepting() && out.len() >= MIN_LEN {
                (Some(out), rng.state)
            } else {
                (None, rng.state)
            };
        }
        let byte = weighted_pick(&cands, &mut rng);
        // The byte was chosen from probed-live candidates, so `advance` is expected
        // to succeed; the `Err` arm is a defensive guard that abandons the attempt
        // rather than trusting the invariant blindly.
        if pda.advance(byte).is_err() {
            return (None, rng.state);
        }
        out.push(byte);
    }
    if pda.is_accepting() && out.len() >= MIN_LEN {
        (Some(out), rng.state)
    } else {
        (None, rng.state)
    }
}

/// Generate exactly [`WALK_COUNT`] deterministic accepting walks. Each attempt
/// resumes the previous one's final PRNG state, so successful and failed attempts
/// alike form one reproducible SplitMix64 stream.
///
/// The count is a guarantee, not a target: the loop runs until `WALK_COUNT` walks
/// are collected, bounded by [`ATTEMPT_LIMIT`] purely so a bug can never spin
/// forever, and a final assertion turns any shortfall into a failure at this source
/// rather than a confusing mismatch downstream.
#[must_use]
pub fn generate_walks() -> Vec<Vec<u8>> {
    let mut walks = Vec::with_capacity(WALK_COUNT);
    let mut seed = BASE_SEED;
    let mut attempts = 0usize;
    while walks.len() < WALK_COUNT && attempts < ATTEMPT_LIMIT {
        attempts += 1;
        let (walk, next_state) = attempt(seed);
        seed = next_state;
        if let Some(bytes) = walk {
            walks.push(bytes);
        }
    }
    assert_eq!(
        walks.len(),
        WALK_COUNT,
        "generate_walks fell short of WALK_COUNT within ATTEMPT_LIMIT attempts"
    );
    walks
}
