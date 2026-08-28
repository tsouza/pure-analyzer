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

use purecard::grammar::pda::{Pda, State};
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
///
/// Widened from the original `[2, 12)` (issue #117): a minimal
/// `Class.all()->filter(x|$x.field == 'literal')` continuation alone needs
/// ~18 tokens (`->`, `filter`, `(`, binder, `|`, `$`, binder-ref, `.`, field
/// name, `==`, literal, `)`, on top of the source/`.all()` prefix), so `12`
/// was mathematically below the shortest real class-navigation predicate —
/// confirmed live: across 512 generated walks (64 × 8 fixture schemas) under
/// the original range, `schema_walk_rule_coverage.rs` found N1/N2/T1/T2/T3/N6
/// never fired even once. `HARD_CAP` leaves `HARD_CAP - GROW_MAX = 24` tokens
/// of closing-phase budget past the new ceiling, comfortably more than a
/// filter clause's own closing sequence needs.
const GROW_MIN: u64 = 20;
const GROW_MAX: u64 = 60;

/// Hard cap on emitted tokens per attempt — a safety bound so a pathological
/// walk terminates rather than spins. Widened alongside `GROW_MAX` (issue
/// #117) so the closing phase past a deep growth target still has a
/// comfortable budget (`HARD_CAP - GROW_MAX = 68` tokens) to actually close
/// out a filter/aggregation clause's own nested brackets.
const HARD_CAP: usize = 128;

/// Weight added, in the closing phase, to a candidate whose result is a
/// completed session — biases each closing step toward finishing the walk.
const ACCEPT_BONUS: u32 = 10;

/// Weight added, at the pipeline-source position only, to a candidate that
/// names a real schema class — biasing exploration toward `Class.all()->…`
/// (arm-C) over the store path's own equally N3-legal but far less
/// grammatically constrained arm-A/relational territory (issue #117).
/// Widening [`GROW_MIN`]/[`GROW_MAX`] alone did not fix #117's finding
/// (confirmed live: even at `GROW_MAX = 60`/`HARD_CAP = 128`, walks reached
/// 127 tokens without ever completing real class-member navigation) — the
/// store path's generic value-expression grammar has a combinatorially larger
/// branching factor than a class's own well-typed navigation, so uniform
/// random selection gets diluted away from it long before growth budget runs
/// out. A store-path walk is still reachable (this only *biases*, unlike
/// `MustOpen`'s hard override), preserving arm-A/relational coverage.
const CLASS_SOURCE_BONUS: u32 = 200;

/// Uniform per-candidate weight outside the accept/class-source bonuses.
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

/// Every keyword name `docs/spec/grammar.md` §5.2–§5.3 (plus its arm-R
/// extension, §7's `relFnGroupBy`/`relProject`/`relSort`/`relExtendWindow`/
/// `relRename`) admits right after a `->` arrow: `step`, `reducer`,
/// `boolPred`, `collapse`, and `fn`'s productions, plus the two
/// `relationalSource` method names and the two nested helper calls
/// (`colRename`'s `pair`, `colDef`'s `col`). Deliberately excludes
/// `tdsGetter`'s names (`getInteger`/…) — `colAccess` dot-calls those, they
/// never follow an arrow.
///
/// Biases [`build_candidates`]'s arrow-method-name choice toward these
/// (issue #117): nothing in this overlay narrows that position at all (it
/// isn't a schema member lookup, so N1/N2/N6 don't apply, and no rule
/// enforces "this identifier names a real Pure builtin" — left to the
/// compiler oracle per the grammar's own over-approximation). Left
/// unconstrained, the position draws uniformly from the *entire* vocabulary,
/// which is dominated by schema property/association/column names (never
/// legal there) — confirmed live: walks like
/// `Db->fk2DefaultCarMakers('avg'==…)` treat an association name as an
/// arrow-called method. A property name is essentially never *also* one of
/// these ~40 fixed keywords, so this bias reliably steers exploration toward
/// a real pipeline step instead.
const ARROW_METHOD_NAMES: &[&str] = &[
    // step (§5.2)
    "filter",
    "project",
    "groupBy",
    "olapGroupBy",
    "restrict",
    "sort",
    "take",
    "distinct",
    "renameColumns",
    "extend",
    "join",
    "limit",
    "agg",
    // nested helper calls (colRename, colDef)
    "pair",
    "col",
    // reducer (§5.3)
    "count",
    "sum",
    "average",
    "min",
    "max",
    "size",
    "rowNumber",
    // boolPred (§5.3)
    "exists",
    "contains",
    "startsWith",
    "endsWith",
    "isEmpty",
    "isNotEmpty",
    "between",
    // collapse (§5.3, T6)
    "toOne",
    // fn (§5.3)
    "parseFloat",
    "parseInteger",
    "toString",
    "toLower",
    "toUpper",
    "substring",
    "year",
    "at",
    "cast",
    "first",
    "concatenate",
    // relationalSource (§5.2)
    "tableReference",
    "tableToTDS",
    // arm-R extension (§7)
    "over",
    "rename",
];

/// Weight added, right after a `->` arrow (`PendingCall::JustArrowed`), to a
/// candidate whose bytes exactly match one of [`ARROW_METHOD_NAMES`] — see
/// that constant's doc comment for why this position needs the bias.
const ARROW_METHOD_BONUS: u32 = 200;

/// Weight added, right after a `$` (`State::AfterDollar`), to a candidate
/// exactly matching `attempt`'s tracked `known_binder` — the identifier a
/// lambda pipe (`x|…`) was most recently opened with. Nothing in this
/// overlay narrows a refVar's own name to the currently-bound identifiers
/// (`$`+any identifier is admissible, real or not — a residue left to the
/// compiler oracle), so without this bias the walker draws the post-`$`
/// identifier independently and uniformly at random, essentially never
/// reusing the exact binder name — meaning `$x.field` almost never actually
/// resolves against the class `x` was bound to, even once `filter(x|…)`
/// itself is reached (issue #117).
const KNOWN_BINDER_BONUS: u32 = 500;

/// Weight added, right after a `$known_binder` reference just completed
/// (`attempt`'s `just_referenced_binder`), to the `.` candidate — continuing
/// straight into member navigation rather than diluting across the many
/// other legal continuations of a completed term (`->`, a comparator,
/// arithmetic, a closer, `&&`/`||`, …). Reaching `$known_binder` at all is
/// already the rarer event this bias chain exists for (issue #117); left
/// unbiased here, that reuse alone still overwhelmingly does *not* continue
/// into `.field` (confirmed live: `$b||…`, `$m>=…` — every legal term
/// continuation competing equally).
const NAVIGATION_DOT_BONUS: u32 = 200;

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

/// Whether `bytes` is shaped like a plain identifier (a legal `binderVar`) —
/// starts with a letter/underscore, every byte is a word byte. Used to
/// recognize a lambda binder name (see `attempt`'s `known_binder` tracking).
fn looks_like_ident(bytes: &[u8]) -> bool {
    matches!(bytes.first(), Some(&b) if b.is_ascii_alphabetic() || b == b'_')
        && bytes.iter().all(|&b| is_word_byte(b))
}

/// Whether the token just accepted (`bytes`) opens the lambda of the
/// identifier before it (`last_token`) — a bare `|` immediately after
/// something identifier-shaped (`binderVar "|" …`, the grammar's only use of
/// a bare `|` past the query-opening one, which has no preceding token to
/// match this) — factored out of `attempt` so the exact condition is directly
/// unit-testable without needing a full grammar/schema/session fixture (see
/// `attempt`'s `known_binder` tracking).
fn opens_binder_lambda(bytes: &[u8], last_token: &[u8]) -> bool {
    bytes == b"|" && looks_like_ident(last_token)
}

/// Whether the token just accepted (`last_token`) is itself a reference to
/// the tracked lambda binder (`known_binder`) — factored out of `attempt` so
/// the exact condition (an empty `last_token` never counts, even against a
/// `known_binder` of `None`) is directly unit-testable (see `attempt`'s
/// `just_referenced_binder`, which biases the *following* step toward `.`).
fn is_binder_reference(last_token: &[u8], known_binder: Option<&[u8]>) -> bool {
    !last_token.is_empty() && known_binder == Some(last_token)
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
    let mut last_token: Vec<u8> = Vec::new();
    let mut known_binder: Option<Vec<u8>> = None;
    let mut seen_arrow_or_dollar = false;
    let mut pending_source_method = false;
    let mut source_method_progress: Vec<u8> = Vec::new();

    for _ in 0..HARD_CAP {
        let growing = is_growing(out.len(), grow_target);
        if !growing && attempt_may_stop(pending_source_method, &pending, &session, out.len()) {
            return (Some(out), rng.state);
        }
        // Whether the token just emitted was itself a reference to the known
        // binder (`$x` completing) — biases the *following* step toward `.`,
        // continuing straight into member navigation (see
        // `build_candidates`'s `JUST_REFERENCED_BINDER_DOT_BONUS`).
        let just_referenced_binder = is_binder_reference(&last_token, known_binder.as_deref());
        let cands = build_candidates(
            &mut session,
            schema,
            vocab,
            &pending,
            growing,
            last_byte,
            known_binder.as_deref(),
            just_referenced_binder,
        );
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
        if opens_binder_lambda(bytes, &last_token) {
            // A `|` right after a plain identifier opens *that* identifier's
            // lambda (`binderVar "|" …` — the grammar's only use of a bare
            // `|` past the query-opening one, which has no preceding token to
            // match this). Remember it so a later `$`-reference can be
            // biased toward reusing it (see `build_candidates`'s
            // `known_binder` — without this, the walker draws the
            // post-`$` identifier independently and uniformly at random,
            // essentially never reusing the exact binder name, so N1/N2
            // member navigation almost never fires despite `filter(x|…)`
            // itself being reached).
            known_binder = Some(last_token.clone());
        }
        last_token = bytes.to_vec();
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
/// `MustOpen` hard override, the fusion exclusion, the source-position
/// class bias, and the closing-phase bias — see [`attempt`]'s docs for what
/// each guards against. Every id here comes from `allowed_mask()`, so a later
/// `accept_token` is guaranteed to succeed (the mask/accept invariant
/// proves) — no per-candidate probe needed to confirm admissibility.
// Each argument is a distinct, documented input to the per-step bias (the
// live session, schema, vocab, in-flight call state, growth phase, fusion
// guard, and the two binder-tracking signals issue #117 added); bundling
// them into a context struct would add indirection to the hot path for no
// clarity gain, so the count is accepted here rather than silenced globally
// (mirrors `pure-analyzer-purecard::schema::narrow::narrow_into`'s identical
// precedent).
#[allow(clippy::too_many_arguments)]
fn build_candidates(
    session: &mut DecoderSession,
    schema: &Schema,
    vocab: &Vocab,
    pending: &PendingCall,
    growing: bool,
    last_byte: Option<u8>,
    known_binder: Option<&[u8]>,
    just_referenced_binder: bool,
) -> Vec<(u32, u32)> {
    let ids: Vec<u32> = session.allowed_mask().iter_ones().collect();
    // `allowed_mask()`'s exclusive borrow of `session` ends with the
    // `collect()` above, so this immutable read is free to follow it.
    let state = session.pda().map(Pda::state);
    let at_source = state == Some(State::ExpectSource);
    let at_dollar = state == Some(State::AfterDollar);
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
        let is_class_source =
            at_source && std::str::from_utf8(bytes).is_ok_and(|text| schema.has_class(text));
        let is_arrow_method = matches!(pending, PendingCall::JustArrowed)
            && std::str::from_utf8(bytes).is_ok_and(|text| ARROW_METHOD_NAMES.contains(&text));
        let is_known_binder_ref = at_dollar && known_binder.is_some_and(|b| b == bytes);
        let is_navigation_dot = just_referenced_binder && bytes == b".";
        let w = if !growing && ends_with_closer(bytes) {
            DEFAULT_WEIGHT + ACCEPT_BONUS
        } else if is_class_source {
            DEFAULT_WEIGHT + CLASS_SOURCE_BONUS
        } else if is_arrow_method {
            DEFAULT_WEIGHT + ARROW_METHOD_BONUS
        } else if is_known_binder_ref {
            DEFAULT_WEIGHT + KNOWN_BINDER_BONUS
        } else if is_navigation_dot {
            DEFAULT_WEIGHT + NAVIGATION_DOT_BONUS
        } else {
            DEFAULT_WEIGHT
        };
        cands.push((id, w));
    }
    cands
}

/// Every Pure primitive type-annotation name a typed reduce-lambda binder may
/// declare (`docs/spec/schema.md` §6.2.2; mirrors the schema crate's own
/// unexposed `PrimName`, which cannot itself cross the crate boundary without
/// growing the public surface for no reason beyond this one lookup) —
/// `recipe_reducer` tries each in turn against the vocabulary, since it
/// cannot assume any one of them appears literally in a given db's
/// corpus-derived vocabulary.
const PRIM_TYPE_NAMES: &[&str] = &[
    "Integer",
    "Number",
    "Float",
    "Decimal",
    "String",
    "Boolean",
    "Date",
    "StrictDate",
    "DateTime",
];

/// Reducer names T3 never masks regardless of the reduce-lambda's declared
/// element type (`min`/`max`/`count` — see [`ARROW_METHOD_NAMES`]'s own
/// reducer entries and `pure-analyzer-purecard::schema::narrow::keeps_reducer`'s
/// corpus evidence) — the only names `recipe_reducer` can pair with an
/// arbitrarily-found [`PRIM_TYPE_NAMES`] entry without risking rejection by
/// T3's own mask (`sum`/`average` are legal only for a numeric element type).
const UNCONSTRAINED_REDUCER_NAMES: &[&str] = &["count", "min", "max"];

/// The vocabulary id of the token spelled exactly `text`, if any.
fn find_token(vocab: &Vocab, text: &[u8]) -> Option<u32> {
    (0..vocab.len() as u32).find(|&id| vocab.bytes(id) == Some(text))
}

/// The vocabulary id of a single-byte ASCII-digit token, if any. A recipe's
/// numeric-literal RHS never needs a *specific* digit, only one admissible
/// under T1's `ReValue(Numeric)` narrowing.
fn find_digit_token(vocab: &Vocab) -> Option<u32> {
    (0..vocab.len() as u32).find(|&id| {
        vocab
            .bytes(id)
            .is_some_and(|b| matches!(b, [d] if d.is_ascii_digit()))
    })
}

/// The vocabulary id of a single-byte ASCII-whitespace token, if any.
///
/// [`recipe_navigation_predicate`] needs one right after the member
/// identifier: an identifier has no self-terminating byte, so closing it
/// with a byte that is itself *semantically loaded* (an ordered comparator
/// like `<`) drives the byte-PDA straight through `AfterValue` into the
/// comparator's own state in one step, without `AfterValue` ever becoming
/// externally observable — confirmed empirically (issue #119) that checking
/// [`DecoderSession::active_l2_position`] right after such a token reads
/// `None`, not [`L2Position::Comparator`](purecard::schema::L2Position::Comparator),
/// even though the comparator itself is correctly narrowed and admitted. An
/// inert whitespace byte closes the identifier without itself opening
/// anything, landing cleanly on the anchor state and letting `Comparator`'s
/// position read through. The same gap applies to reading `ReValue` right
/// after a single-byte ordered comparator (`<`/`>`, as opposed to a
/// multi-byte token like `==`, which is not itself split by this gap) —
/// [`recipe_navigation_predicate`] uses one whitespace token after *both*
/// the member and the comparator for exactly this reason.
fn find_whitespace_token(vocab: &Vocab) -> Option<u32> {
    (0..vocab.len() as u32).find(|&id| {
        vocab
            .bytes(id)
            .is_some_and(|b| matches!(b, [w] if w.is_ascii_whitespace()))
    })
}

/// Every `(class, member)` id pair whose class and member names are both real
/// vocabulary tokens, in vocabulary id order. Not every schema class/property
/// is guaranteed to appear literally in a given db's corpus-derived
/// vocabulary — an arm-A-only corpus may supply none at all (issue #119) — so
/// [`recipe_navigation_predicate`]/`recipe_reducer` try every candidate this
/// returns rather than assume the first is admissible.
///
/// `numeric_only` restricts to members [`Schema::member_is_numeric`] confirms
/// are a numeric primitive — needed by [`recipe_navigation_predicate`]
/// (issue #55): without it, an association end can be picked for its `<`
/// comparator target, which is L1/L2-admissible (T2 only narrows when a
/// primitive navExpr resolved, so a non-primitive member leaves the position
/// unconstrained pass-through) but rejected by the real Legend compiler —
/// confirmed live. `recipe_reducer`'s key-lambda member is never compared,
/// so it passes `false`.
fn class_member_candidates(schema: &Schema, vocab: &Vocab, numeric_only: bool) -> Vec<(u32, u32)> {
    let mut out = Vec::new();
    for class_id in 0..vocab.len() as u32 {
        let Some(class_bytes) = vocab.bytes(class_id) else {
            continue;
        };
        let Ok(class_text) = std::str::from_utf8(class_bytes) else {
            continue;
        };
        if !schema.has_class(class_text) {
            continue;
        }
        for member in schema.member_names(class_text) {
            if numeric_only && !schema.member_is_numeric(class_text, &member) {
                continue;
            }
            if let Some(member_id) = find_token(vocab, member.as_bytes()) {
                out.push((class_id, member_id));
            }
        }
    }
    out
}

/// Try `tokens` against a fresh session over `grammar`/`schema`, returning
/// them back as a walk only if every one is L2-admissible in sequence *and*
/// the session is genuinely complete afterward — the same acceptance bar
/// [`attempt`]'s own random exploration holds every walk to.
///
/// Checks [`allowed_mask`](DecoderSession::allowed_mask) before *every*
/// `accept_token`, rather than trusting `accept_token`'s own `Ok` result:
/// `accept_token` only enforces L1 grammar-legality, not the L2 schema
/// narrow, by design (`session.rs`'s own doc comment: "do not treat
/// `accept_token` as a schema-validation backstop"). A recipe's
/// hand-constructed sequence is exactly the case that doc comment warns
/// about — confirmed live (issue #119): a first version of this function
/// skipped the mask check and let a walk through whose `<` comparator was
/// L1-legal but not L2-admissible for the resolved member's type, caught by
/// `schema_walk_completeness.rs`'s L1/L2 subset proof.
fn try_walk(grammar: &CompiledGrammar, schema: &Schema, tokens: &[u32]) -> Option<Vec<u32>> {
    let Ok(mut session) = DecoderSession::with_schema(grammar, schema.clone()) else {
        return None;
    };
    for &id in tokens {
        if !session.allowed_mask().test(id) {
            return None;
        }
        session.accept_token(id).ok()?;
    }
    session.is_complete().then(|| tokens.to_vec())
}

/// A deterministic, schema-parameterized walk realizing
/// `Class.all()->filter(a|$a.<member> < <digit>)` — the shape that fires N1/N2
/// (`Member`), T2 (`Comparator`), and T1 (`ReValue`) together, none of which
/// issue #117's per-token weight biases alone could reliably reach (reaching
/// `$x.field cmp literal` needs several independent nested-grammar branches
/// to land on the specific path toward navigation in sequence — see that
/// issue and #119 for the full investigation). `None` when this db's
/// vocabulary has no admissible `(class, member)` pair reaching the shape —
/// an arm-A-only corpus with no bare-ident class-member access at all is a
/// documented residual (`schema_walk_rule_coverage.rs`'s `EXPECTED_UNFIRED`),
/// not a bug.
fn recipe_navigation_predicate(
    grammar: &CompiledGrammar,
    schema: &Schema,
    vocab: &Vocab,
) -> Option<Vec<u32>> {
    let pipe = find_token(vocab, b"|")?;
    let dot = find_token(vocab, b".")?;
    let open = find_token(vocab, b"(")?;
    let close = find_token(vocab, b")")?;
    let arrow = find_token(vocab, b"->")?;
    let filter = find_token(vocab, b"filter")?;
    let binder = find_token(vocab, b"a")?;
    let dollar = find_token(vocab, b"$")?;
    let lt = find_token(vocab, b"<")?;
    let digit = find_digit_token(vocab)?;
    let ws = find_whitespace_token(vocab)?;
    let all = find_token(vocab, SOURCE_METHOD.as_bytes())?;
    for (class_id, member_id) in class_member_candidates(schema, vocab, true) {
        let tokens = [
            pipe, class_id, dot, all, open, close, arrow, filter, open, binder, pipe, dollar,
            binder, dot, member_id, ws, lt, ws, digit, close,
        ];
        if let Some(walk) = try_walk(grammar, schema, &tokens) {
            return Some(walk);
        }
    }
    None
}

/// A deterministic, schema-parameterized walk realizing
/// `Class.all()->agg(a|$a.<member>,b:<PrimType>[*]|$b-><reducer>())` — the
/// shape that fires T3 (`Reducer`), which issue #117's per-token weight
/// biases alone could not reliably reach (the walker never got deep enough to
/// exercise an `agg` aggregation at all). Uses a bare `->agg(...)` step
/// rather than wrapping it in `groupBy(~[...], ...)`: `agg` is already one of
/// [`ARROW_METHOD_NAMES`]'s own step names, and none of the 8 fixture
/// corpora use arm-R's `~[...]` column-set syntax at all (confirmed live —
/// `schema_walk_state_coverage.rs`'s `SawTilde` residual), so a recipe built
/// on it would find no vocabulary token for `~` in any of them and never
/// fire at all. `None` when this db's vocabulary has no admissible
/// combination of a real member, a primitive type-annotation name, and an
/// unconstrained reducer name — a documented residual
/// (`schema_walk_rule_coverage.rs`'s `EXPECTED_UNFIRED`), not a bug.
fn recipe_reducer(grammar: &CompiledGrammar, schema: &Schema, vocab: &Vocab) -> Option<Vec<u32>> {
    let pipe = find_token(vocab, b"|")?;
    let dot = find_token(vocab, b".")?;
    let open = find_token(vocab, b"(")?;
    let close = find_token(vocab, b")")?;
    let arrow = find_token(vocab, b"->")?;
    let comma = find_token(vocab, b",")?;
    let colon = find_token(vocab, b":")?;
    let star = find_token(vocab, b"*")?;
    let bopen = find_token(vocab, b"[")?;
    let bclose = find_token(vocab, b"]")?;
    let agg = find_token(vocab, b"agg")?;
    let key_binder = find_token(vocab, b"a")?;
    let val_binder = find_token(vocab, b"b")?;
    let dollar = find_token(vocab, b"$")?;
    let all = find_token(vocab, SOURCE_METHOD.as_bytes())?;
    let candidates = class_member_candidates(schema, vocab, false);

    for &prim_name in PRIM_TYPE_NAMES {
        let Some(prim_id) = find_token(vocab, prim_name.as_bytes()) else {
            continue;
        };
        for &reducer_name in UNCONSTRAINED_REDUCER_NAMES {
            let Some(reducer_id) = find_token(vocab, reducer_name.as_bytes()) else {
                continue;
            };
            for &(class_id, member_id) in &candidates {
                let tokens = [
                    pipe, class_id, dot, all, open, close, arrow, agg, open, key_binder, pipe,
                    dollar, key_binder, dot, member_id, comma, val_binder, colon, prim_id, bopen,
                    star, bclose, pipe, dollar, val_binder, arrow, reducer_id, open, close, close,
                ];
                if let Some(walk) = try_walk(grammar, schema, &tokens) {
                    return Some(walk);
                }
            }
        }
    }
    None
}

/// Every recipe walk (issue #119) a caller tries to include ahead of its
/// random exploration, in order. A recipe that found no admissible
/// vocabulary/schema combination is dropped rather than padded, so a db
/// missing one shape simply gets fewer recipe walks and more random ones,
/// never a placeholder.
///
/// `include_reducer` gates `recipe_reducer` specifically (issue #55):
/// confirmed live against a real PMCD that its bare `->agg(...)` step,
/// while L1/L2-admissible, is not real, compilable Pure on its own (Legend
/// rejects it outside a `groupBy(...)` wrapper this recipe does not build) —
/// [`generate_schema_walks`] still wants it (its own scope is L1/L2 rule
/// coverage, not live compilability, and the recipe does fire T3's
/// `Reducer`), but [`generate_first_complete_schema_walks`] (issue #55's own
/// live-compile-rate target) should not spend one of its walk slots on a
/// construct known not to compile.
fn recipe_walks(
    grammar: &CompiledGrammar,
    schema: &Schema,
    vocab: &Vocab,
    include_reducer: bool,
) -> Vec<Vec<u32>> {
    let mut walks: Vec<Vec<u32>> = recipe_navigation_predicate(grammar, schema, vocab)
        .into_iter()
        .collect();
    if include_reducer {
        walks.extend(recipe_reducer(grammar, schema, vocab));
    }
    walks
}

/// Shared implementation behind both [`generate_schema_walks`] and
/// [`generate_first_complete_schema_walks`]: `recipe_walks`'s deterministic,
/// schema-parameterized walks first (issue #119 — reaching the
/// class-member-navigation shapes those target needs *every* one of several
/// nested-grammar choices to align, which per-token random weighting alone
/// could not reliably reach, per issue #117's investigation), padded out to
/// [`WALK_COUNT`] by `attempt`'s random exploration (seeded from
/// `base_seed`, growing per `grow_target`). Each random attempt resumes the
/// previous one's final PRNG state, so successful and failed attempts alike
/// form one reproducible SplitMix64 stream.
///
/// Recipe walks are included in *both* callers, not only the forced-growth
/// one: a recipe walk is already a minimal, complete construct (issue #55 —
/// `generate_first_complete_schema_walks`'s own eager mode never grows past
/// the first accepting configuration, so it needs realistic *complete*
/// navigation/aggregation shapes offered to it directly, not grown into;
/// nothing about the recipe/eager split makes one recipe walk's shape
/// invalid for the other's proof).
///
/// The count is a guarantee, not a target: the random-exploration loop runs
/// until enough walks are collected to reach [`WALK_COUNT`] in total, bounded
/// by an internal attempt limit purely so a bug can never spin forever, and a
/// final assertion turns any shortfall into a failure at this source rather
/// than a confusing mismatch downstream.
///
/// # Panics
///
/// Panics if fewer than [`WALK_COUNT`] walks are collected in total within
/// the internal attempt limit.
fn walks_with_recipes(
    grammar: &CompiledGrammar,
    schema: &Schema,
    base_seed: u64,
    grow_target: Option<u64>,
    include_reducer: bool,
    label: &str,
) -> Vec<Vec<u32>> {
    let vocab = grammar.vocab();
    let mut walks = recipe_walks(grammar, schema, vocab, include_reducer);
    walks.truncate(WALK_COUNT);
    let target = WALK_COUNT - walks.len();
    let random = collect_walks(target, base_seed, ATTEMPT_LIMIT, label, |seed| {
        attempt(grammar, schema, seed, grow_target)
    });
    walks.extend(random);
    walks
}

/// Generate exactly [`WALK_COUNT`] deterministic accepting walks (as
/// token-id sequences) over `grammar` under `schema`'s L2 overlay, forcing
/// growth to a varied length before closing — see `walks_with_recipes` for
/// the shared recipe-then-random mechanics.
///
/// # Panics
///
/// Panics if fewer than [`WALK_COUNT`] walks are collected in total within
/// the internal attempt limit.
#[must_use]
pub fn generate_schema_walks(grammar: &CompiledGrammar, schema: &Schema) -> Vec<Vec<u32>> {
    walks_with_recipes(
        grammar,
        schema,
        BASE_SEED,
        None,
        true,
        "generate_schema_walks",
    )
}

/// Whether the shared retry loop in [`collect_walks`] should keep going
/// *before* its hard iteration bound is reached: fewer than `target` walks
/// collected so far — factored out so it is directly unit-testable without
/// needing a full grammar/schema fixture.
fn keep_generating(walks_len: usize, target: usize) -> bool {
    walks_len < target
}

/// Shared retry loop behind both [`generate_schema_walks`] and
/// [`generate_first_complete_schema_walks`]: gather exactly `target`
/// accepting walks from `base_seed`'s SplitMix64 stream, retrying (via each
/// call to `attempt_fn`'s own returned next-seed state) up to `attempt_limit`
/// times. `label` names the caller in the panic message on shortfall.
///
/// `target` is a parameter rather than always [`WALK_COUNT`] because
/// [`generate_schema_walks`] (issue #119) fills in its recipe walks first and
/// asks this loop for only the remainder, so the two together still sum to
/// exactly [`WALK_COUNT`].
///
/// The `for` loop's own range is the *unconditional* bound (mirroring
/// [`attempt`]'s `for _ in 0..HARD_CAP`): even a broken [`keep_generating`]
/// can only make this loop run every one of its `attempt_limit` iterations,
/// never more — a hang here would wedge the whole test binary, since Rust's
/// test harness has no per-test timeout.
///
/// `attempt_fn` is injected (rather than calling [`attempt`] directly)
/// purely for testability: a test can pass a trivial, always-failing closure
/// with a small `attempt_limit` to prove the give-up path fires correctly
/// and quickly, without the cost of an unbounded real generation run.
fn collect_walks(
    target: usize,
    base_seed: u64,
    attempt_limit: usize,
    label: &str,
    mut attempt_fn: impl FnMut(u64) -> (Option<Vec<u32>>, u64),
) -> Vec<Vec<u32>> {
    let mut walks = Vec::with_capacity(target);
    let mut seed = base_seed;
    for _ in 0..attempt_limit {
        if !keep_generating(walks.len(), target) {
            break;
        }
        let (walk, next_state) = attempt_fn(seed);
        seed = next_state;
        if let Some(ids) = walk {
            walks.push(ids);
        }
    }
    assert_eq!(
        walks.len(),
        target,
        "{label} fell short of its target walk count within its attempt limit"
    );
    walks
}

/// Distinct from [`BASE_SEED`] so the eager stream below never coincides with
/// the varied-length one.
const EAGER_BASE_SEED: u64 = 0x4561_6765_7257_616B; // "EagerWak" as ASCII bytes.

/// Generate exactly [`WALK_COUNT`] deterministic accepting walks:
/// `recipe_walks`'s walks first (see `walks_with_recipes`), then random
/// exploration that stops at the *first* point the schema-aware session is
/// genuinely complete (`grow_target = 0`, `MIN_LEN = 1`), rather than
/// [`generate_schema_walks`]'s forced further growth.
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
/// the moment `is_complete()` admits EOS and it's sampled, so this random
/// portion is a closer proxy for real decode behavior than
/// [`generate_schema_walks`]'s forced-growth one — though not, on its own,
/// sufficient to reach issue #55's 100% compile-rate target for every
/// construct shape: replaying it live can still surface residue this
/// generator has no way to close (e.g. missing L2 property-narrowing
/// coverage, bare-class-vs-instance-typed navigation). The recipe walks this
/// function now also includes (issue #55) close part of that gap directly:
/// they are already complete, realistic navigation/aggregation constructs
/// the eager random loop essentially never grows into on its own (it stops
/// at the *first* accepting configuration, which is almost always a bare
/// `Class.all()`) — confirmed live to actually compile against a real
/// PMCD (`live_legend_schema_walk_compile.rs`), not merely L1/L2-admissible,
/// for the navigation-predicate shape once its member selection was made
/// type-aware (see `class_member_candidates`'s `numeric_only`).
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
    walks_with_recipes(
        grammar,
        schema,
        EAGER_BASE_SEED,
        Some(0),
        false,
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
        let growing = build_candidates(
            &mut session,
            &schema(),
            vocab,
            &pending,
            true,
            Some(b'('),
            None,
            false,
        );
        assert_eq!(weight_of(&growing, ")"), Some(DEFAULT_WEIGHT));
        assert_eq!(weight_of(&growing, "\n  "), Some(DEFAULT_WEIGHT));

        // Done growing: only the closer `)` gets the bonus; a non-closer
        // candidate (the whitespace token) stays at the default weight.
        let closing = build_candidates(
            &mut session,
            &schema(),
            vocab,
            &pending,
            false,
            Some(b'('),
            None,
            false,
        );
        assert_eq!(
            weight_of(&closing, ")"),
            Some(DEFAULT_WEIGHT + ACCEPT_BONUS)
        );
        assert_eq!(weight_of(&closing, "\n  "), Some(DEFAULT_WEIGHT));
    }

    /// A source-position vocabulary offering both a real schema class (`A`)
    /// and the store's own, equally N3-legal db path (`spider::d::Db`) as
    /// alternatives — lets a test see the class-source bias's contrast
    /// directly (see [`CLASS_SOURCE_BONUS`]).
    fn vocab_with_source_alternatives() -> Vocab {
        let tokens: Vec<Vec<u8>> = ["|", "A", "spider::d::Db"]
            .iter()
            .map(|s| s.as_bytes().to_vec())
            .collect();
        let eos = tokens.len() as u32;
        Vocab::from_byte_tokens(tokens, eos)
    }

    #[test]
    fn build_candidates_biases_a_real_class_over_the_store_path_at_the_source_position() {
        let grammar = CompiledGrammar::compile(vocab_with_source_alternatives());
        let mut session = DecoderSession::with_schema(&grammar, schema()).expect("valid overlay");
        let vocab = grammar.vocab();
        drive(&mut session, vocab, &["|"]);
        let pending = PendingCall::None;

        let weight_of = |cands: &[(u32, u32)], piece: &str| {
            cands
                .iter()
                .find(|&&(id, _)| vocab.bytes(id) == Some(piece.as_bytes()))
                .map(|&(_, w)| w)
        };

        let cands = build_candidates(
            &mut session,
            &schema(),
            vocab,
            &pending,
            true,
            Some(b'|'),
            None,
            false,
        );
        assert_eq!(
            weight_of(&cands, "A"),
            Some(DEFAULT_WEIGHT + CLASS_SOURCE_BONUS)
        );
        assert_eq!(weight_of(&cands, "spider::d::Db"), Some(DEFAULT_WEIGHT));
    }

    /// A post-arrow vocabulary offering both a real Pure builtin
    /// ([`ARROW_METHOD_NAMES`]'s `count`) and an arbitrary identifier
    /// (`zzz`, not a builtin) — both equally admissible right after `->`,
    /// since nothing in the L2 overlay narrows that position (see
    /// [`ARROW_METHOD_NAMES`]'s doc comment).
    fn vocab_with_arrow_alternatives() -> Vocab {
        let tokens: Vec<Vec<u8>> = ["|", "A", ".", "all", "(", ")", "-", ">", "count", "zzz"]
            .iter()
            .map(|s| s.as_bytes().to_vec())
            .collect();
        let eos = tokens.len() as u32;
        Vocab::from_byte_tokens(tokens, eos)
    }

    #[test]
    fn build_candidates_biases_a_known_pure_builtin_name_right_after_an_arrow() {
        let grammar = CompiledGrammar::compile(vocab_with_arrow_alternatives());
        let mut session = DecoderSession::with_schema(&grammar, schema()).expect("valid overlay");
        let vocab = grammar.vocab();
        drive(
            &mut session,
            vocab,
            &["|", "A", ".", "all", "(", ")", "-", ">"],
        );
        let pending = PendingCall::JustArrowed;

        let weight_of = |cands: &[(u32, u32)], piece: &str| {
            cands
                .iter()
                .find(|&&(id, _)| vocab.bytes(id) == Some(piece.as_bytes()))
                .map(|&(_, w)| w)
        };

        let cands = build_candidates(
            &mut session,
            &schema(),
            vocab,
            &pending,
            true,
            Some(b'>'),
            None,
            false,
        );
        assert_eq!(
            weight_of(&cands, "count"),
            Some(DEFAULT_WEIGHT + ARROW_METHOD_BONUS)
        );
        assert_eq!(weight_of(&cands, "zzz"), Some(DEFAULT_WEIGHT));
    }

    /// A vocabulary reaching a `filter(x|$…)` lambda's binder reference, so a
    /// test can drive a real session to [`State::AfterDollar`] and to the
    /// value boundary right after `$x` completes — the two spots
    /// [`KNOWN_BINDER_BONUS`]/[`NAVIGATION_DOT_BONUS`] bias.
    fn vocab_with_dollar_reference() -> Vocab {
        let tokens: Vec<Vec<u8>> = [
            "|", "A", ".", "all", "(", ")", "-", ">", "filter", "x", "$", "y", ".",
        ]
        .iter()
        .map(|s| s.as_bytes().to_vec())
        .collect();
        let eos = tokens.len() as u32;
        Vocab::from_byte_tokens(tokens, eos)
    }

    #[test]
    fn build_candidates_biases_the_known_binder_at_a_dollar_reference() {
        let grammar = CompiledGrammar::compile(vocab_with_dollar_reference());
        let mut session = DecoderSession::with_schema(&grammar, schema()).expect("valid overlay");
        let vocab = grammar.vocab();
        drive(
            &mut session,
            vocab,
            &[
                "|", "A", ".", "all", "(", ")", "-", ">", "filter", "(", "x", "|", "$",
            ],
        );
        let pending = PendingCall::None;

        let weight_of = |cands: &[(u32, u32)], piece: &str| {
            cands
                .iter()
                .find(|&&(id, _)| vocab.bytes(id) == Some(piece.as_bytes()))
                .map(|&(_, w)| w)
        };

        let cands = build_candidates(
            &mut session,
            &schema(),
            vocab,
            &pending,
            true,
            Some(b'$'),
            Some(b"x"),
            false,
        );
        assert_eq!(
            weight_of(&cands, "x"),
            Some(DEFAULT_WEIGHT + KNOWN_BINDER_BONUS)
        );
        assert_eq!(weight_of(&cands, "y"), Some(DEFAULT_WEIGHT));
    }

    #[test]
    fn build_candidates_biases_the_dot_right_after_a_binder_reference_completes() {
        let grammar = CompiledGrammar::compile(vocab_with_dollar_reference());
        let mut session = DecoderSession::with_schema(&grammar, schema()).expect("valid overlay");
        let vocab = grammar.vocab();
        drive(
            &mut session,
            vocab,
            &[
                "|", "A", ".", "all", "(", ")", "-", ">", "filter", "(", "x", "|", "$", "x",
            ],
        );
        let pending = PendingCall::None;

        let weight_of = |cands: &[(u32, u32)], piece: &str| {
            cands
                .iter()
                .find(|&&(id, _)| vocab.bytes(id) == Some(piece.as_bytes()))
                .map(|&(_, w)| w)
        };

        // `just_referenced_binder = true`, passed directly (this signal is
        // computed by `attempt`, not by session state) — biases `.`, an
        // ordinary, already-admissible property-navigation continuation
        // here, over any other legal continuation (`-`, starting a further
        // arrow hop).
        let cands = build_candidates(
            &mut session,
            &schema(),
            vocab,
            &pending,
            true,
            Some(b'x'),
            None,
            true,
        );
        assert_eq!(
            weight_of(&cands, "."),
            Some(DEFAULT_WEIGHT + NAVIGATION_DOT_BONUS)
        );
        assert_eq!(weight_of(&cands, "-"), Some(DEFAULT_WEIGHT));
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

    #[test]
    fn looks_like_ident_requires_a_letter_or_underscore_start_and_all_word_bytes() {
        assert!(looks_like_ident(b"x"));
        assert!(looks_like_ident(b"_x1"));
        assert!(looks_like_ident(b"camelCase2"));
        // Empty: no first byte to check, not identifier-shaped. Also the
        // case that distinguishes the `&&` from a `||`: `bytes.iter().all()`
        // on an empty slice is vacuously `true`, so only the first-byte
        // check being `false` (and `&&` propagating it) keeps this `false`.
        assert!(!looks_like_ident(b""));
        // Digit-first: not a legal `binderVar` start.
        assert!(!looks_like_ident(b"1x"));
        // A non-word byte anywhere disqualifies it, even after a good start.
        assert!(!looks_like_ident(b"x.y"));
    }

    #[test]
    fn opens_binder_lambda_requires_a_bare_pipe_after_an_identifier_shaped_token() {
        // Both halves true: opens.
        assert!(opens_binder_lambda(b"|", b"x"));
        // Right byte, wrong preceding shape: doesn't open.
        assert!(!opens_binder_lambda(b"|", b"1x"));
        // Right preceding shape, wrong byte: doesn't open.
        assert!(!opens_binder_lambda(b".", b"x"));
    }

    #[test]
    fn is_binder_reference_requires_a_nonempty_exact_match() {
        // Exact match against a tracked binder: a reference.
        assert!(is_binder_reference(b"x", Some(b"x")));
        // Different identifier: not a reference.
        assert!(!is_binder_reference(b"y", Some(b"x")));
        // No binder tracked at all: never a reference.
        assert!(!is_binder_reference(b"x", None));
        // An empty `last_token` never counts, even against a `None` binder
        // (mirrors `Some(b"") == None` being trivially false, but pins the
        // intent explicitly rather than relying on that coincidence).
        assert!(!is_binder_reference(b"", None));
        // An empty `last_token` never counts even against an *equal*, also-
        // empty tracked binder — the only case that actually distinguishes
        // the emptiness guard from the equality check (both halves agree
        // whenever `known_binder` isn't itself `Some(b"")`).
        assert!(!is_binder_reference(b"", Some(b"")));
    }

    #[test]
    fn keep_generating_stops_the_instant_the_target_is_reached() {
        assert!(keep_generating(0, 3));
        assert!(keep_generating(2, 3));
        assert!(!keep_generating(3, 3));
    }

    #[test]
    #[should_panic(expected = "fell short of its target walk count")]
    fn collect_walks_gives_up_within_its_attempt_limit_when_it_never_succeeds() {
        // A closure that never produces a walk, with a tiny attempt limit,
        // proves `collect_walks` actually gives up (panics) rather than
        // spinning forever — the property `attempt_limit` exists for, at a
        // scale a test can afford. Safe as a plain `#[should_panic]` (no
        // hang risk even under a broken `keep_generating`/exit condition):
        // `collect_walks`'s `for _ in 0..attempt_limit` is an *unconditional*
        // bound on iterations, exactly like `attempt`'s own `HARD_CAP` loop.
        let _ = collect_walks(3, 0, 5, "test", |seed| (None, seed));
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

    /// A schema with one real, primitive-typed member — [`RECIPE_SAMPLE`]'s
    /// own class `A` (from the outer [`SAMPLE`]) has no properties at all, so
    /// the recipe machinery needs its own fixture with something to navigate.
    const RECIPE_SAMPLE: &str = r#"{
      "db_id": "r", "db_path": "spider::r::Db",
      "classes": { "A": { "simple_name": "A",
        "properties": [
          {"name": "year", "type": {"kind": "primitive", "name": "Integer"}, "mult": {"lower": 1, "upper": 1}},
          {"name": "label", "type": {"kind": "primitive", "name": "String"}, "mult": {"lower": 1, "upper": 1}}
        ],
        "qualified_properties": [], "super_types": [] } },
      "associations": [], "enums": {}
    }"#;

    fn recipe_schema() -> Schema {
        Schema::from_json(RECIPE_SAMPLE).expect("parses")
    }

    /// Every token both recipes need against [`recipe_schema`]: a real class
    /// (`A`), a real member (`year`), and every structural lexeme each
    /// recipe shape requires.
    fn vocab_for_recipes() -> Vocab {
        let tokens: Vec<Vec<u8>> = [
            "|", "A", ".", "all", "(", ")", "->", "filter", "a", "$", "year", "label", "<", " ",
            "1", "agg", "b", ",", ":", "Integer", "[", "*", "]", "count",
        ]
        .iter()
        .map(|s| s.as_bytes().to_vec())
        .collect();
        let eos = tokens.len() as u32;
        Vocab::from_byte_tokens(tokens, eos)
    }

    fn id_of(vocab: &Vocab, text: &str) -> u32 {
        find_token(vocab, text.as_bytes()).unwrap_or_else(|| panic!("token {text:?} not in vocab"))
    }

    #[test]
    fn find_token_locates_the_exact_byte_match_or_none() {
        let vocab = vocab_for_recipes();
        assert_eq!(find_token(&vocab, b"->"), Some(id_of(&vocab, "->")));
        assert_eq!(find_token(&vocab, b"nope"), None);
    }

    #[test]
    fn find_digit_token_locates_a_single_ascii_digit_or_none() {
        let vocab = vocab_for_recipes();
        assert_eq!(find_digit_token(&vocab), Some(id_of(&vocab, "1")));
        let no_digits = Vocab::from_byte_tokens(vec![b"a".to_vec(), b"ab".to_vec()], 2);
        assert_eq!(find_digit_token(&no_digits), None);
    }

    #[test]
    fn find_whitespace_token_locates_a_single_byte_whitespace_or_none() {
        let vocab = vocab_for_recipes();
        assert_eq!(find_whitespace_token(&vocab), Some(id_of(&vocab, " ")));
        let no_ws = Vocab::from_byte_tokens(vec![b"a".to_vec(), b"ab".to_vec()], 2);
        assert_eq!(find_whitespace_token(&no_ws), None);
    }

    #[test]
    fn class_member_candidates_pairs_real_classes_with_members_present_in_vocab() {
        let vocab = vocab_for_recipes();
        let candidates = class_member_candidates(&recipe_schema(), &vocab, false);
        assert_eq!(
            candidates,
            vec![
                (id_of(&vocab, "A"), id_of(&vocab, "year")),
                (id_of(&vocab, "A"), id_of(&vocab, "label")),
            ]
        );
    }

    #[test]
    fn class_member_candidates_excludes_non_numeric_members_when_numeric_only() {
        let vocab = vocab_for_recipes();
        let candidates = class_member_candidates(&recipe_schema(), &vocab, true);
        assert_eq!(
            candidates,
            vec![(id_of(&vocab, "A"), id_of(&vocab, "year"))]
        );
    }

    #[test]
    fn class_member_candidates_is_empty_when_no_class_name_is_a_vocab_token() {
        // "year" is a real member name, but no real class name appears.
        let vocab = Vocab::from_byte_tokens(vec![b"year".to_vec()], 1);
        assert!(class_member_candidates(&recipe_schema(), &vocab, false).is_empty());
    }

    #[test]
    fn class_member_candidates_skips_a_member_absent_from_the_vocab() {
        // "A" is a real class, but its only member ("year") has no token.
        let vocab = Vocab::from_byte_tokens(vec![b"A".to_vec()], 1);
        assert!(class_member_candidates(&recipe_schema(), &vocab, false).is_empty());
    }

    #[test]
    fn try_walk_succeeds_only_when_every_token_is_l2_admissible_and_the_walk_completes() {
        let grammar = CompiledGrammar::compile(vocab_for_recipes());
        let vocab = grammar.vocab();
        let ids: Vec<u32> = ["|", "A", ".", "all", "(", ")"]
            .iter()
            .map(|s| id_of(vocab, s))
            .collect();
        assert_eq!(
            try_walk(&grammar, &recipe_schema(), &ids),
            Some(ids.clone())
        );
        // The same prefix, minus its closing paren: never reaches completion.
        assert_eq!(
            try_walk(&grammar, &recipe_schema(), &ids[..ids.len() - 1]),
            None
        );
    }

    #[test]
    fn try_walk_rejects_a_token_l1_admits_but_the_l2_mask_excludes() {
        // A garbage classpath-shaped identifier: L1 admits any identifier at
        // the source position, but N3's mask only ever admits a real class
        // or the store path (`Schema::has_class`) — `accept_token` alone
        // would not catch this (its own doc comment: it enforces only L1,
        // never the L2 schema mask), which is exactly the root cause a first
        // version of `try_walk` hit live (issue #119): it let an L1-legal,
        // L2-inadmissible token through, caught only by
        // `schema_walk_completeness.rs`'s separate L1/L2 subset proof. This
        // pins the mask-check as a real defense, not vestigial.
        let vocab = Vocab::from_byte_tokens(vec![b"|".to_vec(), b"Nope".to_vec()], 2);
        let grammar = CompiledGrammar::compile(vocab);
        assert_eq!(try_walk(&grammar, &recipe_schema(), &[0, 1]), None);
    }

    #[test]
    fn recipe_navigation_predicate_builds_the_expected_walk_from_a_real_schema() {
        let grammar = CompiledGrammar::compile(vocab_for_recipes());
        let vocab = grammar.vocab();
        let expected: Vec<u32> = [
            "|", "A", ".", "all", "(", ")", "->", "filter", "(", "a", "|", "$", "a", ".", "year",
            " ", "<", " ", "1", ")",
        ]
        .iter()
        .map(|s| id_of(vocab, s))
        .collect();
        assert_eq!(
            recipe_navigation_predicate(&grammar, &recipe_schema(), vocab),
            Some(expected)
        );
    }

    #[test]
    fn recipe_navigation_predicate_is_none_without_a_qualifying_class_member_pair() {
        // Every structural token the shape needs, but no real class or
        // member name at all.
        let tokens: Vec<Vec<u8>> = [
            "|", ".", "all", "(", ")", "->", "filter", "a", "$", "<", " ", "1",
        ]
        .iter()
        .map(|s| s.as_bytes().to_vec())
        .collect();
        let eos = tokens.len() as u32;
        let grammar = CompiledGrammar::compile(Vocab::from_byte_tokens(tokens, eos));
        assert_eq!(
            recipe_navigation_predicate(&grammar, &recipe_schema(), grammar.vocab()),
            None
        );
    }

    #[test]
    fn recipe_navigation_predicate_is_none_when_a_needed_structural_token_is_missing() {
        // No "filter" token anywhere in the vocabulary.
        let tokens: Vec<Vec<u8>> = [
            "|", "A", ".", "all", "(", ")", "->", "year", "$", "<", " ", "1",
        ]
        .iter()
        .map(|s| s.as_bytes().to_vec())
        .collect();
        let eos = tokens.len() as u32;
        let grammar = CompiledGrammar::compile(Vocab::from_byte_tokens(tokens, eos));
        assert_eq!(
            recipe_navigation_predicate(&grammar, &recipe_schema(), grammar.vocab()),
            None
        );
    }

    #[test]
    fn recipe_reducer_builds_the_expected_walk_from_a_real_schema() {
        let grammar = CompiledGrammar::compile(vocab_for_recipes());
        let vocab = grammar.vocab();
        let expected: Vec<u32> = [
            "|", "A", ".", "all", "(", ")", "->", "agg", "(", "a", "|", "$", "a", ".", "year", ",",
            "b", ":", "Integer", "[", "*", "]", "|", "$", "b", "->", "count", "(", ")", ")",
        ]
        .iter()
        .map(|s| id_of(vocab, s))
        .collect();
        assert_eq!(
            recipe_reducer(&grammar, &recipe_schema(), vocab),
            Some(expected)
        );
    }

    #[test]
    fn recipe_reducer_is_none_without_the_agg_step_name_in_vocab() {
        let tokens: Vec<Vec<u8>> = ["|", "A", ".", "all", "(", ")", "->"]
            .iter()
            .map(|s| s.as_bytes().to_vec())
            .collect();
        let eos = tokens.len() as u32;
        let grammar = CompiledGrammar::compile(Vocab::from_byte_tokens(tokens, eos));
        assert_eq!(
            recipe_reducer(&grammar, &recipe_schema(), grammar.vocab()),
            None
        );
    }

    #[test]
    fn recipe_walks_includes_only_the_recipes_that_actually_succeed() {
        // The full vocabulary with include_reducer: both recipes succeed.
        let full = CompiledGrammar::compile(vocab_for_recipes());
        let both = recipe_walks(&full, &recipe_schema(), full.vocab(), true);
        assert_eq!(both.len(), 2);

        // Missing "agg": only the navigation-predicate recipe can succeed
        // regardless of include_reducer.
        let tokens: Vec<Vec<u8>> = [
            "|", "A", ".", "all", "(", ")", "->", "filter", "a", "$", "year", "<", " ", "1",
        ]
        .iter()
        .map(|s| s.as_bytes().to_vec())
        .collect();
        let eos = tokens.len() as u32;
        let partial = CompiledGrammar::compile(Vocab::from_byte_tokens(tokens, eos));
        let one = recipe_walks(&partial, &recipe_schema(), partial.vocab(), true);
        assert_eq!(one.len(), 1);
    }

    #[test]
    fn recipe_walks_excludes_the_reducer_recipe_when_include_reducer_is_false() {
        // The full vocabulary supports both recipes, but include_reducer=false
        // (issue #55: the reducer recipe's bare `->agg(...)` step is
        // L1/L2-admissible but not real, compilable Pure) drops it.
        let full = CompiledGrammar::compile(vocab_for_recipes());
        let only_navigation = recipe_walks(&full, &recipe_schema(), full.vocab(), false);
        assert_eq!(only_navigation.len(), 1);
    }

    #[test]
    fn generate_schema_walks_totals_exactly_walk_count_even_with_recipe_walks_included() {
        let grammar = CompiledGrammar::compile(vocab_for_recipes());
        let walks = generate_schema_walks(&grammar, &recipe_schema());
        assert_eq!(walks.len(), WALK_COUNT);
    }

    #[test]
    fn generate_first_complete_schema_walks_totals_exactly_walk_count_and_includes_the_navigation_recipe()
     {
        let grammar = CompiledGrammar::compile(vocab_for_recipes());
        let walks = generate_first_complete_schema_walks(&grammar, &recipe_schema());
        assert_eq!(walks.len(), WALK_COUNT);
        let vocab = grammar.vocab();
        let expected_recipe: Vec<u32> = [
            "|", "A", ".", "all", "(", ")", "->", "filter", "(", "a", "|", "$", "a", ".", "year",
            " ", "<", " ", "1", ")",
        ]
        .iter()
        .map(|s| id_of(vocab, s))
        .collect();
        assert!(
            walks.contains(&expected_recipe),
            "expected the navigation-predicate recipe walk to be included"
        );
    }
}
