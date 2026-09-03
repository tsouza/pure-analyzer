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
//!
//! Every corpus here is a set of *strings*, and a string only ever contains
//! bytes that continue it legally — which left the two engines free to disagree
//! about what they **reject**, invisibly (issue #55 Phase 9 walked into exactly
//! that). [`every_reachable_configuration_agrees_on_every_byte_across_both_engines`]
//! closes that off: it sweeps every reachable `(state, stack-top)` pair against
//! all 256 bytes, which is the transition relation in full. The corpora remain
//! for what they are good at — naming shapes a human can read.
#![forbid(unsafe_code)]

use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;

#[path = "support/corpus.rs"]
mod corpus;
#[path = "support/error.rs"]
mod error;
#[path = "support/walker.rs"]
mod walker;

use corpus::load_gold;
use purecard::grammar::EMITTED_SUBSET_SPEC;
use purecard::grammar::pda::Frame;
use purecard::{ByteRecognizer, CompiledGrammar, DecodeError, DecoderSession, Vocab};

/// The number of reachable `(state, stack-top)` pairs
/// [`every_reachable_configuration_agrees_on_every_byte_across_both_engines`]
/// sweeps. Pinned so a state or frame that silently stops being reachable — the
/// way a rule change can orphan one — reddens this suite instead of quietly
/// shrinking its coverage.
// Issue #361 added a `Frame::RelColBracket` and six states narrowing arm-R's
// `~[…]`/bare `~col` column-spec positions off the generic value hub — a real
// automaton change, reviewed in that PR, not a silent re-pin. (325 was the
// pre-#361 count, itself already grown from 323 by issue #352's own
// `let`-binder-value states.) Issue #368 added three more states
// (`AfterRelColColon`/`InRelColLambdaBinder`/`AfterRelColLambdaBinder`)
// narrowing arm-R's binder-after-colon position off the generic typed-binder
// `AfterColon` machinery — another real automaton change, not a re-pin. (407
// was the pre-#368 count.)
const EXPECTED_REACHABLE_CONFIGURATIONS: usize = 428;

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
/// Arm-C (class-navigation) modern-dialect seed count (mirrors
/// `tests/modern_dialect_soundness.rs`).
const SEED_ARM_C: usize = 8;
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
    CompiledGrammar::compile(Vocab::from_byte_tokens(Vec::new()))
}

fn spec_grammar() -> CompiledGrammar {
    CompiledGrammar::from_spec(EMITTED_SUBSET_SPEC, Vocab::from_byte_tokens(Vec::new()))
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

/// The value-position `::` strings issue #55 Phase 9 pinned, in their own
/// function for the same reason as [`phase_7_literal_and_binder_corpus`].
///
/// This family exists because the shape it covers found a real hole in *this*
/// suite: with `pda.rs` tightened and `emitted_subset.json` left untouched, the
/// two engines disagreed outright on `|Country.all()->filter(c|$c.name!=f()::a)`
/// (fixed `Dead(38)`, spec `Complete`) while every corpus above stayed green —
/// no gold, seed or precision string puts a `::` on a term that is not a name,
/// so nothing here could see the divergence.
///
/// These strings pin the *instance*, in the readable, shape-named form the rest
/// of this file is written in.
/// [`every_reachable_configuration_agrees_on_every_byte_across_both_engines`]
/// is what closes the *class* (constitution §5) — a curated corpus can always
/// be missing tomorrow's shape, and that is the defect this one exposed.
fn phase_9_value_position_classpath_corpus() -> Vec<String> {
    vec![
        // a `::` binds to a term-start name or a string literal
        "|X.all()->filter(c|$c.n!=mpg::getInteger)".to_owned(),
        "|X.all()->filter(c|$c.n!=meta::pure::tds::TDSRow)".to_owned(),
        "|X.all()->filter(c|$c.n!='europe'::makeId)".to_owned(),
        "|X.all()->filter(c|$c.n!='a b'::c)".to_owned(),
        "|X.all()->filter(c|$c.n!=mpg ::getInteger)".to_owned(),
        // and to nothing else — the exact shape that exposed the gap first
        "|X.all()->filter(c|$c.n!=f()::a)".to_owned(),
        "|X.all()->filter(c|$c.n!=[1]::a)".to_owned(),
        "|X.all()->filter(c|$c.n!=1::a)".to_owned(),
        "|X.all()->filter(c|$c.n!=%2018-03-17::a)".to_owned(),
        "|X.all()->filter(c|$c.n!=$x::a)".to_owned(),
        "|X.all()->filter(c|$c.n!=$x.foo::a)".to_owned(),
        "|X.all()->filter(c|$c.n!=x->getInteger()::a)".to_owned(),
        "|X.all()->filter(c|$c.n!=x->getInteger::a)".to_owned(),
        // the same colon with no binder slot open, where the refusal moves onto
        // the first `:` instead of the second
        "{|X.all():language*meta::pure::tds::TDSRow}".to_owned(),
        "|X.all():a".to_owned(),
        // the typed-binder arms the rule leaves in place
        "|X.all()->groupBy(~[K], ~'Agg': x|$x.v : y|$y->sum())".to_owned(),
        "|X.all()->project(~[N: x|$x.a])->extend(over(~N), ~[agg:{p,w,r|$r.v}:y|$y->sum()])"
            .to_owned(),
    ]
}

/// The `%`-literal and typed-binder strings issue #55 Phase 7 pinned, kept in
/// their own function so [`precision_corpus`] stays inside the workspace's
/// function-length lint rather than growing without bound as phases land.
fn phase_7_literal_and_binder_corpus() -> Vec<String> {
    vec![
        // milestoning literal is exactly the `%latest` keyword
        "|X.all()->take(%Latest)".to_owned(),
        "|X.all()->take(%latest1)".to_owned(),
        "|X.all()->take(%latestX)".to_owned(),
        "|X.all(%latest)->take(1)".to_owned(),
        "|X.all(%latest, %latest)->take(1)".to_owned(),
        "|X.all(%latestdate)->take(1)".to_owned(),
        "|X.all(%late)->take(1)".to_owned(),
        "|X.all(%a)->take(1)".to_owned(),
        "|X.all(%filter)->take(1)".to_owned(),
        "|X.all()->filter(x|$x.d == %latest)".to_owned(),
        // a typed binder's type is a classpath, a multiplicity, then a pipe
        "|X.all()->extend(getFloat:row)".to_owned(),
        "|X.all()->extend(a:b.c[1]|1)".to_owned(),
        "|X.all()->extend(a:b+1)".to_owned(),
        "|X.all()->extend(a:'b'|1)".to_owned(),
        "|X.all()->extend(a:b : c[1]|1)".to_owned(),
        "|X.all()->extend(a:b:::c[1]|1)".to_owned(),
        "|X.all()->extend(a:b:: c[1]|1)".to_owned(),
        "|X.all()->extend(a:b['europe']|1)".to_owned(),
        "|X.all()->extend(a:b[]|1)".to_owned(),
        "|X.all()->extend(a:b[**]|1)".to_owned(),
        "|X.all()->extend(a:b[1],c)".to_owned(),
        "|X.all()->extend(a:b[1]->foo())".to_owned(),
        "|X.all()->extend(a:b[1]&&1)".to_owned(),
        "|X.all()->extend(a:b[1]||1)".to_owned(),
        "|X.all()->extend(a:b||1)".to_owned(),
        "|X.all()->extend(a:b[1]|1)".to_owned(),
        "|X.all()->extend(a:b::c[1]|1)".to_owned(),
        "|X.all()->extend(a:b ::c[1]|1)".to_owned(),
        "|X.all()->extend(a :b [*]|1)".to_owned(),
        "|X.all()->extend(a:b[ 12 ] | 1)".to_owned(),
        "|X.all()->groupBy(~[a:x|$x.b],~'t':y|$y->sum())".to_owned(),
        // a `[` binds to a binder type and to nothing else
        "|X.all()->filter(x|$x.a[1] > 1)".to_owned(),
        "|X.all()->filter(x|foo[1] > 1)".to_owned(),
        "|X.all()->take(1)['a']".to_owned(),
        "|X.all()->extend(getFloat[1])".to_owned(),
        "|X.all()->extend(a:b [1]|1)".to_owned(),
    ]
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
        // an empty date literal dies, and so does one that opens on a separator
        "|X.all()->take(%)".to_owned(),
        "|X.all()->filter(x|$x.d < %)".to_owned(),
        "|X.all()->filter(x|$x.d < %-)".to_owned(),
        "|X.all()->filter(x|$x.d < %T)".to_owned(),
        "|X.all()->filter(x|$x.d < %:)".to_owned(),
        "|X.all(%->take(1))".to_owned(),
        "|X.all()->filter(x|$x.d < %1)".to_owned(),
        "|X.all()->filter(x|$x.d < %2018-03-17T07:13:53.000)".to_owned(),
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
    cases.extend(phase_7_literal_and_binder_corpus());
    cases.extend(phase_9_value_position_classpath_corpus());
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

/// The longest witness prefix [`reachable_configurations`] will extend. Every
/// `(state, stack-top)` pair the automaton has is reached well inside this;
/// the cap only stops the BFS from chasing an unbounded stack.
const MAX_WITNESS_LEN: usize = 64;

/// The `(state, stack-top)` pair that, with the byte, wholly determines the
/// fixed PDA's next move — the exact argument tuple of its `step` function, and
/// therefore the only axis along which the two engines can disagree at all.
type ConfigKey = (&'static str, &'static str);

/// The key `session` is currently in, or [`None`] once it has left the
/// byte-PDA (a state the recognizer no longer tracks a `Pda` for).
fn config_key(session: &DecoderSession<'_>) -> Option<ConfigKey> {
    let pda = session.pda()?;
    Some((pda.state().name(), pda.stack_top().map_or("-", Frame::name)))
}

/// Drive `prefix` through a fresh session over `grammar`, returning it if every
/// byte was accepted.
fn session_after<'g>(prefix: &[u8], grammar: &'g CompiledGrammar) -> Option<DecoderSession<'g>> {
    let mut session = DecoderSession::new(grammar);
    for &byte in prefix {
        session.accept_byte(byte).ok()?;
    }
    Some(session)
}

/// Breadth-first search of the fixed engine's reachable `(state, stack-top)`
/// pairs, returning the shortest witness byte string that reaches each.
///
/// Uses only the public recognizer surface — feed bytes, read
/// [`DecoderSession::pda`] — so it observes the shipped decoder, not an
/// internal table.
fn reachable_configurations(grammar: &CompiledGrammar) -> BTreeMap<ConfigKey, Vec<u8>> {
    let mut witnesses = BTreeMap::new();
    let mut queue = VecDeque::new();

    let start = DecoderSession::new(grammar);
    if let Some(key) = config_key(&start) {
        witnesses.insert(key, Vec::new());
        queue.push_back(Vec::new());
    }

    while let Some(prefix) = queue.pop_front() {
        for byte in 0..=u8::MAX {
            let Some(mut session) = session_after(&prefix, grammar) else {
                continue;
            };
            if session.accept_byte(byte).is_err() {
                continue;
            }
            let Some(key) = config_key(&session) else {
                continue;
            };
            if witnesses.contains_key(&key) {
                continue;
            }
            let mut next = prefix.clone();
            next.push(byte);
            witnesses.insert(key, next.clone());
            if next.len() < MAX_WITNESS_LEN {
                queue.push_back(next);
            }
        }
    }
    witnesses
}

/// **The gate that closes the class, not the instance** (issue #55 Phase 9,
/// constitution §5).
///
/// Every corpus above — gold, seed, precision, randomized walks — is a set of
/// *strings*, and every string only ever contains bytes that continue it
/// legally. That leaves the two engines free to disagree about which bytes are
/// **rejected**, and they did: Phase 9 tightened `pda.rs` without
/// `emitted_subset.json` and the suite stayed green while
/// `|Country.all()->filter(c|$c.name!=f()::a)` was `Dead` under one engine and
/// `Complete` under the other. Adding strings for that shape closes the
/// instance; it does not stop the next production from drifting the same way.
///
/// This does. The fixed PDA's move is a pure function of `(state, stack-top,
/// byte)`, so sweeping **every** reachable `(state, stack-top)` pair against
/// **every** one of the 256 bytes covers the transition relation exhaustively
/// along the only axis on which a transcription can differ. Push and pop
/// targets are covered too, because the configuration a move lands in is itself
/// a pair the search reaches. A divergence names the witness, the byte and the
/// two verdicts.
#[test]
fn every_reachable_configuration_agrees_on_every_byte_across_both_engines() {
    let fixed = fixed_grammar();
    let spec = spec_grammar();

    let witnesses = reachable_configurations(&fixed);
    assert_eq!(
        witnesses.len(),
        EXPECTED_REACHABLE_CONFIGURATIONS,
        "reachable (state, stack-top) pair count — a change here is a real \
         automaton change and wants its own review, never a re-pin"
    );

    for ((state, top), witness) in &witnesses {
        for byte in 0..=u8::MAX {
            let mut probe = witness.clone();
            probe.push(byte);
            let fixed_verdict = replay(&probe, &fixed);
            let spec_verdict = replay(&probe, &spec);
            assert_eq!(
                fixed_verdict,
                spec_verdict,
                "fixed and spec-compiled grammars diverge at ({state}, stack-top {top}) \
                 on byte {byte:#04x} ({:?}): fixed={fixed_verdict:?} spec={spec_verdict:?}\n  \
                 witness: {:?}",
                byte as char,
                String::from_utf8_lossy(witness)
            );
        }
    }
}
