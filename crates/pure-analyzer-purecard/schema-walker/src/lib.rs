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

/// Weight added, right after `$known_binder.` completes (`attempt`'s
/// `just_navigated_from_binder`), to a candidate identifier that
/// [`Schema::member_is_numeric`] confirms is a numeric primitive member of
/// the binder's own source class. Issue #55, live-verified: without this,
/// the member-name position draws uniformly from the whole vocabulary
/// (nothing here narrows to *numeric* members specifically — N1/N2 only
/// requires *any* real member), so a random walk reaching `$x.` still
/// overwhelmingly lands on an association end or non-numeric primitive.
/// Every downstream comparator/reducer recipe this walker's own
/// `recipe_navigation_predicate` needed a *type-aware* `class_member_candidates`
/// filter to get right (issue #55/#122: an association end passed to `<`
/// is L1/L2-admissible but rejected by the real Legend compiler) — this
/// bonus applies that same lesson to random exploration, not just recipes.
const MEMBER_NUMERIC_BONUS: u32 = 200;

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

/// Whether `bytes` names a real schema class — shared by `build_candidates`'s
/// `is_class_source` bias and `attempt`'s `source_class` capture (issue #55),
/// so "is this token a real class path" has one definition instead of two
/// copies drifting apart.
fn is_real_class(schema: &Schema, bytes: &[u8]) -> bool {
    std::str::from_utf8(bytes).is_ok_and(|text| schema.has_class(text))
}

/// Whether the token just accepted (`bytes`) landed at a fresh pipeline
/// source position (`pre_state`, the PDA state *before* this token) and
/// named a real schema class — the moment `attempt`'s `source_class` should
/// be captured. Factored out so the exact condition is directly
/// unit-testable without needing a full grammar/schema/session fixture
/// (mirrors [`navigated_from_binder`]'s established extraction pattern).
fn accepted_real_class_source(pre_state: Option<State>, schema: &Schema, bytes: &[u8]) -> bool {
    pre_state == Some(State::ExpectSource) && is_real_class(schema, bytes)
}

/// Whether the token just accepted (`bytes`) completes `$known_binder.` — the
/// token before it was itself a binder reference (`just_referenced_binder`)
/// and this one is the navigation dot. Factored out of `attempt` so the exact
/// condition is directly unit-testable without needing a full
/// grammar/schema/session fixture (mirrors [`opens_binder_lambda`]'s
/// established extraction pattern). Feeds `attempt`'s
/// `just_navigated_from_binder`, which biases the *following* step's
/// identifier choice toward a numeric member of the binder's source class
/// (issue #55, `MEMBER_NUMERIC_BONUS`).
fn navigated_from_binder(just_referenced_binder: bool, bytes: &[u8]) -> bool {
    just_referenced_binder && bytes == b"."
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
/// A third residue used to be guarded here, by a per-position
/// `pending_source_method` flag: `DecoderSession::is_complete()` was pure
/// `Pda::is_accepting()` — an L1 *lookahead* fact that never consulted the
/// L2-narrowed mask — so any partial identifier was trivially "completable",
/// and a walk could stop at `Class.a` or at a bare `Class.all` with its
/// mandatory parens still owed. That is now fixed **in the decoder** (issue
/// #55 Phase 2, `schema::narrow`): the overlay clears the EOS bit at a trie
/// cursor that has only reached a strict prefix of a legal name, admits
/// nothing but `(` after a resolved niladic method name, and
/// `is_complete()` reads that same verdict — so the walker can trust it here
/// and needs no counterpart of its own.
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
    let mut source_class: Option<Vec<u8>> = None;
    let mut just_navigated_from_binder = false;

    for _ in 0..HARD_CAP {
        let growing = is_growing(out.len(), grow_target);
        if !growing && walk_is_done(&pending, &session, out.len()) {
            return (Some(out), rng.state);
        }
        // The PDA state *before* this step's token lands — read once here so
        // both `build_candidates`'s own `at_source`/`at_dollar` reads and the
        // post-accept `source_class` capture below (which needs the
        // pre-token state, not the post-token one `session.pda()` would give
        // after `accept_token`) agree on the same snapshot.
        let pre_state = session.pda().map(Pda::state);
        // Whether the token just emitted was itself a reference to the known
        // binder (`$x` completing) — biases the *following* step toward `.`,
        // continuing straight into member navigation (see
        // `build_candidates`'s `JUST_REFERENCED_BINDER_DOT_BONUS`).
        let just_referenced_binder = is_binder_reference(&last_token, known_binder.as_deref());
        // `$known_binder.` just completed (issue #55): bias the *following*
        // step's candidate identifier toward a numeric member of the
        // binder's own source class, mirroring `class_member_candidates`'s
        // `numeric_only` filter that `recipe_navigation_predicate` already
        // needed to avoid a live-verified Legend rejection (an association
        // end passed to `<`).
        let member_bias_class = just_navigated_from_binder
            .then_some(source_class.as_deref())
            .flatten();
        let cands = build_candidates(
            &mut session,
            schema,
            vocab,
            &pending,
            growing,
            last_byte,
            known_binder.as_deref(),
            just_referenced_binder,
            member_bias_class,
        );
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
        if accepted_real_class_source(pre_state, schema, bytes) {
            // The source classpath just landed on a real class (as opposed
            // to the store path, also legal here) — remember it so a later
            // `$known_binder.` can bias its member-name choice toward one of
            // *this* class's numeric members (`MEMBER_NUMERIC_BONUS`).
            source_class = Some(bytes.to_vec());
        }
        // `$known_binder.` just completed with *this* token: bias the next
        // step's identifier choice toward a numeric member of `source_class`.
        just_navigated_from_binder = navigated_from_binder(just_referenced_binder, bytes);
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
    member_bias_class: Option<&[u8]>,
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
        let is_class_source = at_source && is_real_class(schema, bytes);
        let is_arrow_method = matches!(pending, PendingCall::JustArrowed)
            && std::str::from_utf8(bytes).is_ok_and(|text| ARROW_METHOD_NAMES.contains(&text));
        let is_known_binder_ref = at_dollar && known_binder.is_some_and(|b| b == bytes);
        let is_navigation_dot = navigated_from_binder(just_referenced_binder, bytes);
        let is_numeric_member = member_bias_class.is_some_and(|class| {
            std::str::from_utf8(class).is_ok_and(|class_text| {
                std::str::from_utf8(bytes)
                    .is_ok_and(|member_text| schema.member_is_numeric(class_text, member_text))
            })
        });
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
        } else if is_numeric_member {
            DEFAULT_WEIGHT + MEMBER_NUMERIC_BONUS
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

/// The vocabulary ids of the first `count` distinct tokens shaped like a
/// complete single-quoted string literal (starts and ends with `'`), in
/// vocabulary id order, or `None` if fewer than `count` exist.
/// [`recipe_groupby`] and [`recipe_groupby_scalar_multi_agg`] each need two
/// such tokens for `groupBy`'s output
/// column-name list — content never matters for L1/L2 admissibility, only
/// the shape, so any two real string-literal tokens from the vocabulary
/// serve equally well as placeholders.
fn find_quoted_string_tokens(vocab: &Vocab, count: usize) -> Option<Vec<u32>> {
    let mut found = Vec::new();
    for id in 0..vocab.len() as u32 {
        if vocab
            .bytes(id)
            .is_some_and(|b| b.len() >= 2 && b.first() == Some(&b'\'') && b.last() == Some(&b'\''))
        {
            found.push(id);
            if found.len() == count {
                return Some(found);
            }
        }
    }
    None
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
/// `Class.all()->groupBy([a|$a.<key>], [agg(a|$a.<val>,
/// b:<PrimType>[*]|$b-><reducer>())], ['<col1>', '<col2>'])` — the shape that
/// fires T3 (`Reducer`), which issue #117's per-token weight biases alone could
/// not reliably reach (the walker never got deep enough to exercise an
/// aggregation at all).
///
/// Identical to [`recipe_groupby`]'s shape but for the reduce lambda's binder,
/// which carries the `: <PrimType>[*]` annotation T3 arms on — so this file
/// states the aggregation shape once, in [`GroupbyTokens::tokens`], and the two
/// recipes differ only in that argument.
///
/// **The bare `->agg(...)` step this used to build is not real Pure** (issue
/// #55, Phase 5). The live engine rejects it — `Country.all()->agg(a|$a.code,
/// b:Integer[*]|$b->count())` → "Can't find variable class for variable 'a'",
/// because `agg`'s every overload wants a `String[1]` or a lambda first, never
/// the `Country[*]` an extent presents — so this recipe was spending a walk slot
/// on a shape the compiler oracle could never accept, and T3's only coverage sat
/// on it. `recipe_groupby`'s own doc comment already recorded that `groupBy(...)`
/// is the wrapper real Pure requires. Live-confirmed on this branch: the
/// annotated form above returns `TabularDataSet`.
///
/// `None` when this db's vocabulary has no admissible combination of a real
/// class with at least one member, a primitive type-annotation name, an
/// unconstrained reducer name, and two distinct string-literal tokens for the
/// output column names — a documented residual
/// (`schema_walk_rule_coverage.rs`'s `EXPECTED_UNFIRED`), not a bug.
fn recipe_reducer(grammar: &CompiledGrammar, schema: &Schema, vocab: &Vocab) -> Option<Vec<u32>> {
    let ids = GroupbyTokens::find(vocab)?;
    let colon = find_token(vocab, b":")?;
    let star = find_token(vocab, b"*")?;
    let cols = find_quoted_string_tokens(vocab, 2)?;
    let (col1, col2) = (cols[0], cols[1]);
    let candidates = class_member_candidates(schema, vocab, false);

    for &prim_name in PRIM_TYPE_NAMES {
        let Some(prim_id) = find_token(vocab, prim_name.as_bytes()) else {
            continue;
        };
        let annotation = ReduceBinderType {
            colon,
            prim: prim_id,
            star,
        };
        for &reducer_name in UNCONSTRAINED_REDUCER_NAMES {
            let Some(reducer_id) = find_token(vocab, reducer_name.as_bytes()) else {
                continue;
            };
            for &(key_class, key_member) in &candidates {
                for &(val_class, val_member) in &candidates {
                    if val_class != key_class {
                        continue;
                    }
                    let agg = AggShape {
                        val_member,
                        reducer_id,
                        binder_type: Some(annotation),
                    };
                    let tokens = ids.tokens(key_class, key_member, agg, (col1, col2));
                    if let Some(walk) = try_walk(grammar, schema, &tokens) {
                        return Some(walk);
                    }
                }
            }
        }
    }
    None
}

/// The `: <PrimType>[*]` annotation a reduce lambda's binder carries when the
/// aggregation walk is built to arm T3 ([`recipe_reducer`]); absent for the
/// plain compile-rate shape ([`recipe_groupby`]). The surrounding `[`/`]` come
/// from [`GroupbyTokens`], which already holds them.
#[derive(Debug, Clone, Copy)]
struct ReduceBinderType {
    colon: u32,
    prim: u32,
    star: u32,
}

/// The `agg(a|$a.<val>, b<annotation>|$b-><reducer>())` argument of a groupBy
/// walk — the only part of [`GroupbyTokens::tokens`]'s shape that differs
/// between the recipes that build one, kept together so the builder takes the
/// aggregation as one thing rather than three loose ids.
#[derive(Debug, Clone, Copy)]
struct AggShape {
    /// The member the aggregation reads (`$a.<val>`).
    val_member: u32,
    /// The reducer the value lambda applies (`$b-><reducer>()`).
    reducer_id: u32,
    /// The reduce binder's type annotation, present only where T3 must be armed.
    binder_type: Option<ReduceBinderType>,
}

/// Structural token ids the `groupBy([a|$a.<key>], [agg(a|$a.<val>,
/// b|$b-><reducer>())], ['<col1>', '<col2>'])` shape needs, gathered once so
/// [`recipe_groupby`] and [`recipe_groupby_having_restrict`] share both the
/// vocabulary lookups and the token-building logic rather than duplicating
/// either.
struct GroupbyTokens {
    pipe: u32,
    dot: u32,
    open: u32,
    close: u32,
    arrow: u32,
    comma: u32,
    bopen: u32,
    bclose: u32,
    group_by: u32,
    agg: u32,
    key_binder: u32,
    val_binder: u32,
    dollar: u32,
    all: u32,
}

impl GroupbyTokens {
    fn find(vocab: &Vocab) -> Option<Self> {
        Some(Self {
            pipe: find_token(vocab, b"|")?,
            dot: find_token(vocab, b".")?,
            open: find_token(vocab, b"(")?,
            close: find_token(vocab, b")")?,
            arrow: find_token(vocab, b"->")?,
            comma: find_token(vocab, b",")?,
            bopen: find_token(vocab, b"[")?,
            bclose: find_token(vocab, b"]")?,
            group_by: find_token(vocab, b"groupBy")?,
            agg: find_token(vocab, b"agg")?,
            key_binder: find_token(vocab, b"a")?,
            val_binder: find_token(vocab, b"b")?,
            dollar: find_token(vocab, b"$")?,
            all: find_token(vocab, SOURCE_METHOD.as_bytes())?,
        })
    }

    /// `|<class>.all()->groupBy([a|$a.<key>], [agg(a|$a.<val>,
    /// b<annotation>|$b-><reducer>())], ['<col1>', '<col2>'])`, where
    /// `<annotation>` is `: <PrimType>[*]` when [`ReduceBinderType`] is supplied
    /// ([`recipe_reducer`], which needs it to arm T3) and empty otherwise
    /// ([`recipe_groupby`]).
    fn tokens(&self, key_class: u32, key_member: u32, agg: AggShape, cols: (u32, u32)) -> Vec<u32> {
        let AggShape {
            val_member,
            reducer_id,
            binder_type,
        } = agg;
        let (col1, col2) = cols;
        let annotation: Vec<u32> = binder_type
            .map(|t| vec![t.colon, t.prim, self.bopen, t.star, self.bclose])
            .unwrap_or_default();
        let mut tokens = vec![
            self.pipe,
            key_class,
            self.dot,
            self.all,
            self.open,
            self.close,
            self.arrow,
            self.group_by,
            self.open,
            self.bopen,
            self.key_binder,
            self.pipe,
            self.dollar,
            self.key_binder,
            self.dot,
            key_member,
            self.bclose,
            self.comma,
            self.bopen,
            self.agg,
            self.open,
            self.key_binder,
            self.pipe,
            self.dollar,
            self.key_binder,
            self.dot,
            val_member,
            self.comma,
            self.val_binder,
        ];
        tokens.extend(annotation);
        tokens.extend([
            self.pipe,
            self.dollar,
            self.val_binder,
            self.arrow,
            reducer_id,
            self.open,
            self.close,
            self.close,
            self.bclose,
            self.comma,
            self.bopen,
            col1,
            self.comma,
            col2,
            self.bclose,
            self.close,
        ]);
        tokens
    }

    /// `->restrict(['<col2>', '<col1>'])` — the just-emitted columns subset
    /// and reordered. N6 narrows `restrict`'s names against the
    /// `RelationScope` the preceding `groupBy` established, so the only
    /// admissible names are that call's own two, which is why this takes no
    /// name of its own. Shared by [`recipe_groupby_restrict`] and
    /// [`HavingRestrictTokens::tail`], whose shapes differ only in what
    /// precedes this identical tail.
    fn restrict_tail(&self, restrict: u32, col1: u32, col2: u32) -> Vec<u32> {
        vec![
            self.arrow,
            restrict,
            self.open,
            self.bopen,
            col2,
            self.comma,
            col1,
            self.bclose,
            self.close,
        ]
    }
}

/// A deterministic, schema-parameterized walk realizing
/// `Class.all()->groupBy([a|$a.<keyMember>], [agg(a|$a.<valMember>,
/// b|$b-><reducer>())], ['<col1>', '<col2>'])` — a real, compilable arm-C
/// class-level aggregation shape (issue #55), confirmed live against a real
/// PMCD (`Country.all()->groupBy([a|$a.continent], [agg(a|$a.population,
/// b|$b->count())], ['continent', 'cnt'])` returns `TabularDataSet`),
/// mirrored from real gold-corpus examples (e.g. `world_1`'s own
/// `dev:792`). Unlike [`recipe_reducer`]'s bare `->agg(...)` step, this
/// wraps it in the `groupBy(...)` real Pure requires.
///
/// Untyped (`b|$b->count()`, no `PrimName` annotation), so it never arms T3
/// and does not fire `Reducer` — its purpose is raising the live compile
/// rate (issue #55), not L2 rule coverage (issue #119's `recipe_reducer`
/// already covers `Reducer`). `None` when this db's vocabulary has no
/// admissible combination of a real class with at least one member, an
/// unconstrained reducer name, and two distinct string-literal tokens for
/// the output column names.
fn recipe_groupby(grammar: &CompiledGrammar, schema: &Schema, vocab: &Vocab) -> Option<Vec<u32>> {
    let ids = GroupbyTokens::find(vocab)?;
    let cols = find_quoted_string_tokens(vocab, 2)?;
    let (col1, col2) = (cols[0], cols[1]);
    let candidates = class_member_candidates(schema, vocab, false);

    for &reducer_name in UNCONSTRAINED_REDUCER_NAMES {
        let Some(reducer_id) = find_token(vocab, reducer_name.as_bytes()) else {
            continue;
        };
        for &(key_class, key_member) in &candidates {
            for &(val_class, val_member) in &candidates {
                if val_class != key_class {
                    continue;
                }
                let tokens = ids.tokens(
                    key_class,
                    key_member,
                    AggShape {
                        val_member,
                        reducer_id,
                        binder_type: None,
                    },
                    (col1, col2),
                );
                if let Some(walk) = try_walk(grammar, schema, &tokens) {
                    return Some(walk);
                }
            }
        }
    }
    None
}

/// Structural token ids the HAVING+restrict tail
/// `->filter(r|$r.getInteger('<col>') > <digit>)->restrict(['<col2>',
/// '<col1>'])` needs, on top of [`GroupbyTokens`]'s groupBy-prefix ids.
struct HavingRestrictTokens {
    having_binder: u32,
    filter: u32,
    get_integer: u32,
    gt: u32,
    restrict: u32,
    digit: u32,
    ws: u32,
}

impl HavingRestrictTokens {
    fn find(vocab: &Vocab) -> Option<Self> {
        Some(Self {
            having_binder: find_token(vocab, b"r")?,
            filter: find_token(vocab, b"filter")?,
            get_integer: find_token(vocab, b"getInteger")?,
            gt: find_token(vocab, b">")?,
            restrict: find_token(vocab, b"restrict")?,
            digit: find_digit_token(vocab)?,
            ws: find_whitespace_token(vocab)?,
        })
    }

    /// `->filter(r|$r.getInteger('<col2>') > <digit>)->restrict(['<col2>',
    /// '<col1>'])`.
    fn tail(&self, groupby: &GroupbyTokens, col1: u32, col2: u32) -> Vec<u32> {
        let mut tokens = vec![
            groupby.arrow,
            self.filter,
            groupby.open,
            self.having_binder,
            groupby.pipe,
            groupby.dollar,
            self.having_binder,
            groupby.dot,
            self.get_integer,
            groupby.open,
            col2,
            groupby.close,
            self.ws,
            self.gt,
            self.ws,
            self.digit,
            groupby.close,
        ];
        tokens.extend(groupby.restrict_tail(self.restrict, col1, col2));
        tokens
    }
}

/// A deterministic, schema-parameterized walk realizing
/// `Class.all()->groupBy([a|$a.<keyMember>], [agg(a|$a.<valMember>,
/// b|$b-><reducer>())], ['<col1>', '<col2>'])->restrict(['<col2>', '<col1>'])`
/// — [`recipe_groupby`]'s shape with a bare restrict tail and *no* HAVING
/// filter between them, which is what distinguishes it from
/// [`recipe_groupby_having_restrict`]: an aggregate a model subsets and
/// reorders without also thresholding it. Mirrored verbatim from a real,
/// Legend-compiled gold example in the sibling `pure-lingua` corpus
/// (`datasets/legacy-trajectories/1.0.0/train/gold_train_v1.jsonl:1375`,
/// `employee_hire_evaluation`):
/// `Employee.all()->groupBy([x|$x.city],[agg(x|$x.employeeId,y|$y->count())],['City','cnt'])->restrict(['cnt','City'])`
/// — including its column *reordering*, which
/// [`GroupbyTokens::restrict_tail`] reproduces.
///
/// Confirmed live against a real PMCD before shipping, per the recipe rule
/// [`recipe_reducer`] documents the hard way — this recipe's *own generated
/// text* (not a hand-written approximation of it) compiles, e.g. `world_1`'s
/// `Country.all()->groupBy([a|$a.code],[agg(a|$a.code,b|$b->count())],['default','country'])->restrict(['country','default'])`
/// returning `meta::pure::tds::TabularDataSet`, for every fixture db that
/// yields any recipe walk at all (`pets_1`, the eighth, yields none from any
/// recipe).
///
/// `None` under exactly [`recipe_groupby`]'s conditions, plus a vocabulary
/// with no `restrict` step-name token.
fn recipe_groupby_restrict(
    grammar: &CompiledGrammar,
    schema: &Schema,
    vocab: &Vocab,
) -> Option<Vec<u32>> {
    let ids = GroupbyTokens::find(vocab)?;
    let restrict = find_token(vocab, b"restrict")?;
    let cols = find_quoted_string_tokens(vocab, 2)?;
    let (col1, col2) = (cols[0], cols[1]);
    let candidates = class_member_candidates(schema, vocab, false);

    for &reducer_name in UNCONSTRAINED_REDUCER_NAMES {
        let Some(reducer_id) = find_token(vocab, reducer_name.as_bytes()) else {
            continue;
        };
        for &(key_class, key_member) in &candidates {
            for &(val_class, val_member) in &candidates {
                if val_class != key_class {
                    continue;
                }
                let mut tokens = ids.tokens(
                    key_class,
                    key_member,
                    AggShape {
                        val_member,
                        reducer_id,
                        binder_type: None,
                    },
                    (col1, col2),
                );
                tokens.extend(ids.restrict_tail(restrict, col1, col2));
                if let Some(walk) = try_walk(grammar, schema, &tokens) {
                    return Some(walk);
                }
            }
        }
    }
    None
}

/// A deterministic, schema-parameterized walk realizing
/// `Class.all()->groupBy([a|$a.<keyMember>], [agg(a|$a.<valMember>,
/// b|$b-><reducer>())], ['<col1>', '<col2>'])->filter(r|$r.getInteger('<col2>')
/// (greater-than) <digit>)->restrict(['<col2>', '<col1>'])` —
/// [`recipe_groupby`]'s shape with the HAVING+restrict tail `world_1`'s own
/// gold-corpus `dev:792` chains onto it, confirmed live against a real PMCD
/// (`Country.all()->groupBy([a|$a.continent], [agg(a|$a.population,
/// b|$b->count())], ['continent', 'cnt'])->filter(r|$r.getInteger('cnt')
/// (greater-than) 2)->restrict(['cnt', 'continent'])` returns
/// `TabularDataSet`). Reuses the aggregation output column (`col2`) as both
/// the `getInteger` lookup and a `restrict` member, matching `dev:792`'s own
/// reuse of `'cnt'` for both.
///
/// `None` when this db's vocabulary has no admissible combination of a real
/// class with at least one member, an unconstrained reducer name, two
/// distinct string-literal tokens for the output column names, the
/// `filter`/`getInteger`/`restrict` step/method names, a digit token for the
/// HAVING threshold, or a whitespace token around the comparator.
fn recipe_groupby_having_restrict(
    grammar: &CompiledGrammar,
    schema: &Schema,
    vocab: &Vocab,
) -> Option<Vec<u32>> {
    let ids = GroupbyTokens::find(vocab)?;
    let having = HavingRestrictTokens::find(vocab)?;
    let cols = find_quoted_string_tokens(vocab, 2)?;
    let (col1, col2) = (cols[0], cols[1]);
    let candidates = class_member_candidates(schema, vocab, false);

    for &reducer_name in UNCONSTRAINED_REDUCER_NAMES {
        let Some(reducer_id) = find_token(vocab, reducer_name.as_bytes()) else {
            continue;
        };
        for &(key_class, key_member) in &candidates {
            for &(val_class, val_member) in &candidates {
                if val_class != key_class {
                    continue;
                }
                let mut tokens = ids.tokens(
                    key_class,
                    key_member,
                    AggShape {
                        val_member,
                        reducer_id,
                        binder_type: None,
                    },
                    (col1, col2),
                );
                tokens.extend(having.tail(&ids, col1, col2));
                if let Some(walk) = try_walk(grammar, schema, &tokens) {
                    return Some(walk);
                }
            }
        }
    }
    None
}

/// Every `(class, member)` pair [`class_member_candidates`] finds whose
/// member additionally resolves as a [`Schema::member_is_string`] `String`
/// primitive — needed by [`recipe_filter_project`]'s `==` comparator, which
/// (like [`recipe_navigation_predicate`]'s `<` before it, issue #122) is
/// L1/L2-admissible against an association end but not real, compilable Pure
/// there (`equal` has no overload comparing a class-typed collection to a
/// `String`).
fn string_member_candidates(schema: &Schema, vocab: &Vocab) -> Vec<(u32, u32)> {
    class_member_candidates(schema, vocab, false)
        .into_iter()
        .filter(|&(class_id, member_id)| {
            let Some(class_text) = vocab
                .bytes(class_id)
                .and_then(|b| std::str::from_utf8(b).ok())
            else {
                return false;
            };
            let Some(member_text) = vocab
                .bytes(member_id)
                .and_then(|b| std::str::from_utf8(b).ok())
            else {
                return false;
            };
            schema.member_is_string(class_text, member_text)
        })
        .collect()
}

/// A deterministic, schema-parameterized walk realizing
/// `Class.all()->filter(a|$a.<stringMember> == '<literal>')
/// ->project([a|$a.<stringMember>], ['<col>'])` — a real, compilable arm-C
/// filter-then-project shape (issue #55), mirrored from a real gold-corpus
/// example (`activity_1`'s own `train_spider:6723`:
/// `Faculty.all()->filter(x|$x.sex == 'F')->project([x|$x.fname, x|$x.lname,
/// x|$x.phone], ['Fname','Lname','phone'])`) and confirmed live against a
/// real PMCD (`Country.all()->filter(a|$a.code == 'GBR')
/// ->project([a|$a.name], ['name'])` returns `TabularDataSet`).
///
/// Uses `==` against a `String` member rather than [`recipe_filter_project`]'s
/// earlier `>`-against-a-numeric-member design (dropped: confirmed against
/// `world_1`'s own corpus-derived vocabulary that none of its numeric members
/// appear there as literal tokens at all, so no numeric-comparator recipe —
/// including the already-shipped [`recipe_navigation_predicate`] — can ever
/// fire for it; `world_1` does supply several `String` members as literal
/// tokens).
///
/// `None` when this db's vocabulary has no admissible combination of a real
/// class with at least one `String` member, the `filter`/`project` step
/// names, two distinct string-literal tokens (one for the comparator's
/// literal, one for the column alias), or a whitespace token around `==`.
fn recipe_filter_project(
    grammar: &CompiledGrammar,
    schema: &Schema,
    vocab: &Vocab,
) -> Option<Vec<u32>> {
    let pipe = find_token(vocab, b"|")?;
    let dot = find_token(vocab, b".")?;
    let open = find_token(vocab, b"(")?;
    let close = find_token(vocab, b")")?;
    let arrow = find_token(vocab, b"->")?;
    let comma = find_token(vocab, b",")?;
    let bopen = find_token(vocab, b"[")?;
    let bclose = find_token(vocab, b"]")?;
    let filter = find_token(vocab, b"filter")?;
    let project = find_token(vocab, b"project")?;
    let binder = find_token(vocab, b"a")?;
    let dollar = find_token(vocab, b"$")?;
    let eq = find_token(vocab, b"==")?;
    let ws = find_whitespace_token(vocab)?;
    let all = find_token(vocab, SOURCE_METHOD.as_bytes())?;
    let cols = find_quoted_string_tokens(vocab, 2)?;
    let (literal, col) = (cols[0], cols[1]);
    let candidates = string_member_candidates(schema, vocab);

    for &(class_id, member_id) in &candidates {
        let tokens = [
            pipe, class_id, dot, all, open, close, arrow, filter, open, binder, pipe, dollar,
            binder, dot, member_id, ws, eq, ws, literal, close, arrow, project, open, bopen,
            binder, pipe, dollar, binder, dot, member_id, bclose, comma, bopen, col, bclose, close,
        ];
        if let Some(walk) = try_walk(grammar, schema, &tokens) {
            return Some(walk);
        }
    }
    None
}

/// A deterministic, schema-parameterized walk realizing
/// `Class.all()->groupBy([], [agg(a|$a.<member>, b|$b-><reducer1>()),
/// agg(a|$a.<member>, b|$b-><reducer2>())], ['<col1>', '<col2>'])` — the
/// scalar (empty-key) multi-metric aggregation shape (issue #55), mirrored
/// from a real, Legend-compiled gold example (`pure-lingua`'s
/// `datasets/legacy-trajectories/1.0.0/train/gold_train_v1.jsonl:85`, db
/// `concert_singer`: `Stadium.all()->groupBy([], [agg(s|$s.capacity,
/// x|$x->average()), agg(s|$s.capacity, y|$y->max())], ['avgCapacity',
/// 'maxCapacity'])`). Confirmed live against a real PMCD — this recipe's own
/// substituted output for `world_1`,
/// `|spider::world_1::model::default::Country.all()->groupBy([],[agg(a|$a.code,
/// b|$b->count()),agg(a|$a.code,b|$b->min())],['default','country'])`, returns
/// `meta::pure::tds::TabularDataSet` (and likewise for every other fixture db
/// whose vocabulary realizes the shape).
///
/// Differs from [`recipe_groupby`] in the two axes that gold example itself
/// varies: an *empty* key list, and *two* `agg(...)` entries rather than one.
/// Both `agg` lambdas reuse the same binders and the same member — each
/// lambda is independently scoped, and the shape's generality comes from
/// needing two distinct [`UNCONSTRAINED_REDUCER_NAMES`] entries rather than
/// two distinct members, which no db's vocabulary is guaranteed to supply.
/// The gold example's `average` is deliberately *not* used: only
/// `count`/`min`/`max` are schema-agnostic (see
/// [`UNCONSTRAINED_REDUCER_NAMES`]).
///
/// `None` when this db's vocabulary has no admissible combination of a real
/// class member, two distinct unconstrained reducer names, and two distinct
/// string-literal tokens for the output column names.
fn recipe_groupby_scalar_multi_agg(
    grammar: &CompiledGrammar,
    schema: &Schema,
    vocab: &Vocab,
) -> Option<Vec<u32>> {
    let pipe = find_token(vocab, b"|")?;
    let dot = find_token(vocab, b".")?;
    let open = find_token(vocab, b"(")?;
    let close = find_token(vocab, b")")?;
    let arrow = find_token(vocab, b"->")?;
    let comma = find_token(vocab, b",")?;
    let bopen = find_token(vocab, b"[")?;
    let bclose = find_token(vocab, b"]")?;
    let group_by = find_token(vocab, b"groupBy")?;
    let agg = find_token(vocab, b"agg")?;
    let key_binder = find_token(vocab, b"a")?;
    let val_binder = find_token(vocab, b"b")?;
    let dollar = find_token(vocab, b"$")?;
    let all = find_token(vocab, SOURCE_METHOD.as_bytes())?;
    let cols = find_quoted_string_tokens(vocab, 2)?;
    let (col1, col2) = (cols[0], cols[1]);
    let candidates = class_member_candidates(schema, vocab, false);

    for (first_name, second_name) in reducer_name_pairs() {
        let (Some(first_id), Some(second_id)) = (
            find_token(vocab, first_name.as_bytes()),
            find_token(vocab, second_name.as_bytes()),
        ) else {
            continue;
        };
        for &(class_id, member_id) in &candidates {
            let tokens = [
                pipe, class_id, dot, all, open, close, arrow, group_by, open, bopen, bclose, comma,
                bopen, agg, open, key_binder, pipe, dollar, key_binder, dot, member_id, comma,
                val_binder, pipe, dollar, val_binder, arrow, first_id, open, close, close, comma,
                agg, open, key_binder, pipe, dollar, key_binder, dot, member_id, comma, val_binder,
                pipe, dollar, val_binder, arrow, second_id, open, close, close, bclose, comma,
                bopen, col1, comma, col2, bclose, close,
            ];
            if let Some(walk) = try_walk(grammar, schema, &tokens) {
                return Some(walk);
            }
        }
    }
    None
}

/// Every unordered pair of *distinct* [`UNCONSTRAINED_REDUCER_NAMES`]
/// entries, in list order — the reducer choices
/// [`recipe_groupby_scalar_multi_agg`]'s two aggregations try in turn. Two
/// distinct names are what make the shape a genuine *multi*-metric
/// aggregation; a db whose vocabulary supplies fewer than two of the three
/// cannot realize it at all.
fn reducer_name_pairs() -> impl Iterator<Item = (&'static str, &'static str)> {
    UNCONSTRAINED_REDUCER_NAMES
        .iter()
        .enumerate()
        .flat_map(|(index, &first)| {
            UNCONSTRAINED_REDUCER_NAMES[index + 1..]
                .iter()
                .map(move |&second| (first, second))
        })
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
    let mut walks: Vec<Vec<u32>> = [
        recipe_navigation_predicate(grammar, schema, vocab),
        recipe_groupby(grammar, schema, vocab),
        recipe_groupby_scalar_multi_agg(grammar, schema, vocab),
        recipe_groupby_restrict(grammar, schema, vocab),
        recipe_groupby_having_restrict(grammar, schema, vocab),
        recipe_filter_project(grammar, schema, vocab),
    ]
    .into_iter()
    .flatten()
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
) -> SchemaWalkSet {
    let vocab = grammar.vocab();
    let mut walks = recipe_walks(grammar, schema, vocab, include_reducer);
    walks.truncate(WALK_COUNT);
    let recipe_len = walks.len();
    let target = WALK_COUNT - recipe_len;
    let random = collect_walks(target, base_seed, ATTEMPT_LIMIT, label, |seed| {
        attempt(grammar, schema, seed, grow_target)
    });
    walks.extend(random);
    SchemaWalkSet { walks, recipe_len }
}

/// A generated walk set together with the boundary between its two
/// partitions: `recipe_walks`'s deterministic, schema-parameterized
/// constructs first, then the random exploration that pads the set out to
/// [`WALK_COUNT`].
///
/// Issue #55's live-compile-rate lane must report and gate the two
/// separately — a recipe walk is compile-by-construction, so counting one as
/// evidence that a decoder rule improved precision would be circular. The
/// boundary is a positional fact of how `walks_with_recipes` assembles the
/// set, so it is carried out in the return value rather than recomputed by
/// each consumer, which could silently drift from the assembly order.
#[derive(Debug)]
pub struct SchemaWalkSet {
    walks: Vec<Vec<u32>>,
    recipe_len: usize,
}

impl SchemaWalkSet {
    /// Every walk, recipe partition first — exactly the sequence
    /// [`generate_first_complete_schema_walks`] returns.
    #[must_use]
    pub fn walks(&self) -> &[Vec<u32>] {
        &self.walks
    }

    /// How many leading [`walks`](Self::walks) belong to the recipe
    /// partition; the rest are exploration walks. Split a set with
    /// `walks().split_at(recipe_len())`.
    #[must_use]
    pub fn recipe_len(&self) -> usize {
        self.recipe_len
    }
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
    .walks
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
    generate_first_complete_schema_walk_set(grammar, schema).walks
}

/// [`generate_first_complete_schema_walks`]'s walks with their recipe /
/// exploration partition boundary attached — the form issue #55's
/// live-compile-rate lane needs, since it reports and gates the two
/// partitions separately (a recipe walk compiles by construction and is
/// therefore never evidence of decoder precision).
///
/// # Panics
///
/// Panics if fewer than [`WALK_COUNT`] walks are collected within the
/// internal attempt limit.
#[must_use]
pub fn generate_first_complete_schema_walk_set(
    grammar: &CompiledGrammar,
    schema: &Schema,
) -> SchemaWalkSet {
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
            None,
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
            None,
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
            None,
        );
        assert_eq!(
            weight_of(&cands, "A"),
            Some(DEFAULT_WEIGHT + CLASS_SOURCE_BONUS)
        );
        assert_eq!(weight_of(&cands, "spider::d::Db"), Some(DEFAULT_WEIGHT));
    }

    /// A post-arrow vocabulary offering a real Pure builtin
    /// ([`ARROW_METHOD_NAMES`]'s `count`), an arbitrary identifier (`zzz`, not a
    /// builtin), and a builtin the L2 overlay denies on a class extent
    /// (`pair` — N3f, issue #55 Phase 5). All three are admissible *as a name*
    /// right after `->`: N3f clears a denied name at the token that closes its
    /// lexeme, not at its first byte, so the bias contrast is read at the name
    /// and the denial at the call's `(`.
    fn vocab_with_arrow_alternatives() -> Vocab {
        let tokens: Vec<Vec<u8>> = [
            "|", "A", ".", "all", "(", ")", "-", ">", "count", "zzz", "pair",
        ]
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
            None,
        );
        assert_eq!(
            weight_of(&cands, "count"),
            Some(DEFAULT_WEIGHT + ARROW_METHOD_BONUS)
        );
        assert_eq!(weight_of(&cands, "zzz"), Some(DEFAULT_WEIGHT));
        // N3f denies `pair` on a class extent but only at the token that closes
        // the name, so the bias still sees it here — the contrast this test
        // asserts is a *weighting* one and must not quietly become a masking one.
        assert_eq!(
            weight_of(&cands, "pair"),
            Some(DEFAULT_WEIGHT + ARROW_METHOD_BONUS)
        );

        // …and one token later the denial is what the walker sees: the call `(`
        // every method name owes is gone for `pair` and present for `count`.
        for (method, call_is_offered) in [("pair", false), ("count", true)] {
            let mut session =
                DecoderSession::with_schema(&grammar, schema()).expect("valid overlay");
            drive(
                &mut session,
                vocab,
                &["|", "A", ".", "all", "(", ")", "-", ">", method],
            );
            let cands = build_candidates(
                &mut session,
                &schema(),
                vocab,
                &PendingCall::MustOpen,
                true,
                method.as_bytes().last().copied(),
                None,
                false,
                None,
            );
            assert_eq!(
                weight_of(&cands, "(").is_some(),
                call_is_offered,
                "the call `(` after `->{method}`"
            );
        }
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

    /// The binder bias must single out the *current* binder among the variables
    /// that are legal here — so the stream binds **two** (`x` in the first
    /// lambda, `y` in the second) and the assertion contrasts them.
    ///
    /// A one-binder stream cannot test this any more: S2
    /// (`L2Position::RefVar`) clears every name the stream never bound, so the
    /// lone binder would be the only candidate left and *any* weighting would
    /// pass — including `is_known_binder_ref`'s `&&` degrading to `||`, which
    /// makes every candidate at a `$` read as the known binder. Two bound names
    /// keep that mutation observable.
    #[test]
    fn build_candidates_biases_the_known_binder_at_a_dollar_reference() {
        let grammar = CompiledGrammar::compile(vocab_with_dollar_reference());
        let mut session = DecoderSession::with_schema(&grammar, schema()).expect("valid overlay");
        let vocab = grammar.vocab();
        drive(
            &mut session,
            vocab,
            &[
                "|", "A", ".", "all", "(", ")", "-", ">", "filter", "(", "x", "|", "$", "x", ")",
                "-", ">", "filter", "(", "y", "|", "$",
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
            Some(b"y"),
            false,
            None,
        );
        // The binder this lambda actually declared carries the bonus…
        assert_eq!(
            weight_of(&cands, "y"),
            Some(DEFAULT_WEIGHT + KNOWN_BINDER_BONUS)
        );
        // …while `x` — bound by the *enclosing* lambda, so still admissible under
        // S2's monotonic binder record — carries only the default. This is the
        // pair that makes the bias, not merely the mask, the thing under test.
        assert_eq!(weight_of(&cands, "x"), Some(DEFAULT_WEIGHT));
    }

    /// S2's own effect on the candidate set, kept as its own case now that the
    /// bias test above needs two bound names: an identifier L1 admits after `$`
    /// but that nothing in the stream ever bound is gone before the walker
    /// weighs it. Emitting `$y` here is precisely the unbound-refVar failure the
    /// live engine rejects with "Can't find variable class for variable 'y' in
    /// the graph".
    #[test]
    fn build_candidates_drops_an_unbound_variable_at_a_dollar_reference() {
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
        let cands = build_candidates(
            &mut session,
            &schema(),
            vocab,
            &PendingCall::None,
            true,
            Some(b'$'),
            Some(b"x"),
            false,
            None,
        );
        let has = |piece: &str| {
            cands
                .iter()
                .any(|&(id, _)| vocab.bytes(id) == Some(piece.as_bytes()))
        };
        assert!(has("x"), "the bound binder stays admissible");
        assert!(!has("y"), "an unbound name is masked by S2");
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
            None,
        );
        assert_eq!(
            weight_of(&cands, "."),
            Some(DEFAULT_WEIGHT + NAVIGATION_DOT_BONUS)
        );
        assert_eq!(weight_of(&cands, "-"), Some(DEFAULT_WEIGHT));
    }

    /// A schema whose class `A` has one numeric and one non-numeric member,
    /// both real, so a test can see [`MEMBER_NUMERIC_BONUS`]'s contrast
    /// directly.
    const NUMERIC_MEMBER_SAMPLE: &str = r#"{
      "db_id": "d", "db_path": "spider::d::Db",
      "classes": { "A": { "simple_name": "A",
        "properties": [
          {"name": "n", "type": {"kind": "primitive", "name": "Integer"}, "mult": {"lower": 1, "upper": 1}},
          {"name": "label", "type": {"kind": "primitive", "name": "String"}, "mult": {"lower": 1, "upper": 1}}
        ],
        "qualified_properties": [], "super_types": [] } },
      "associations": [], "enums": {}
    }"#;

    fn numeric_member_schema() -> Schema {
        Schema::from_json(NUMERIC_MEMBER_SAMPLE).expect("parses")
    }

    fn vocab_with_numeric_and_non_numeric_members() -> Vocab {
        let tokens: Vec<Vec<u8>> = [
            "|", "A", ".", "all", "(", ")", "-", ">", "filter", "x", "$", "n", "label",
        ]
        .iter()
        .map(|s| s.as_bytes().to_vec())
        .collect();
        let eos = tokens.len() as u32;
        Vocab::from_byte_tokens(tokens, eos)
    }

    #[test]
    fn build_candidates_biases_a_numeric_member_right_after_a_binder_navigation_dot() {
        let grammar = CompiledGrammar::compile(vocab_with_numeric_and_non_numeric_members());
        let mut session =
            DecoderSession::with_schema(&grammar, numeric_member_schema()).expect("valid overlay");
        let vocab = grammar.vocab();
        drive(
            &mut session,
            vocab,
            &[
                "|", "A", ".", "all", "(", ")", "-", ">", "filter", "(", "x", "|", "$", "x", ".",
            ],
        );
        let pending = PendingCall::None;

        let weight_of = |cands: &[(u32, u32)], piece: &str| {
            cands
                .iter()
                .find(|&&(id, _)| vocab.bytes(id) == Some(piece.as_bytes()))
                .map(|&(_, w)| w)
        };

        // `member_bias_class = Some(b"A")`, passed directly (this signal is
        // computed by `attempt`, not by session state) — biases a numeric
        // member of `A` over a non-numeric one, at the exact position a
        // `$known_binder.` navigation's member-name choice happens.
        let cands = build_candidates(
            &mut session,
            &numeric_member_schema(),
            vocab,
            &pending,
            true,
            Some(b'.'),
            None,
            false,
            Some(b"A"),
        );
        assert_eq!(
            weight_of(&cands, "n"),
            Some(DEFAULT_WEIGHT + MEMBER_NUMERIC_BONUS)
        );
        assert_eq!(weight_of(&cands, "label"), Some(DEFAULT_WEIGHT));
    }

    #[test]
    fn build_candidates_does_not_bias_a_numeric_member_without_navigation_from_a_binder() {
        // Same vocabulary/position, but `member_bias_class = None` (the
        // caller determined this step did *not* follow `$known_binder.`) —
        // the numeric-member bonus must not fire regardless of the schema.
        let grammar = CompiledGrammar::compile(vocab_with_numeric_and_non_numeric_members());
        let mut session =
            DecoderSession::with_schema(&grammar, numeric_member_schema()).expect("valid overlay");
        let vocab = grammar.vocab();
        drive(
            &mut session,
            vocab,
            &[
                "|", "A", ".", "all", "(", ")", "-", ">", "filter", "(", "x", "|", "$", "x", ".",
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
            &numeric_member_schema(),
            vocab,
            &pending,
            true,
            Some(b'.'),
            None,
            false,
            None,
        );
        assert_eq!(weight_of(&cands, "n"), Some(DEFAULT_WEIGHT));
        assert_eq!(weight_of(&cands, "label"), Some(DEFAULT_WEIGHT));
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
    fn walk_is_done_trusts_a_mask_aware_session_and_still_owes_an_arrow_call() {
        // A hand-driven session (rather than a specific seed happening to
        // land in a specific state) makes each half of `walk_is_done`
        // directly controllable and deterministic.
        let grammar = CompiledGrammar::compile(vocab_with_source_method_ambiguity());
        let mut session = DecoderSession::with_schema(&grammar, schema()).expect("valid overlay");
        let vocab = grammar.vocab();
        drive(&mut session, vocab, &["|", "A", ".", "all", "(", ")"]);
        assert!(session.is_complete());
        assert!(walk_is_done(&PendingCall::None, &session, 1));

        // A `->name` hop still owes its mandatory `(` — the one completion
        // fact the L1 PDA genuinely cannot express, so `PendingCall` stays.
        assert!(!walk_is_done(&PendingCall::MustOpen, &session, 1));
    }

    /// The two stops the retired `pending_source_method` walker flag used to
    /// forbid, now refused by the decoder's own mask-aware completion (issue
    /// #55 Phase 2) — asserted here, at the walker's own consumer boundary, so
    /// retiring that flag cannot silently re-open either escape.
    #[test]
    fn a_source_method_prefix_or_an_uncalled_source_method_is_never_complete() {
        let grammar = CompiledGrammar::compile(vocab_with_source_method_ambiguity());
        let vocab = grammar.vocab();

        // `|A.a` — a strict byte-prefix of `all`, L1-accepting (`InIdent` over
        // an empty stack), and rejected live (`can't find property 'a'`).
        let mut prefix = DecoderSession::with_schema(&grammar, schema()).expect("valid overlay");
        drive(&mut prefix, vocab, &["|", "A", ".", "a"]);
        assert!(!prefix.is_complete());
        assert!(!walk_is_done(&PendingCall::None, &prefix, 1));

        // `|A.all` — a whole name, but a niladic call whose parens are owed.
        let mut uncalled = DecoderSession::with_schema(&grammar, schema()).expect("valid overlay");
        drive(&mut uncalled, vocab, &["|", "A", ".", "all"]);
        assert!(!uncalled.is_complete());
        assert!(!walk_is_done(&PendingCall::None, &uncalled, 1));

        // …and the counterfactual, so this cannot pass by masking everything:
        // the same source, properly called, *is* complete.
        drive(&mut uncalled, vocab, &["(", ")"]);
        assert!(uncalled.is_complete());
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
    fn is_real_class_matches_only_a_name_the_schema_actually_has() {
        let s = schema();
        assert!(is_real_class(&s, b"A"));
        assert!(!is_real_class(&s, b"Nope"));
        // Non-UTF-8 bytes never match, rather than panicking.
        assert!(!is_real_class(&s, &[0xFF, 0xFE]));
    }

    #[test]
    fn accepted_real_class_source_requires_both_the_source_position_and_a_real_class() {
        let s = schema();
        // Both halves true: a real class landed at the source position.
        assert!(accepted_real_class_source(
            Some(State::ExpectSource),
            &s,
            b"A"
        ));
        // Right position, but not a real class name (e.g. the store path).
        assert!(!accepted_real_class_source(
            Some(State::ExpectSource),
            &s,
            b"spider::d::Db"
        ));
        // A real class name, but not at the source position.
        assert!(!accepted_real_class_source(
            Some(State::AfterValue),
            &s,
            b"A"
        ));
        // Neither.
        assert!(!accepted_real_class_source(None, &s, b"A"));
    }

    #[test]
    fn navigated_from_binder_requires_both_the_prior_reference_and_a_dot() {
        // Both halves true: just navigated from the binder.
        assert!(navigated_from_binder(true, b"."));
        // The prior token wasn't a binder reference: no navigation.
        assert!(!navigated_from_binder(false, b"."));
        // The prior token was a reference, but this one isn't a dot.
        assert!(!navigated_from_binder(true, b"-"));
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

    /// Every token every recipe needs against [`recipe_schema`]: a real class
    /// (`A`), a real member (`year`), and every structural lexeme each
    /// recipe shape requires. Two distinct
    /// [`UNCONSTRAINED_REDUCER_NAMES`] entries (`count`, `min`) are present
    /// because [`recipe_groupby_scalar_multi_agg`] needs a *pair*; the
    /// single-reducer recipes are unaffected, since both scan
    /// [`UNCONSTRAINED_REDUCER_NAMES`] in list order and still settle on
    /// `count`.
    fn vocab_for_recipes() -> Vocab {
        let tokens: Vec<Vec<u8>> = [
            "|",
            "A",
            ".",
            "all",
            "(",
            ")",
            "->",
            "filter",
            "a",
            "$",
            "year",
            "label",
            "<",
            " ",
            "1",
            "agg",
            "b",
            ",",
            ":",
            "Integer",
            "[",
            "*",
            "]",
            "count",
            "min",
            "groupBy",
            "'col1'",
            "'col2'",
            "r",
            "getInteger",
            ">",
            "restrict",
            "project",
            "==",
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
    fn find_quoted_string_tokens_locates_n_distinct_tokens_or_none() {
        let vocab = vocab_for_recipes();
        assert_eq!(
            find_quoted_string_tokens(&vocab, 2),
            Some(vec![id_of(&vocab, "'col1'"), id_of(&vocab, "'col2'")])
        );
        // Only one quoted-string token exists: asking for two fails entirely,
        // even though one alone would succeed.
        let one_quote = Vocab::from_byte_tokens(vec![b"'x'".to_vec()], 1);
        assert_eq!(find_quoted_string_tokens(&one_quote, 2), None);
        assert_eq!(
            find_quoted_string_tokens(&one_quote, 1),
            Some(vec![id_of(&one_quote, "'x'")])
        );
        // A single quote byte alone isn't a *complete* string literal shape.
        let bare_quote = Vocab::from_byte_tokens(vec![b"'".to_vec()], 1);
        assert_eq!(find_quoted_string_tokens(&bare_quote, 1), None);
        // An empty quoted string (exactly the two quote bytes) is the
        // shortest *complete* shape and does count.
        let empty_quote = Vocab::from_byte_tokens(vec![b"''".to_vec()], 1);
        assert_eq!(
            find_quoted_string_tokens(&empty_quote, 1),
            Some(vec![id_of(&empty_quote, "''")])
        );
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
            "|", "A", ".", "all", "(", ")", "->", "groupBy", "(", "[", "a", "|", "$", "a", ".",
            "year", "]", ",", "[", "agg", "(", "a", "|", "$", "a", ".", "year", ",", "b", ":",
            "Integer", "[", "*", "]", "|", "$", "b", "->", "count", "(", ")", ")", "]", ",", "[",
            "'col1'", ",", "'col2'", "]", ")",
        ]
        .iter()
        .map(|s| id_of(vocab, s))
        .collect();
        assert_eq!(
            recipe_reducer(&grammar, &recipe_schema(), vocab),
            Some(expected)
        );
    }

    /// The T3 recipe is [`recipe_groupby`]'s shape plus exactly one thing: the
    /// reduce binder's `: <PrimType>[*]` annotation. Pinning the *difference*
    /// keeps the two from drifting into separately-maintained token lists, which
    /// is what let the bare `->agg(...)` shape survive as long as it did.
    #[test]
    fn recipe_reducer_is_the_groupby_recipe_plus_the_reduce_binder_annotation() {
        let grammar = CompiledGrammar::compile(vocab_for_recipes());
        let vocab = grammar.vocab();
        let plain = recipe_groupby(&grammar, &recipe_schema(), vocab).expect("realizable");
        let annotated = recipe_reducer(&grammar, &recipe_schema(), vocab).expect("realizable");
        let annotation: Vec<u32> = [":", "Integer", "[", "*", "]"]
            .iter()
            .map(|s| id_of(vocab, s))
            .collect();
        let binder = id_of(vocab, "b");
        let at = plain
            .iter()
            .position(|&id| id == binder)
            .expect("the reduce binder is in the plain shape");
        let mut expected = plain;
        expected.splice(at + 1..at + 1, annotation);
        assert_eq!(annotated, expected);
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
    fn recipe_groupby_builds_the_expected_walk_from_a_real_schema() {
        let grammar = CompiledGrammar::compile(vocab_for_recipes());
        let vocab = grammar.vocab();
        let expected: Vec<u32> = [
            "|", "A", ".", "all", "(", ")", "->", "groupBy", "(", "[", "a", "|", "$", "a", ".",
            "year", "]", ",", "[", "agg", "(", "a", "|", "$", "a", ".", "year", ",", "b", "|", "$",
            "b", "->", "count", "(", ")", ")", "]", ",", "[", "'col1'", ",", "'col2'", "]", ")",
        ]
        .iter()
        .map(|s| id_of(vocab, s))
        .collect();
        assert_eq!(
            recipe_groupby(&grammar, &recipe_schema(), vocab),
            Some(expected)
        );
    }

    #[test]
    fn recipe_groupby_is_none_without_the_groupby_step_name_in_vocab() {
        let tokens: Vec<Vec<u8>> = [
            "|", "A", ".", "all", "(", ")", "->", "agg", "a", "b", "$", "year", "count", "[", "]",
            ",", "'col1'", "'col2'",
        ]
        .iter()
        .map(|s| s.as_bytes().to_vec())
        .collect();
        let eos = tokens.len() as u32;
        let grammar = CompiledGrammar::compile(Vocab::from_byte_tokens(tokens, eos));
        assert_eq!(
            recipe_groupby(&grammar, &recipe_schema(), grammar.vocab()),
            None
        );
    }

    #[test]
    fn recipe_groupby_is_none_without_two_distinct_quoted_string_tokens() {
        let tokens: Vec<Vec<u8>> = [
            "|", "A", ".", "all", "(", ")", "->", "groupBy", "agg", "a", "b", "$", "year", "count",
            "[", "]", ",",
        ]
        .iter()
        .map(|s| s.as_bytes().to_vec())
        .collect();
        let eos = tokens.len() as u32;
        let grammar = CompiledGrammar::compile(Vocab::from_byte_tokens(tokens, eos));
        assert_eq!(
            recipe_groupby(&grammar, &recipe_schema(), grammar.vocab()),
            None
        );
    }

    #[test]
    fn reducer_name_pairs_enumerates_every_distinct_pair_in_list_order() {
        assert_eq!(
            reducer_name_pairs().collect::<Vec<_>>(),
            vec![("count", "min"), ("count", "max"), ("min", "max")]
        );
    }

    #[test]
    fn recipe_groupby_scalar_multi_agg_builds_the_expected_walk_from_a_real_schema() {
        let grammar = CompiledGrammar::compile(vocab_for_recipes());
        let vocab = grammar.vocab();
        let expected: Vec<u32> = [
            "|", "A", ".", "all", "(", ")", "->", "groupBy", "(", "[", "]", ",", "[", "agg", "(",
            "a", "|", "$", "a", ".", "year", ",", "b", "|", "$", "b", "->", "count", "(", ")", ")",
            ",", "agg", "(", "a", "|", "$", "a", ".", "year", ",", "b", "|", "$", "b", "->", "min",
            "(", ")", ")", "]", ",", "[", "'col1'", ",", "'col2'", "]", ")",
        ]
        .iter()
        .map(|s| id_of(vocab, s))
        .collect();
        assert_eq!(
            recipe_groupby_scalar_multi_agg(&grammar, &recipe_schema(), vocab),
            Some(expected)
        );
    }

    #[test]
    fn recipe_groupby_scalar_multi_agg_is_none_with_only_one_reducer_name_in_vocab() {
        // Every token the shape needs *except* a second distinct
        // `UNCONSTRAINED_REDUCER_NAMES` entry: one reducer alone cannot
        // realize a multi-metric aggregation.
        let tokens: Vec<Vec<u8>> = [
            "|", "A", ".", "all", "(", ")", "->", "groupBy", "agg", "a", "b", "$", "year", "count",
            "[", "]", ",", "'col1'", "'col2'",
        ]
        .iter()
        .map(|s| s.as_bytes().to_vec())
        .collect();
        let eos = tokens.len() as u32;
        let grammar = CompiledGrammar::compile(Vocab::from_byte_tokens(tokens, eos));
        assert_eq!(
            recipe_groupby_scalar_multi_agg(&grammar, &recipe_schema(), grammar.vocab()),
            None
        );
    }

    #[test]
    fn recipe_groupby_scalar_multi_agg_is_none_without_the_groupby_step_name_in_vocab() {
        let tokens: Vec<Vec<u8>> = [
            "|", "A", ".", "all", "(", ")", "->", "agg", "a", "b", "$", "year", "count", "min",
            "[", "]", ",", "'col1'", "'col2'",
        ]
        .iter()
        .map(|s| s.as_bytes().to_vec())
        .collect();
        let eos = tokens.len() as u32;
        let grammar = CompiledGrammar::compile(Vocab::from_byte_tokens(tokens, eos));
        assert_eq!(
            recipe_groupby_scalar_multi_agg(&grammar, &recipe_schema(), grammar.vocab()),
            None
        );
    }

    #[test]
    fn recipe_groupby_scalar_multi_agg_is_none_without_two_distinct_quoted_string_tokens() {
        let tokens: Vec<Vec<u8>> = [
            "|", "A", ".", "all", "(", ")", "->", "groupBy", "agg", "a", "b", "$", "year", "count",
            "min", "[", "]", ",",
        ]
        .iter()
        .map(|s| s.as_bytes().to_vec())
        .collect();
        let eos = tokens.len() as u32;
        let grammar = CompiledGrammar::compile(Vocab::from_byte_tokens(tokens, eos));
        assert_eq!(
            recipe_groupby_scalar_multi_agg(&grammar, &recipe_schema(), grammar.vocab()),
            None
        );
    }

    #[test]
    fn recipe_groupby_scalar_multi_agg_is_none_without_a_real_class_member_pair() {
        // Every structural token the shape needs, plus two reducer names and
        // two quoted strings, but no real class or member name at all.
        let tokens: Vec<Vec<u8>> = [
            "|", ".", "all", "(", ")", "->", "groupBy", "agg", "a", "b", "$", "count", "min", "[",
            "]", ",", "'col1'", "'col2'",
        ]
        .iter()
        .map(|s| s.as_bytes().to_vec())
        .collect();
        let eos = tokens.len() as u32;
        let grammar = CompiledGrammar::compile(Vocab::from_byte_tokens(tokens, eos));
        assert_eq!(
            recipe_groupby_scalar_multi_agg(&grammar, &recipe_schema(), grammar.vocab()),
            None
        );
    }

    #[test]
    fn recipe_groupby_restrict_builds_the_expected_walk_from_a_real_schema() {
        let grammar = CompiledGrammar::compile(vocab_for_recipes());
        let vocab = grammar.vocab();
        let expected: Vec<u32> = [
            "|", "A", ".", "all", "(", ")", "->", "groupBy", "(", "[", "a", "|", "$", "a", ".",
            "year", "]", ",", "[", "agg", "(", "a", "|", "$", "a", ".", "year", ",", "b", "|", "$",
            "b", "->", "count", "(", ")", ")", "]", ",", "[", "'col1'", ",", "'col2'", "]", ")",
            "->", "restrict", "(", "[", "'col2'", ",", "'col1'", "]", ")",
        ]
        .iter()
        .map(|s| id_of(vocab, s))
        .collect();
        assert_eq!(
            recipe_groupby_restrict(&grammar, &recipe_schema(), vocab),
            Some(expected)
        );
    }

    #[test]
    fn recipe_groupby_restrict_is_none_without_the_restrict_step_name_in_vocab() {
        // Everything `recipe_groupby` itself needs — which still succeeds on
        // this vocabulary — but no "restrict" token, so only the
        // restrict-tailed recipe drops out.
        let tokens: Vec<Vec<u8>> = [
            "|", "A", ".", "all", "(", ")", "->", "groupBy", "agg", "a", "b", "$", "year", "count",
            "[", "]", ",", "'col1'", "'col2'",
        ]
        .iter()
        .map(|s| s.as_bytes().to_vec())
        .collect();
        let eos = tokens.len() as u32;
        let grammar = CompiledGrammar::compile(Vocab::from_byte_tokens(tokens, eos));
        let vocab = grammar.vocab();
        assert!(recipe_groupby(&grammar, &recipe_schema(), vocab).is_some());
        assert_eq!(
            recipe_groupby_restrict(&grammar, &recipe_schema(), vocab),
            None
        );
    }

    #[test]
    fn recipe_groupby_restrict_is_none_without_two_distinct_quoted_string_tokens() {
        let tokens: Vec<Vec<u8>> = [
            "|", "A", ".", "all", "(", ")", "->", "groupBy", "restrict", "agg", "a", "b", "$",
            "year", "count", "[", "]", ",",
        ]
        .iter()
        .map(|s| s.as_bytes().to_vec())
        .collect();
        let eos = tokens.len() as u32;
        let grammar = CompiledGrammar::compile(Vocab::from_byte_tokens(tokens, eos));
        assert_eq!(
            recipe_groupby_restrict(&grammar, &recipe_schema(), grammar.vocab()),
            None
        );
    }

    #[test]
    fn recipe_groupby_having_restrict_builds_the_expected_walk_from_a_real_schema() {
        let grammar = CompiledGrammar::compile(vocab_for_recipes());
        let vocab = grammar.vocab();
        let expected: Vec<u32> = [
            "|",
            "A",
            ".",
            "all",
            "(",
            ")",
            "->",
            "groupBy",
            "(",
            "[",
            "a",
            "|",
            "$",
            "a",
            ".",
            "year",
            "]",
            ",",
            "[",
            "agg",
            "(",
            "a",
            "|",
            "$",
            "a",
            ".",
            "year",
            ",",
            "b",
            "|",
            "$",
            "b",
            "->",
            "count",
            "(",
            ")",
            ")",
            "]",
            ",",
            "[",
            "'col1'",
            ",",
            "'col2'",
            "]",
            ")",
            "->",
            "filter",
            "(",
            "r",
            "|",
            "$",
            "r",
            ".",
            "getInteger",
            "(",
            "'col2'",
            ")",
            " ",
            ">",
            " ",
            "1",
            ")",
            "->",
            "restrict",
            "(",
            "[",
            "'col2'",
            ",",
            "'col1'",
            "]",
            ")",
        ]
        .iter()
        .map(|s| id_of(vocab, s))
        .collect();
        assert_eq!(
            recipe_groupby_having_restrict(&grammar, &recipe_schema(), vocab),
            Some(expected)
        );
    }

    #[test]
    fn recipe_groupby_having_restrict_is_none_without_the_restrict_step_name_in_vocab() {
        let tokens: Vec<Vec<u8>> = [
            "|",
            "A",
            ".",
            "all",
            "(",
            ")",
            "->",
            "groupBy",
            "agg",
            "a",
            "b",
            "$",
            "year",
            "count",
            "[",
            "]",
            ",",
            "'col1'",
            "'col2'",
            "filter",
            "r",
            "getInteger",
            ">",
            " ",
            "1",
        ]
        .iter()
        .map(|s| s.as_bytes().to_vec())
        .collect();
        let eos = tokens.len() as u32;
        let grammar = CompiledGrammar::compile(Vocab::from_byte_tokens(tokens, eos));
        assert_eq!(
            recipe_groupby_having_restrict(&grammar, &recipe_schema(), grammar.vocab()),
            None
        );
    }

    #[test]
    fn recipe_groupby_having_restrict_is_none_without_a_digit_token() {
        let tokens: Vec<Vec<u8>> = [
            "|",
            "A",
            ".",
            "all",
            "(",
            ")",
            "->",
            "groupBy",
            "agg",
            "a",
            "b",
            "$",
            "year",
            "count",
            "[",
            "]",
            ",",
            "'col1'",
            "'col2'",
            "filter",
            "restrict",
            "r",
            "getInteger",
            ">",
            " ",
        ]
        .iter()
        .map(|s| s.as_bytes().to_vec())
        .collect();
        let eos = tokens.len() as u32;
        let grammar = CompiledGrammar::compile(Vocab::from_byte_tokens(tokens, eos));
        assert_eq!(
            recipe_groupby_having_restrict(&grammar, &recipe_schema(), grammar.vocab()),
            None
        );
    }

    #[test]
    fn string_member_candidates_keeps_only_string_typed_members() {
        let vocab = vocab_for_recipes();
        assert_eq!(
            string_member_candidates(&recipe_schema(), &vocab),
            vec![(id_of(&vocab, "A"), id_of(&vocab, "label"))]
        );
    }

    #[test]
    fn recipe_filter_project_builds_the_expected_walk_from_a_real_schema() {
        let grammar = CompiledGrammar::compile(vocab_for_recipes());
        let vocab = grammar.vocab();
        let expected: Vec<u32> = [
            "|", "A", ".", "all", "(", ")", "->", "filter", "(", "a", "|", "$", "a", ".", "label",
            " ", "==", " ", "'col1'", ")", "->", "project", "(", "[", "a", "|", "$", "a", ".",
            "label", "]", ",", "[", "'col2'", "]", ")",
        ]
        .iter()
        .map(|s| id_of(vocab, s))
        .collect();
        assert_eq!(
            recipe_filter_project(&grammar, &recipe_schema(), vocab),
            Some(expected)
        );
    }

    #[test]
    fn recipe_filter_project_is_none_without_the_project_step_name_in_vocab() {
        let tokens: Vec<Vec<u8>> = [
            "|", "A", ".", "all", "(", ")", "->", "filter", "a", "$", "label", "==", " ", "[", "]",
            ",", "'col1'", "'col2'",
        ]
        .iter()
        .map(|s| s.as_bytes().to_vec())
        .collect();
        let eos = tokens.len() as u32;
        let grammar = CompiledGrammar::compile(Vocab::from_byte_tokens(tokens, eos));
        assert_eq!(
            recipe_filter_project(&grammar, &recipe_schema(), grammar.vocab()),
            None
        );
    }

    #[test]
    fn recipe_filter_project_is_none_without_a_string_member() {
        // "year" (Integer) is a real member, but not a `String` one — the
        // shape's `==` comparator against a string literal needs a `String`
        // member specifically.
        let tokens: Vec<Vec<u8>> = [
            "|", "A", ".", "all", "(", ")", "->", "filter", "project", "a", "$", "year", "==", " ",
            "[", "]", ",", "'col1'", "'col2'",
        ]
        .iter()
        .map(|s| s.as_bytes().to_vec())
        .collect();
        let eos = tokens.len() as u32;
        let grammar = CompiledGrammar::compile(Vocab::from_byte_tokens(tokens, eos));
        assert_eq!(
            recipe_filter_project(&grammar, &recipe_schema(), grammar.vocab()),
            None
        );
    }

    #[test]
    fn recipe_walks_includes_only_the_recipes_that_actually_succeed() {
        // The full vocabulary with include_reducer: all seven recipes succeed.
        let full = CompiledGrammar::compile(vocab_for_recipes());
        let all_seven = recipe_walks(&full, &recipe_schema(), full.vocab(), true);
        assert_eq!(all_seven.len(), 7);

        // Missing "agg"/"groupBy"/"restrict"/quoted strings: only the
        // navigation-predicate recipe can succeed regardless of
        // include_reducer.
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
    fn recipe_walks_excludes_only_the_reducer_recipe_when_include_reducer_is_false() {
        // The full vocabulary supports all seven recipes, but
        // include_reducer=false (issue #55: the reducer recipe's bare
        // `->agg(...)` step is L1/L2-admissible but not real, compilable
        // Pure) drops only that one — `recipe_groupby`,
        // `recipe_groupby_scalar_multi_agg`, `recipe_groupby_restrict`,
        // `recipe_groupby_having_restrict`, and `recipe_filter_project` (all
        // real, compilable) stay included.
        let full = CompiledGrammar::compile(vocab_for_recipes());
        let six = recipe_walks(&full, &recipe_schema(), full.vocab(), false);
        assert_eq!(six.len(), 6);
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

    #[test]
    fn the_walk_set_splits_exactly_where_the_recipe_partition_ends() {
        let grammar = CompiledGrammar::compile(vocab_for_recipes());
        let schema = recipe_schema();
        let set = generate_first_complete_schema_walk_set(&grammar, &schema);

        // The boundary is the eager generator's own recipe count
        // (include_reducer = false), and the two partitions reassemble into
        // the full, unchanged walk sequence.
        let recipes = recipe_walks(&grammar, &schema, grammar.vocab(), false);
        assert_eq!(set.recipe_len(), recipes.len());
        assert_eq!(set.walks().len(), WALK_COUNT);
        let (recipe, exploration) = set.walks().split_at(set.recipe_len());
        assert_eq!(recipe, recipes.as_slice());
        assert_eq!(exploration.len(), WALK_COUNT - recipes.len());
        assert_eq!(
            set.walks(),
            generate_first_complete_schema_walks(&grammar, &schema).as_slice(),
            "the set's walks must be the very sequence the Vec-returning generator yields"
        );
    }

    #[test]
    fn a_schema_with_no_admissible_recipe_yields_an_all_exploration_walk_set() {
        // The minimal vocabulary supports no recipe shape at all, so the
        // partition boundary sits at zero rather than at a placeholder walk.
        let grammar = CompiledGrammar::compile(vocab_with_source_method_ambiguity());
        let set = generate_first_complete_schema_walk_set(&grammar, &schema());
        assert_eq!(set.recipe_len(), 0);
        assert_eq!(set.walks().len(), WALK_COUNT);
    }
}
