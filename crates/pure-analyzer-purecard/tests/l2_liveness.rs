//! L2 **liveness** (`docs/spec/schema.md` §6.7): over a vocabulary complete over
//! the grammar's alphabet, the schema overlay may narrow the per-step mask, but it
//! may never empty it — nor, at a term it agrees is already whole, mask away every
//! way of ending that term.
//!
//! A mask with no vocabulary bit *and* no EOS bit hands the host nothing to
//! sample and no way to stop — a decoder deadlock, not a constraint (issue
//! #275). The lane that was supposed to catch that (`l2_properties.rs`) could
//! not: its subset check iterates `l2_mask.iter_ones()`, which does nothing at
//! all on an empty mask, so it passed over the defect. This lane asserts the
//! missing half, and pins the four witnesses that violated it.
//!
//! The second invariant is the same failure one degree weaker, and is what issue
//! #296 was: a rule written as a permit set of *continuations* left the mask
//! non-empty but cleared every **terminator**, so a stream that had already
//! produced a whole term could only be extended, never ended. At a completed-term
//! position no obligation is outstanding — no arity to meet, no name to finish —
//! so the only remaining question is which frame is open, and that is the
//! byte-PDA's to answer: whatever terminator L1 admits there, L2 must admit too.
//!
//! **The precondition is load-bearing, and it is why this lane is byte-granular.**
//! A rule that narrows to a set of *names* can only leave a live token if some
//! token spells a legal name's next bytes; a vocabulary too impoverished to do
//! that empties the mask with the rule behaving exactly as specified (see §6.7,
//! and `l2_precision.rs`, whose walk-local vocabularies do exactly this on
//! purpose). The vocabulary here carries every byte in the grammar's alphabet
//! (printable ASCII plus `WS`) as its own token, and every real BPE vocabulary
//! covers that alphabet many times over — so this is the *shipping* case, not a
//! lenient one.
//!
//! Four deliberate choices make the search adversarial rather than confirmatory:
//!
//! * **A single-byte vocabulary.** Every printable ASCII byte is its own token.
//!   On the axis this lane is about — whether a rule that classifies *whole*
//!   lexemes still leaves a continuation — it is the worst case: it is what lets a
//!   stream stop on a bare `$` sigil or on the `.` that opens a float, at a
//!   position the rule was written for a whole token. It is deliberately the
//!   *easiest* case on the spellability axis above, which is the precondition
//!   rather than the claim; the one shape it cannot express — a token that *ends*
//!   on the sigil — gets its own hand-built vocabulary below.
//! * **Only L2-admissible tokens are ever accepted.** The walk samples from the
//!   schema mask itself, so every position it reaches is one the overlay claims
//!   is legal — a position the host would genuinely arrive at.
//! * **Two walk families.** Cold walks from an empty stream reach the shapes no
//!   gold query spells; gold-anchored walks branch off a real query and reach the
//!   deeply-scoped rules a cold random walk essentially never gets to. The third
//!   witness here was found only by the second family.
//! * **All 8 committed schema fixtures**, so the invariant is a property of the
//!   overlay and not of one schema's shape.
#![forbid(unsafe_code)]

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::OnceLock;

#[path = "support/completed_term.rs"]
mod completed_term;
#[path = "support/corpus.rs"]
mod corpus;
#[path = "support/error.rs"]
mod error;
#[path = "support/fixture_dbs.rs"]
mod fixture_dbs;
#[path = "support/l2.rs"]
mod l2;
#[path = "support/l2_rules.rs"]
mod l2_rules;
#[path = "support/lex.rs"]
mod lex;

use completed_term::{TERM_END_BYTES, is_completed_term};
use corpus::load_gold;
use fixture_dbs::FIXTURE_DBS;
use l2::load_schema;
use l2_rules::rule_kind;
use purecard::{BitMask, CompiledGrammar, DecoderSession, Schema, Vocab};

/// The vocabulary: one token per printable ASCII byte, `' '` (0x20) through `'~'`
/// (0x7e), plus the non-printing whitespace bytes. Together that is exactly the
/// grammar's own alphabet — printable ASCII and `WS` (`b" \t\n\r"`, `pda.rs`) —
/// so every continuation the byte-PDA admits is spellable by some token, which is
/// the liveness invariant's precondition. A byte-granular vocabulary is also the
/// adversarial case for an overlay whose rules read whole lexemes: a whole-token
/// vocabulary can never leave the stream parked on a lone `$` or a lone `.`.
const FIRST_BYTE: u8 = 0x20;
const LAST_BYTE: u8 = 0x7e;
const LAYOUT_BYTES: &[u8] = b"\t\n\r";

/// The anchor of the refVar family's witness: a block query, opened with nothing
/// bound. Its `$` continuation (`{|$`) put the byte-PDA at `AfterDollar`, which
/// admits nothing but an identifier, while S2's legal-name set — the names the
/// stream has bound — was empty, so the rule cleared every token there and the
/// EOS bit with them. S2 now reads at the sigil instead, where a live alternative
/// still exists, so the deadlocked position is no longer reachable.
const WITNESS_BLOCK_ANCHOR: &str = "{|";

/// The sigil S2 masks while nothing is bound.
const REFVAR_SIGIL: u8 = b'$';

/// The prefix of the operand-class family's witness, up to the armed operand
/// slot: `||` arms N4b's Boolean operand, and `let->x&&B` is the shortest walk
/// that reaches an armable value position (delta-minimized from a randomly
/// generated witness).
///
/// The witness itself continued `.`, which opens a leading-dot float (`.5`) and
/// is not a number *token*, so it slipped past N4b's whole-token classifier —
/// leaving the byte-PDA at `NeedFracDigit`, which admits only digits, exactly the
/// class N4b masks. The rule now clears the openers alongside the numbers they
/// open, so the deadlocked position is no longer reachable at all.
const WITNESS_OPERAND_SLOT: &str = "|let->x&&B||";

/// The numeric-literal openers a Boolean operand slot must clear with the numbers
/// they open (`NUMBER_OPENERS` in `src/schema/narrow.rs`).
const NUMBER_OPENERS: &[u8] = b".-";

/// The witness of the third family, which only the gold-anchored walk below
/// reaches: a join lambda's **typed** binder (`row: meta::pure::tds::TDSRow[1]|`).
/// The type path landed in S2's bound-variable record, and S2 walks its names as
/// plain identifiers — a `::` ends the lexeme — so `$meta` sat on a live prefix of
/// a name no `$<IDENT>` can ever finish: every identifier byte diverged, every
/// boundary token was cleared as mid-name, and the EOS bit with them.
const WITNESS_TYPED_BINDER: &str = "|spider::battle_death::Db->tableReference('default','death')\
    ->tableToTDS()->filter(row: meta::pure::tds::TDSRow[1]|$";

/// The fixture database `WITNESS_TYPED_BINDER` is spelled against.
const WITNESS_TYPED_BINDER_DB: &str = "battle_death";

/// Seeded walks per fixture database. Deterministic (a committed SplitMix64 over
/// a committed base seed), so the lane is reproducible in CI and never flakes
/// (constitution §2 — no local-only state).
const WALK_SEEDS: u64 = 256;

/// Hard cap on accepted tokens per walk — a bound so a walk terminates rather
/// than spins; a single-byte vocabulary needs many tokens to build anything, so
/// it is generous.
const WALK_MAX_TOKENS: usize = 64;

/// SplitMix64's golden-ratio increment and two avalanche multipliers (Steele et
/// al., 2014), and the base seed.
///
/// A third copy of an eight-line, fully specified PRNG, and knowingly so: the two
/// neighbours (`tests/support/walker.rs`, the `schema-walker` crate) each keep it
/// private to a module this lane has no other reason to pull in — `support/walker.rs`
/// is the byte-PDA walk generator, whose every other symbol would be dead code
/// here. Sharing it would mean a support module whose only export is a PRNG. The
/// *stream* is distinct because the seed is, not because the algorithm is.
const SPLITMIX_GAMMA: u64 = 0x9E37_79B9_7F4A_7C15;
const SPLITMIX_MIX_A: u64 = 0xBF58_476D_1CE4_E5B9;
const SPLITMIX_MIX_B: u64 = 0x94D0_49BB_1331_11EB;
const BASE_SEED: u64 = 0x5075_7265_4361_7264; // "PureCard" as ASCII bytes.

struct SplitMix64(u64);

impl SplitMix64 {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(SPLITMIX_GAMMA);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(SPLITMIX_MIX_A);
        z = (z ^ (z >> 27)).wrapping_mul(SPLITMIX_MIX_B);
        z ^ (z >> 31)
    }
}

/// Every byte the vocabulary carries, in id order. Built once: `id_of` runs inside
/// the walk loop, and rebuilding the table per call would allocate per step.
fn vocab_bytes() -> &'static [u8] {
    static BYTES: OnceLock<Vec<u8>> = OnceLock::new();
    BYTES.get_or_init(|| {
        (FIRST_BYTE..=LAST_BYTE)
            .chain(LAYOUT_BYTES.iter().copied())
            .collect()
    })
}

/// The single-byte vocabulary plus its reserved EOS id.
fn byte_vocab() -> (Vocab, u32) {
    let tokens: Vec<Vec<u8>> = vocab_bytes().iter().map(|byte| vec![*byte]).collect();
    let eos = tokens.len() as u32;
    (Vocab::from_byte_tokens(tokens), eos)
}

/// The token id of `byte` in the single-byte vocabulary.
fn id_of(byte: u8) -> u32 {
    let id = vocab_bytes()
        .iter()
        .position(|candidate| *candidate == byte)
        .unwrap_or_else(|| panic!("byte {byte:#04x} is outside the vocabulary"));
    id as u32
}

/// Drive both an L2 and an L1 session over `text`, asserting every byte is
/// **L2-admissible** on the way (so the position reached is genuinely reachable),
/// and return the two sessions parked at the position after the last byte.
fn drive<'g>(
    grammar: &'g CompiledGrammar,
    schema: &Schema,
    text: &str,
) -> (DecoderSession<'g>, DecoderSession<'g>) {
    let mut l2 =
        DecoderSession::with_schema(grammar, schema.clone()).expect("grammar is fixed-engine");
    let mut l1 = DecoderSession::new(grammar);
    for (offset, byte) in text.bytes().enumerate() {
        let id = id_of(byte);
        assert!(
            l2.allowed_mask().test(id),
            "witness {text:?} is not an L2-admissible walk: byte {:?} at {offset} is masked",
            byte as char
        );
        l2.accept_token(id).expect("L2-admitted token accepts");
        l1.accept_token(id).expect("L1 admits what L2 admits");
    }
    (l2, l1)
}

/// The ids set in `mask`, for a legible assertion message.
fn live(mask: &BitMask) -> Vec<u32> {
    mask.iter_ones().collect()
}

#[test]
fn a_refvar_sigil_is_masked_while_nothing_is_bound() {
    let (vocab, _eos) = byte_vocab();
    let grammar = CompiledGrammar::compile(vocab);
    for db_id in FIXTURE_DBS {
        let schema = load_schema(db_id);
        let (mut l2, mut l1) = drive(&grammar, &schema, WITNESS_BLOCK_ANCHOR);
        let l1_mask = l1.allowed_mask().clone();
        let l2_mask = l2.allowed_mask();
        assert!(
            l1_mask.test(id_of(REFVAR_SIGIL)),
            "{db_id}: L1 admits the sigil here — that is the whole reason S2 has to"
        );
        assert!(
            !l2_mask.test(id_of(REFVAR_SIGIL)),
            "{db_id}: the sigil is admitted at {WITNESS_BLOCK_ANCHOR:?} with nothing bound, so \
             the stream can still walk into the empty-mask position"
        );
        assert!(
            !live(l2_mask).is_empty(),
            "{db_id}: empty L2 mask at {WITNESS_BLOCK_ANCHOR:?}"
        );
    }
}

/// Once a binder exists the sigil is admitted again — the mask is a statement
/// about *this* stream's scope, not a blanket ban, and without this half the rule
/// could be satisfied by clearing `$` outright.
#[test]
fn a_refvar_sigil_is_admitted_once_a_binder_exists() {
    let (vocab, _eos) = byte_vocab();
    let grammar = CompiledGrammar::compile(vocab);
    let db_id = "world_1";
    let schema = load_schema(db_id);
    let bound = "|spider::world_1::model::default::Country.all()->filter(x|";
    let (mut l2, _l1) = drive(&grammar, &schema, bound);
    assert!(
        l2.allowed_mask().test(id_of(REFVAR_SIGIL)),
        "{db_id}: the sigil must stay admissible once the lambda has bound a name"
    );
}

#[test]
fn a_logical_operand_masks_the_float_openers_that_deadlocked_it() {
    let (vocab, _eos) = byte_vocab();
    let grammar = CompiledGrammar::compile(vocab);
    for db_id in FIXTURE_DBS {
        let schema = load_schema(db_id);
        let (mut l2, _l1) = drive(&grammar, &schema, WITNESS_OPERAND_SLOT);
        let mask = l2.allowed_mask();
        assert!(
            !live(mask).is_empty(),
            "{db_id}: empty L2 mask at the armed operand slot {WITNESS_OPERAND_SLOT:?}"
        );
        // A digit is masked here because a number is not a Boolean — the rule N4b
        // always applied. The two *openers* of the same literal must be masked with
        // it, or the stream walks into a byte-PDA state that admits only the digits
        // this rule clears.
        for byte in NUMBER_OPENERS.iter().chain(b"0123456789") {
            assert!(
                !mask.test(id_of(*byte)),
                "{db_id}: {:?} opens a numeric literal in a Boolean operand slot \
                 and must be masked at {WITNESS_OPERAND_SLOT:?}",
                *byte as char
            );
        }
        // The slot is still live for what a Boolean operand really takes.
        assert!(
            mask.test(id_of(b'$')) && mask.test(id_of(b'(')),
            "{db_id}: N4b masked the shapes a Boolean operand does take"
        );
    }
}

#[test]
fn a_binder_type_path_is_not_a_bindable_variable_name() {
    let (vocab, _eos) = byte_vocab();
    let grammar = CompiledGrammar::compile(vocab);
    let schema = load_schema(WITNESS_TYPED_BINDER_DB);
    let (mut l2, _l1) = drive(&grammar, &schema, WITNESS_TYPED_BINDER);
    let mask = l2.allowed_mask();
    assert!(
        !live(mask).is_empty(),
        "empty L2 mask at {WITNESS_TYPED_BINDER:?}: no token to sample and no way to stop"
    );
    // The lambda's real binder is still bindable...
    assert!(
        mask.test(id_of(b'r')),
        "S2 masked the first byte of the binder the lambda actually declared"
    );
    // ...and the first byte of its *type* path is not, so no stream can walk onto
    // a prefix of a name S2's own trie could never finish.
    assert!(
        !mask.test(id_of(b'm')),
        "the binder's type path is still recorded as a bindable variable name"
    );
}

/// The one shape the single-byte lane above cannot reach, and the reason S2's
/// trie rule keeps a fail-open of its own on top of the sigil pass.
///
/// The sigil pass is first-byte discriminated (`fill_unbound_sigil`), because
/// under byte-level BPE the sigil arrives *fused to the name it opens* (`$code`).
/// It therefore cannot see a token that ends on the sigil instead — `($` is one
/// token in a real BPE vocabulary — which lands the byte-PDA on `AfterDollar`
/// with nothing bound anyway. `AfterDollar` admits only an identifier and every
/// identifier token is an S2 candidate, so without the rule's own guard the mask
/// there is empty. This needs a multi-byte vocabulary to express at all.
#[test]
fn a_token_that_ends_on_the_sigil_still_leaves_a_live_mask() {
    const FUSED_SIGIL: &[u8] = b"($";
    let tokens: Vec<Vec<u8>> = [
        b"|".as_slice(),
        b"spider::world_1::model::default::Country",
        b".",
        b"all",
        b"(",
        b")",
        b"->",
        b"filter",
        FUSED_SIGIL,
        b"zzz",
    ]
    .iter()
    .map(|token| token.to_vec())
    .collect();
    let grammar = CompiledGrammar::compile(Vocab::from_byte_tokens(tokens.clone()));
    let schema = load_schema("world_1");
    let id_of_token = |needle: &[u8]| {
        tokens
            .iter()
            .position(|token| token == needle)
            .expect("token is in this vocabulary") as u32
    };

    let mut session =
        DecoderSession::with_schema(&grammar, schema).expect("grammar is fixed-engine");
    for token in [
        b"|".as_slice(),
        b"spider::world_1::model::default::Country",
        b".",
        b"all",
        b"(",
        b")",
        b"->",
        b"filter",
        FUSED_SIGIL,
    ] {
        let id = id_of_token(token);
        assert!(
            session.allowed_mask().test(id),
            "the fused-sigil walk is not L2-admissible at {:?}",
            String::from_utf8_lossy(token)
        );
        session.accept_token(id).expect("L2-admitted token accepts");
    }
    let mask = session.allowed_mask();
    assert!(
        !live(mask).is_empty(),
        "empty L2 mask after a token that ends on the sigil: no token to sample and no way to stop"
    );
    // The fail-open is what leaves it live: S2 has no name to keep, so it stops
    // constraining and the byte-PDA's own identifier set stands.
    assert!(
        mask.test(id_of_token(b"zzz")),
        "S2 narrowed a position where it has no legal name at all"
    );
}

/// What the walk actually exercised, so the assertions below can be shown to be
/// about positions that were genuinely at risk rather than never reached.
#[derive(Default)]
struct Coverage {
    /// Every rule kind active at some visited position.
    rules: BTreeSet<&'static str>,
    /// How many times a terminator was compared at a completed term *because L1
    /// admitted it there* — the only comparisons that can fail. Reaching the rule
    /// is not enough: a position where L1 ends nothing tests nothing.
    term_end_checks: usize,
}

/// Assert the per-step invariants at the sessions' current position, and
/// record what was exercised there.
fn assert_step_invariants(
    l2: &mut DecoderSession<'_>,
    l1: &mut DecoderSession<'_>,
    eos: u32,
    db_id: &str,
    text: &str,
    coverage: &mut Coverage,
) -> Vec<u32> {
    if let Some(kind) = l2.active_l2_position().as_ref().and_then(rule_kind) {
        coverage.rules.insert(kind);
    }
    let completed_term = l2
        .active_l2_position()
        .as_ref()
        .is_some_and(is_completed_term);
    let complete = l2.is_complete();
    let l1_mask = l1.allowed_mask().clone();
    let l2_mask = l2.allowed_mask();
    let ids = live(l2_mask);
    // The invariant this lane exists for.
    assert!(
        !ids.is_empty(),
        "empty L2 mask ({db_id}) after {text:?}: no token to sample and no way to stop"
    );
    // Re-checked here rather than left to `l2_properties.rs`: that lane iterates
    // the L2 mask's set bits, which is vacuous on an empty mask — the very gap
    // that let issue #275 through — so the two halves belong at one position.
    assert!(
        ids.iter().all(|&id| l1_mask.test(id)),
        "L2 widened L1 ({db_id}) after {text:?}"
    );
    assert_eq!(
        complete,
        l2_mask.test(eos),
        "({db_id}) after {text:?}: the published EOS bit and `is_complete` disagree"
    );
    // The completed-term half: a whole term may always be *ended* any way the
    // byte-PDA can end it here.
    if completed_term {
        for byte in TERM_END_BYTES {
            let id = id_of(*byte);
            coverage.term_end_checks += usize::from(l1_mask.test(id));
            assert!(
                !l1_mask.test(id) || l2_mask.test(id),
                "({db_id}) after {text:?}: the term is whole and L1 ends it with {:?}, but L2 \
                 cleared it — the stream can be extended here and never ended",
                char::from(*byte)
            );
        }
        coverage.term_end_checks += usize::from(l1_mask.test(eos));
        assert!(
            !l1_mask.test(eos) || l2_mask.test(eos),
            "({db_id}) after {text:?}: the term is whole and L1 admits the end of the stream, \
             but L2 cleared the EOS bit"
        );
    }
    ids
}

/// The in-scope gold queries of `db_id`.
fn gold_queries(db_id: &str) -> Vec<String> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("corpus/gold_queries.jsonl");
    load_gold(&path)
        .expect("open the committed gold corpus")
        .map(|item| item.expect("gold line parses"))
        .filter(|record| record.db_id == db_id)
        .map(|record| record.pure_text)
        .collect()
}

/// The invariant, at every position an L2-admissible stream can reach.
///
/// Two complementary walk families, because neither alone is enough. The **cold**
/// walks start from an empty stream and only ever accept a token the L2 mask
/// itself admits — that is what reached the `{|$` witness, which no gold query
/// ever spells. The **gold-anchored** walks replay a real query byte by byte and
/// then branch off it at a seeded cut point, which is how the deeply-scoped rules
/// (a bound `$var`, an emitted column, a typed comparison operand) are reached at
/// all: a cold random walk essentially never happens to open a lambda and bind a
/// name.
#[test]
fn the_l2_mask_is_never_empty_at_a_position_l2_admissible_tokens_reach() {
    let (vocab, eos) = byte_vocab();
    let grammar = CompiledGrammar::compile(vocab.clone());
    let mut coverage = Coverage::default();
    let mut steps = 0usize;

    for db_id in FIXTURE_DBS {
        let schema = load_schema(db_id);
        let queries = gold_queries(db_id);
        assert!(
            !queries.is_empty(),
            "no gold queries for fixture db {db_id}"
        );

        for walk in 0..WALK_SEEDS {
            let mut rng = SplitMix64(BASE_SEED.wrapping_add(walk.wrapping_mul(SPLITMIX_GAMMA)));
            let mut l2 = DecoderSession::with_schema(&grammar, schema.clone())
                .expect("grammar is fixed-engine");
            let mut l1 = DecoderSession::new(&grammar);
            let mut text = String::new();

            // The gold-anchored half: replay a query's bytes up to a seeded cut,
            // asserting the invariants over every real position on the way.
            if walk % 2 == 0 {
                let query = &queries[(rng.next() % queries.len() as u64) as usize];
                let bytes = query.as_bytes();
                let cut = (rng.next() % (bytes.len() as u64 + 1)) as usize;
                for &byte in &bytes[..cut] {
                    steps += 1;
                    let ids =
                        assert_step_invariants(&mut l2, &mut l1, eos, db_id, &text, &mut coverage);
                    let id = id_of(byte);
                    assert!(
                        ids.contains(&id),
                        "({db_id}) the overlay masked a gold byte {:?} after {text:?}",
                        byte as char
                    );
                    text.push(char::from(byte));
                    l2.accept_token(id).expect("L2-admitted token accepts");
                    l1.accept_token(id).expect("L1 admits what L2 admits");
                }
            }

            // The exploratory half: branch off wherever the replay stopped.
            for _ in 0..WALK_MAX_TOKENS {
                steps += 1;
                let ids =
                    assert_step_invariants(&mut l2, &mut l1, eos, db_id, &text, &mut coverage);
                let pick = ids[(rng.next() % ids.len() as u64) as usize];
                if pick == eos {
                    break;
                }
                let bytes = vocab.bytes(pick).expect("in-range id has bytes");
                text.push(char::from(bytes[0]));
                l2.accept_token(pick).expect("L2-admitted token accepts");
                l1.accept_token(pick).expect("L1 admits what L2 admits");
            }
        }
    }

    // Non-vacuity, tied to meaning rather than to a tuned number: the walk must
    // have reached the rules the two defects lived in — N4b and S2 for the
    // empty-mask half (#275), and all three completed-term rules for the
    // terminator half (#296). A search that never armed N4b, never bound a
    // variable, or never closed a call would assert the invariants over positions
    // that were never at risk.
    for rule in [
        "RefVar",
        "LogicalOperand",
        "SourceExtent",
        "StoreResult",
        "StrOperator",
    ] {
        assert!(
            coverage.rules.contains(rule),
            "the walk never reached the {rule} rule; {steps} steps visited {:?}",
            coverage.rules
        );
    }
    // Reaching those rules is necessary but not sufficient for the terminator
    // half: the comparison only bites where L1 itself ends the term, so the walk
    // has to have found such a position at least once.
    assert!(
        coverage.term_end_checks > 0,
        "no completed term was ever reached at a position where L1 admits a terminator, \
         so the terminator invariant was asserted vacuously over {steps} steps"
    );
}
