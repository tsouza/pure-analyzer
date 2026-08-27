//! Differential equivalence between the fixed hand-written PDA
//! (`grammar::pda`, via [`CompiledGrammar::compile`]) and the declarative
//! transcription of the shipped emitted-subset grammar
//! (`grammar::EMITTED_SUBSET_SPEC`, via [`CompiledGrammar::from_spec`]),
//! per issue #57.
//!
//! `grammar::compiled` already proves the spec-compiled engine mechanically
//! *works* (small hand-written specs, `src/grammar/compile.rs`'s own unit
//! tests); this suite proves the shipped-grammar *transcription itself* is
//! behaviorally faithful, by replaying every query in the gold corpus, the
//! negative precision corpus, and the modern-dialect seed corpus through both
//! engines and asserting they reach the identical verdict — the same dead-state
//! offset (or neither dies) and the same end-of-stream completeness. A
//! divergence names the exact query and byte offset to fix in
//! `src/grammar/emitted_subset.json`, never a reason to weaken this test.
#![forbid(unsafe_code)]

use std::path::PathBuf;

#[path = "support/corpus.rs"]
mod corpus;
#[path = "support/error.rs"]
mod error;
#[path = "support/walker.rs"]
mod walker;

use corpus::load_gold;
use purecard::grammar::EMITTED_SUBSET_SPEC;
use purecard::{ByteRecognizer, CompiledGrammar, DecodeError, DecoderSession, Vocab};

/// The `Vocab` EOS-token id an empty test vocabulary is built with — the
/// byte-recognizer lanes this suite drives never consult the vocab.
const EMPTY_VOCAB_EOS: u32 = 0;

/// Arm-A (relational envelope) gold record count (mirrors
/// `tests/soundness_replay.rs`).
const GOLD_ARM_A: usize = 4639;
/// Arm-C (class-navigation envelope) gold record count.
const GOLD_ARM_C: usize = 395;
/// The full committed gold corpus size.
const EXPECTED_GOLD_RECORDS: usize = GOLD_ARM_A + GOLD_ARM_C;

/// Arm-A (relational) modern-dialect seed count (mirrors
/// `tests/modern_dialect_soundness.rs`).
const SEED_ARM_A: usize = 0;
/// Arm-C (class-navigation) modern-dialect seed count.
const SEED_ARM_C: usize = 5;
/// Arm-R (Relation/Function API) modern-dialect seed count.
const SEED_ARM_R: usize = 14;
/// The full modern-dialect seed corpus size.
const EXPECTED_SEED_RECORDS: usize = SEED_ARM_A + SEED_ARM_C + SEED_ARM_R;

fn gold_corpus_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("corpus/gold_queries.jsonl")
}

fn seed_corpus_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("corpus/modern_dialect_seeds.jsonl")
}

fn fixed_grammar() -> CompiledGrammar {
    CompiledGrammar::compile(Vocab::from_byte_tokens(Vec::new(), EMPTY_VOCAB_EOS))
}

fn spec_grammar() -> CompiledGrammar {
    CompiledGrammar::from_spec(
        EMITTED_SUBSET_SPEC,
        Vocab::from_byte_tokens(Vec::new(), EMPTY_VOCAB_EOS),
    )
    .expect("the emitted-subset spec transcription compiles")
}

/// The observable outcome of replaying a byte string through one engine: the
/// byte offset a dead state was reached at (if any), and — when it never died —
/// whether the stream ended in a complete (accepting) configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Verdict {
    /// The stream never died and ended in an accepting configuration.
    Complete,
    /// The stream never died but ended in a non-accepting configuration.
    Incomplete,
    /// The stream died at this byte offset.
    Dead(usize),
}

/// Drive `bytes` through a fresh [`DecoderSession`] over `grammar`, reporting
/// the observable [`Verdict`] (dead-state offset, or end-of-stream
/// completeness) — the exact signal `assert_same_verdict` diffs between the
/// fixed and spec-compiled engines.
fn replay(bytes: &[u8], grammar: &CompiledGrammar) -> Verdict {
    let mut session = DecoderSession::new(grammar);
    for &byte in bytes {
        if let Err(DecodeError::DeadState { offset, .. }) = session.accept_byte(byte) {
            return Verdict::Dead(offset);
        }
    }
    if session.is_complete() {
        Verdict::Complete
    } else {
        Verdict::Incomplete
    }
}

/// Assert that `fixed` (the hand-written PDA) and `spec` (the declarative
/// transcription) reach the identical [`Verdict`] for `bytes` — the shared
/// differential-equivalence check every corpus in this suite runs through.
///
/// On failure the panic message names the exact query and the two diverging
/// verdicts, so a transcription bug is locatable without re-deriving it.
fn assert_same_verdict(bytes: &[u8], fixed: &CompiledGrammar, spec: &CompiledGrammar) {
    let fixed_verdict = replay(bytes, fixed);
    let spec_verdict = replay(bytes, spec);
    assert_eq!(
        fixed_verdict,
        spec_verdict,
        "fixed and spec-compiled grammars diverge on {:?}: fixed={fixed_verdict:?} spec={spec_verdict:?}",
        String::from_utf8_lossy(bytes)
    );
}

#[test]
fn gold_corpus_is_equivalent_across_both_engines() {
    let fixed = fixed_grammar();
    let spec = spec_grammar();
    let records = load_gold(&gold_corpus_path()).expect("open the committed gold corpus");

    let mut count = 0usize;
    for item in records {
        let record = item.expect("gold corpus failed to load");
        assert_same_verdict(record.pure_text.as_bytes(), &fixed, &spec);
        count += 1;
    }
    assert_eq!(count, EXPECTED_GOLD_RECORDS, "gold corpus record count");
}

#[test]
fn modern_dialect_seed_corpus_is_equivalent_across_both_engines() {
    let fixed = fixed_grammar();
    let spec = spec_grammar();
    let records = load_gold(&seed_corpus_path()).expect("open the modern-dialect seed corpus");

    let mut count = 0usize;
    for item in records {
        let record = item.expect("modern-dialect seed corpus failed to load");
        assert_same_verdict(record.pure_text.as_bytes(), &fixed, &spec);
        count += 1;
    }
    assert_eq!(
        count, EXPECTED_SEED_RECORDS,
        "modern-dialect seed record count"
    );
}

/// Every malformed-input string `tests/precision_reject.rs` exercises via its
/// `dies(...)` helper (both the ones that must reject and the well-formed
/// anchors that must not) — transcribed verbatim (identical string-literal
/// source, so the compiler computes the same continued/interpolated value
/// rather than a hand-derived copy) so this suite proves the two engines agree
/// on the *entire* negative corpus, not a resample of it.
fn precision_corpus() -> Vec<String> {
    let join = "|a::Db->tableReference('default','A')->tableToTDS()->join(\
                a::Db->tableReference('default','B')->tableToTDS(), \
                meta::relational::metamodel::join::JoinType.INNER, ";

    let mut cases: Vec<String> = vec![
        // well-formed anchors
        "|X.all()->take(3)".to_owned(),
        "|db::Db->tableReference('default','T')->tableToTDS()->limit(5)".to_owned(),
        "{|let m = X.all()->take(1); Y.all()->filter(b|$b.v == $m)->take(1);}".to_owned(),
        // a top-level source must be an identifier
        "|42".to_owned(),
        "|42 ".to_owned(),
        "|*".to_owned(),
        "|( )".to_owned(),
        "|'lit'".to_owned(),
        "|%2018-03-17".to_owned(),
        "|$x->take(1)".to_owned(),
        // a completed term is not followed by a bare identifier
        "|foo bar baz".to_owned(),
        "|foo bar baz ".to_owned(),
        "|X.all() take(3)".to_owned(),
        "|X.all()->take(1) take(2)".to_owned(),
        "|X.all()->filter(nonsense garbage here)".to_owned(),
        // a dangling operator before a closer dies
        "|X.all()->take(1 +)".to_owned(),
        "|X.all()->take(1 -)".to_owned(),
        "|X.all()->take(1 *)".to_owned(),
        "|X.all()->filter(x|$x.a && )".to_owned(),
        "|X.all()->filter(x|$x.a || )".to_owned(),
        "|X.all()->filter(x|$x.a > )".to_owned(),
        "|X.all()->filter(x|$x.a == )".to_owned(),
        // malformed numeric literals die
        "|X.all()->take(-)".to_owned(),
        "|X.all()->take(1.)".to_owned(),
        "|X.all()->take(--5)".to_owned(),
        "|X.all()->filter(x|$x.a > .)".to_owned(),
        "|X.all()->filter(x|$x.a > 1.5e)".to_owned(),
        // an empty date literal dies
        "|X.all()->take(%)".to_owned(),
        "|X.all()->filter(x|$x.d < %)".to_owned(),
        // milestoning literal is lowercase-letters-only
        "|X.all()->take(%Latest)".to_owned(),
        "|X.all()->take(%latest1)".to_owned(),
        "|X.all()->take(%latestX)".to_owned(),
        "|X.all(%latest)->take(1)".to_owned(),
        "|X.all(%latest, %latest)->take(1)".to_owned(),
        "|X.all(%latestdate)->take(1)".to_owned(),
        "|X.all()->filter(x|$x.d == %latest)".to_owned(),
        // arm-R tilde sigil
        "|X.all()->project(~)".to_owned(),
        "|X.all()->project(~ [Col: x|$x.a])".to_owned(),
        "|X.all()->project(~~[Col: x|$x.a])".to_owned(),
        "|X.all()->sort([ascending(~)])".to_owned(),
        "|~.all()->take(1)".to_owned(),
        "|X.all()->project(~[Col: x|$x.a])".to_owned(),
        "|X.all()->groupBy(~[K], ~'Agg': x|$x.v : y|$y->sum())".to_owned(),
        "|X.all()->sort([ascending(~A), descending(~B)])".to_owned(),
        "|X.all()->project(~[N: x|$x.a])->extend(over(~N), ~[agg:{p,w,r|$r.v}:y|$y->sum()])"
            .to_owned(),
        // lone `=` is not a comparison operator
        "|X.all()->filter(x|$x.a = 1)".to_owned(),
        "|db::Db->tableReference('default','T')->tableToTDS()\
                  ->filter(row: meta::pure::tds::TDSRow[1]|$row.getInteger('c') = 1)"
            .to_owned(),
        // block query requires the leading pipe
        "{X.all()->take(1)}".to_owned(),
        "{X.all()->take(1);}".to_owned(),
        "{ X.all()->take(1) }".to_owned(),
        // colon runs beyond a double colon die
        "|X:::Y.all()->take(1)".to_owned(),
        "|meta:::pure::Thing.all()->take(1)".to_owned(),
        "|X:5.all()->take(1)".to_owned(),
        "|X::5.all()->take(1)".to_owned(),
        "|meta:: pure::Thing.all()->take(1)".to_owned(),
        // a bare source classpath without a production dies
        "|X ".to_owned(),
        "|X".to_owned(),
        "|spider::geo::Db ".to_owned(),
        "|spider::geo::Db".to_owned(),
        "|X)".to_owned(),
        "|X-5.all()->take(1)".to_owned(),
        "|spider::geo::Db- ".to_owned(),
        "|X.all()->take(1)".to_owned(),
        "|spider::geo::Db->tableReference('default','T')->tableToTDS()->limit(1)".to_owned(),
        // a star outside a multiplicity bracket dies
        "|X.all()->take(*)".to_owned(),
        "|X.all()->take(1 + *)".to_owned(),
        "|X.all()->filter(x|$x.a > *)".to_owned(),
        "|X.all()->project([$x.a * *], ['c'])".to_owned(),
        "|db::Db->tableReference('default','T')->tableToTDS()\
         ->groupBy([], agg('C', row: meta::pure::tds::TDSRow[1]|$row, \
         y: meta::pure::tds::TDSRow[*]|$y->count()))"
            .to_owned(),
        // a brace lambda with a literal body dies
        format!("{join}{{1}})"),
        format!("{join}{{'x'}})"),
        format!("{join}{{%2018}})"),
        format!(
            "{join}{{r1: meta::pure::tds::TDSRow[1], r2: meta::pure::tds::TDSRow[1]|\
             $r1.getInteger('x') == $r2.getInteger('y')}})"
        ),
        // a block binding without let or with trailing junk dies
        "{|foo bar = X.all()->take(1);}".to_owned(),
        "{|X.all()->take(1) junk}".to_owned(),
        "{|let m = X.all()->take(1); $m->take(1) junk;}".to_owned(),
        "|a::Db->tableReference('default','A')->tableToTDS()->join(\
         a::Db->tableReference('default','B')->tableToTDS(), \
         meta::relational::metamodel::join::JoinType.INNER, \
         {r1: meta::pure::tds::TDSRow[1], r2: meta::pure::tds::TDSRow[1]|\
         $r1.getInteger('x') = $r2.getInteger('y')})"
            .to_owned(),
        "{|let m = X.all()->take(1); $m->take(1);}".to_owned(),
        "{|let a = X.all()->take(1); let b = Y.all()->take(1); $a->take(1);}".to_owned(),
        // whitespace inside a double colon dies
        "|meta: :pure::Thing.all()->take(1)".to_owned(),
        "|db::Db->tableReference('default','T')->tableToTDS()\
         ->filter(row: meta: :pure::tds::TDSRow[1]|$row.getInteger('c') == 1)"
            .to_owned(),
        "|db::Db->tableReference('default','T')->tableToTDS()\
         ->filter(row: meta::pure::tds::TDSRow[1]|$row.getInteger('c') == 1)"
            .to_owned(),
        // delimiter and source invariants hold together
        "|X.all()->take(2]".to_owned(),
        "|X.all())".to_owned(),
        "|X.all()->take(2".to_owned(),
    ];
    cases.sort();
    cases.dedup();
    cases
}

#[test]
fn precision_reject_corpus_is_equivalent_across_both_engines() {
    let fixed = fixed_grammar();
    let spec = spec_grammar();

    for text in precision_corpus() {
        assert_same_verdict(text.as_bytes(), &fixed, &spec);
    }
}

/// Randomized deterministic accepting walks of the fixed PDA's own
/// reachable-state graph (`tests/support/walker.rs`, already used by
/// `tests/mask_properties.rs`) — inputs no fixed corpus curated, so this
/// catches a transcription gap the gold/seed/precision corpora happen not to
/// exercise (deep nesting, multi-byte-operator/arm combinations). Every walk
/// is accepting under the fixed engine by construction; equivalence requires
/// the spec-compiled engine to agree exactly, not merely also accept.
#[test]
fn randomized_accepting_walks_are_equivalent_across_both_engines() {
    let fixed = fixed_grammar();
    let spec = spec_grammar();

    let walks = walker::generate_walks();
    assert_eq!(walks.len(), walker::WALK_COUNT, "walk generation count");
    for walk in &walks {
        assert_eq!(
            replay(walk, &fixed),
            Verdict::Complete,
            "a generated walk must be accepting under the fixed engine by construction"
        );
        assert_same_verdict(walk, &fixed, &spec);
    }
}
