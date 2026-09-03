//! The precision (negative) corpus — the pin no other gate can replace
//! (`docs/spec/grammar.md`; ADR-0004).
//!
//! Gold soundness (`tests/soundness_replay.rs`) proves the PDA *accepts* every
//! valid query; coverage and mutation observe which lines run and which mutants
//! die. **None of them can see over-acceptance** — an automaton that accepted
//! every byte string would pass all three identically. This suite is the missing
//! half: a curated set of malformed emitted-Pure strings that the recogniser MUST
//! reject, so a widening that reopens one of these structural holes reddens a PR
//! instead of silently passing.
//!
//! "Reject" is the exact negation of the soundness killer property: a string is
//! rejected when the real [`DecoderSession`] either hits a [`DecodeError`] on some
//! byte **or** ends the stream in a non-accepting (incomplete) state. Both are
//! genuine refusals — a decoder that never dead-ends but never completes has still
//! declined the string.
#![forbid(unsafe_code)]

#[path = "support/l1.rs"]
mod l1;

use l1::l1_grammar;
use purecard::{ByteRecognizer, DecodeError, DecoderSession};

/// Drive `text` through a fresh real [`DecoderSession`] and report whether the
/// recogniser refuses it — a mid-stream dead state, or an incomplete stream at
/// end-of-input. The mirror image of `soundness_replay::replay`.
fn dies(text: &str) -> bool {
    let grammar = l1_grammar();
    let mut session = DecoderSession::new(&grammar);
    for &byte in text.as_bytes() {
        if let Err(DecodeError::DeadState { .. }) = session.accept_byte(byte) {
            return true;
        }
    }
    !session.is_complete()
}

/// Sanity anchor: a well-formed query from each arm is *not* rejected, so `dies`
/// is discriminating and not vacuously true.
#[test]
fn well_formed_gold_shapes_are_not_rejected() {
    assert!(!dies("|X.all()->take(3)"));
    assert!(!dies(
        "|db::Db->tableReference('default','T')->tableToTDS()->limit(5)"
    ));
    assert!(!dies(
        "{|let m = X.all()->take(1); Y.all()->filter(b|$b.v == $m)->take(1);}"
    ));
}

/// A query is a source pipeline, never a bare value (findings A/B: `|42`, `|*`,
/// `|( )` reached [`AfterValue`] and were accepted as complete).
#[test]
fn a_top_level_source_must_be_an_identifier() {
    assert!(dies("|42"));
    assert!(dies("|42 "));
    assert!(dies("|*"));
    assert!(dies("|( )"));
    assert!(dies("|'lit'"));
    assert!(dies("|%2018-03-17"));
    assert!(dies("|$x->take(1)"));
}

/// A completed term must be followed by a connector/operator/closer, never a bare
/// abutting identifier — the headline missing-`->` hole (findings A/B).
#[test]
fn a_completed_term_is_not_followed_by_a_bare_identifier() {
    assert!(dies("|foo bar baz"));
    assert!(dies("|foo bar baz "));
    assert!(dies("|X.all() take(3)"));
    assert!(dies("|X.all()->take(1) take(2)"));
    assert!(dies("|X.all()->filter(nonsense garbage here)"));
}

/// A binary operator demands an operand; a closer may not follow it (finding D).
#[test]
fn a_dangling_operator_before_a_closer_dies() {
    assert!(dies("|X.all()->take(1 +)"));
    assert!(dies("|X.all()->take(1 -)"));
    assert!(dies("|X.all()->take(1 *)"));
    assert!(dies("|X.all()->filter(x|$x.a && )"));
    assert!(dies("|X.all()->filter(x|$x.a || )"));
    assert!(dies("|X.all()->filter(x|$x.a > )"));
    assert!(dies("|X.all()->filter(x|$x.a == )"));
}

/// Numeric literals must be well-formed: a sign needs a digit, a `.` needs a
/// fractional digit, and a doubled sign is invalid (finding E).
#[test]
fn malformed_numeric_literals_die() {
    assert!(dies("|X.all()->take(-)"));
    assert!(dies("|X.all()->take(1.)"));
    assert!(dies("|X.all()->take(--5)"));
    // A bare `.` with no fractional digit, and an exponent with no digit, still die.
    assert!(dies("|X.all()->filter(x|$x.a > .)"));
    assert!(dies("|X.all()->filter(x|$x.a > 1.5e)"));
    // NB: `-.5` (leading-dot float) and `.5` are engine-legal literals (Legend
    // 4.113.0), so they are admitted, not rejected — see
    // `pda::tests::extended_numeric_and_date_literals_stream`.
}

/// A date literal must open on a **digit** (finding F, tightened in issue #55
/// Phase 7). A bare `%` dies, and so does one whose first byte is a `-`/`T`/`:`
/// date *separator*: those are interior bytes only, and the pinned engine
/// answers "no viable alternative at input '…<%'" for each.
#[test]
fn a_date_literal_opens_on_a_digit() {
    assert!(dies("|X.all()->take(%)"));
    assert!(dies("|X.all()->filter(x|$x.d < %)"));
    assert!(dies("|X.all()->filter(x|$x.d < %-)"));
    assert!(dies("|X.all()->filter(x|$x.d < %T)"));
    assert!(dies("|X.all()->filter(x|$x.d < %:)"));
    assert!(dies("|X.all(%->take(1))"));
    // …while every digit-opened literal the engine parses still streams, down to
    // a bare year run.
    assert!(!dies("|X.all()->filter(x|$x.d < %1)"));
    assert!(!dies("|X.all()->filter(x|$x.d < %2018-03-17)"));
    assert!(!dies("|X.all()->filter(x|$x.d < %2018-03-17T07:13:53.000)"));
}

/// The symbolic milestoning literal is **exactly `%latest`** — Legend 4.113.0
/// lexes that one `LATEST_DATE` token and knows no other `%`-plus-letters symbol.
/// Every proper prefix, extension, and neighbour of the keyword dies, so the
/// production admits the real symbol without opening `%<lowercase>+`.
///
/// `%latestdate` sits on the *rejected* side deliberately, and that flip is what
/// issue #55 Phase 7 established: the pinned engine answers "no viable
/// alternative at input '.all(%latestdate'", and the seed that used to assert L1
/// admits it was itself wrong (`corpus/modern_dialect_seeds.jsonl`'s
/// `issue-55/g2-latest-corrected:4`).
#[test]
fn a_milestoning_literal_is_exactly_the_latest_symbol() {
    // Bare `%` still dies (shared with the date-literal pin above).
    assert!(dies("|X.all()->take(%)"));
    // Uppercase first byte after `%` is not a milestone symbol.
    assert!(dies("|X.all()->take(%Latest)"));
    // Neither a proper prefix of the keyword nor an extension of it survives.
    assert!(dies("|X.all(%late)->take(1)"));
    assert!(dies("|X.all(%latestdate)->take(1)"));
    assert!(dies("|X.all()->take(%latest1)"));
    assert!(dies("|X.all()->take(%latestX)"));
    // Nor does any other `%`-prefixed lowercase run the walk set used to reach.
    assert!(dies("|X.all(%a)->take(1)"));
    assert!(dies("|X.all(%filter)->take(1)"));
    assert!(dies("|X.all(%limit)->take(1)"));
    // …but the real milestone literal streams, in both of the engine's own
    // milestoning argument slots.
    assert!(!dies("|X.all(%latest)->take(1)"));
    assert!(!dies("|X.all(%latest, %latest)->take(1)"));
    assert!(!dies(
        "|X.all()->filter(x|$x.facet(%latest, %latest).level == 'a')->take(1)"
    ));
}

/// A typed binder's right-hand side owes a lambda, so only its own `::`
/// classpath continuation, its multiplicity `[`, and the pipe may follow it —
/// and the multiplicity bracket holds a `mult` and nothing else. Every string
/// here is a live engine "no viable alternative" / "Unexpected token" against
/// the pinned stack (issue #55 Phase 7).
#[test]
fn a_typed_binder_type_that_is_not_a_classpath_multiplicity_pipe_dies() {
    for text in [
        "|X.all()->extend(getFloat:row)",
        "|X.all()->extend(a:b.c[1]|1)",
        "|X.all()->extend(a:b+1)",
        "|X.all()->extend(a:b/1)",
        "|X.all()->extend(a:'b'|1)",
        "|X.all()->extend(a:b : c[1]|1)",
        "|X.all()->extend(a:b:::c[1]|1)",
        "|X.all()->extend(a:b['europe']|1)",
        "|X.all()->extend(a:b[]|1)",
        "|X.all()->extend(a:b[**]|1)",
        "|X.all()->extend(a:b[1],c)",
        "|X.all()->extend(a:b[1]->foo())",
        "|X.all()->extend(a:b[1]&&1)",
        "|X.all()->extend(a:b[1]||1)",
        "|X.all()->extend(a:b||1)",
    ] {
        assert!(dies(text), "the recogniser still streams {text:?}");
    }
}

/// A `[` binds to a typed binder's type and to nothing else. Legend has no
/// positional index at all — it says so in as many words ("Bracket operation is
/// not supported") — and the multiplicity is the emitted subset's only `[`-after
/// -a-name. Pinned here because the arm that used to admit it off *any*
/// identifier became dead the moment the binder grew its own multiplicity chain,
/// and a dead arm is a hole nothing else can see (issue #55 Phase 7).
#[test]
fn a_bracket_off_anything_but_a_binder_type_dies() {
    for text in [
        "|X.all()->filter(x|$x.a[1] > 1)",
        "|X.all()->filter(x|foo[1] > 1)",
        "|X.all()->take(1)['a']",
        "|X.all()->extend(getFloat[1])",
    ] {
        assert!(dies(text), "the recogniser still streams {text:?}");
    }
    // The whitespace a `::`-bearing path may resume across is a *classpath* gap,
    // not a name boundary: a second bare name across it is a dead state, live
    // ("no viable alternative at input '…extend(a:b::cd'").
    assert!(dies("|X.all()->extend(a:b::c d[1]|1)"));
    // …while the multiplicity itself, spaced or not, still streams.
    assert!(!dies("|X.all()->extend(a:b[1]|1)"));
    assert!(!dies("|X.all()->extend(a:b [1]|1)"));
}

/// The other half of the pin: every binder shape the engine *does* parse still
/// streams, so the tightening above cannot have swallowed the production.
#[test]
fn every_engine_legal_typed_binder_still_streams() {
    for text in [
        "|db::Db->tableReference('default','T')->tableToTDS()\
         ->filter(row: meta::pure::tds::TDSRow[1]|$row.getInteger('c') > 1)",
        "|X.all()->extend(a:b[1]|1)",
        "|X.all()->extend(a:b::c[1]|1)",
        "|X.all()->extend(a:b ::c[1]|1)",
        // A `::`-bearing path may resume across whitespace at *any* segment, not
        // only the first — live-attested, as is `row: meta::pure ::tds::TDSRow[1]`.
        "|X.all()->extend(a:b::c ::d[1]|1)",
        "|X.all()->filter(row: meta::pure ::tds::TDSRow[1]|$row)",
        "|X.all()->extend(a :b [*]|1)",
        "|X.all()->extend(a:b[ 12 ] | 1)",
        "|X.all()->groupBy(~[a:x|$x.b],~'t':y|$y->sum())",
        // Arm-R's column binding, spaced: its variable owes no multiplicity, so
        // the pipe follows the *name past its whitespace* rather than a closed
        // `[…]`. Live-attested, as is the two-colon aggregate form.
        "|X.all()->project(~[Total: y |$y->sum()])",
        "|X.all()->groupBy(~[],~'G': x |$x.v : y |$y->sum())",
        "|X.all()->filter(row : meta::pure::tds::TDSRow [1] |$row)",
        "|a::Db->tableReference('default','A')->tableToTDS()->join(\
         a::Db->tableReference('default','B')->tableToTDS(), \
         meta::relational::metamodel::join::JoinType.INNER, \
         {r1: meta::pure::tds::TDSRow[1], r2: meta::pure::tds::TDSRow[1]|\
         $r1.getInteger('x') == $r2.getInteger('y')})",
    ] {
        assert!(!dies(text), "the recogniser refuses engine-legal {text:?}");
    }
}

/// The counterfactual for the three tightenings issue #55 Phase 8 shipped: every
/// shape the engine *does* parse must still stream, so none of them swallowed a
/// production. Each line was probed against the pinned engine on the branch.
#[test]
fn every_engine_legal_date_group_and_pipe_still_streams() {
    for text in [
        // A date literal: a bare year run, a date, and a full timestamp whose
        // fractional seconds follow the seconds field.
        "|X.all(%1)",
        "|X.all(%1974)",
        "|X.all(%1974-1-1)",
        "|X.all(%2018-03-17T07:13:53.000)",
        // A `-` in the *time* half opens a timezone offset, not a date field.
        "|X.all(%2018-03-17T07:13:53-0500)",
        "|X.all(%20:18-3)",
        "|X.all(%latest, %latest)",
        // A parenthesised group: one expression, and every nesting it may hold.
        "|X.all()->limit((1))",
        "|X.all()->limit(((1)))",
        "|X.all()->limit(('a'))",
        "|X.all()->limit(([1,2]))",
        "|X.all()->limit((x|1))",
        "|X.all()->limit((a:b[1]|1))",
        "|X.all()->limit((a::b))",
        // A lambda pipe off a name or a string, including the one that reads as
        // a bare name only because it is an operand.
        "|X.all()->filter(x|1)",
        "|X.all()->filter('a'|1)",
        "|X.all()->filter(a&&b|1)",
        // …and the boolean `||`, which is what a pipe off any *other* term is.
        "|X.all()->filter(f()||1)",
        "|X.all()->filter($x.a||1)",
        "|X.all()->filter([1]||1)",
        // Arm-R's column binding still has no multiplicity: it is a bare
        // variable, never a `::` path, which is what keeps the two apart.
        "|X.all()->project(~[Total: y|$y->sum()])",
        "|X.all()->groupBy(~[],~'G': x|$x.v : y|$y->sum())",
        "|X.all()->extend(over(~a),~[agg:{p,w,r|$r.n}:y|$y->sum()])",
    ] {
        assert!(!dies(text), "the recogniser refuses engine-legal {text:?}");
    }
}

/// The arm-R `~` sigil (gap report G1) must be followed by a column-set `~[`, a
/// bare column reference `~ident`, or a quoted `~'…'`. A dangling `~`, a spaced
/// `~ [`, a doubled `~~`, or a `~` in source position all die — the widening
/// admits the Relation/Function API without opening a bare `~`.
#[test]
fn a_tilde_sigil_must_open_a_column_set_or_reference() {
    assert!(dies("|X.all()->project(~)"));
    assert!(dies("|X.all()->project(~ [Col: x|$x.a])"));
    assert!(dies("|X.all()->project(~~[Col: x|$x.a])"));
    assert!(dies("|X.all()->sort([ascending(~)])"));
    // `~` is not a legal pipeline source.
    assert!(dies("|~.all()->take(1)"));
    // …but the real arm-R constructs stream (column-set, bare ref, quoted ref).
    assert!(!dies("|X.all()->project(~[Col: x|$x.a])"));
    assert!(!dies(
        "|X.all()->groupBy(~[K], ~'Agg': x|$x.v : y|$y->sum())"
    ));
    assert!(!dies("|X.all()->sort([ascending(~A), descending(~B)])"));
    assert!(!dies(
        "|X.all()->project(~[N: x|$x.a])->extend(over(~N), ~[agg:{p,w,r|$r.v}:y|$y->sum()])"
    ));
}

/// A lone `=` is not a comparison operator; only `==` compares, and a single `=`
/// lives only in a block-query `let` binder (finding G).
#[test]
fn a_single_equals_as_a_comparison_operator_dies() {
    assert!(dies("|X.all()->filter(x|$x.a = 1)"));
    assert!(dies(
        "|db::Db->tableReference('default','T')->tableToTDS()\
                  ->filter(row: meta::pure::tds::TDSRow[1]|$row.getInteger('c') = 1)"
    ));
}

/// A block query is `{|…}`; the leading pipe is not optional (finding I).
#[test]
fn a_block_query_without_the_leading_pipe_dies() {
    assert!(dies("{X.all()->take(1)}"));
    assert!(dies("{X.all()->take(1);}"));
    assert!(dies("{ X.all()->take(1) }"));
}

/// Only `::` (classpath) and a single typed-binder `:` are valid; a tripled colon
/// is not (finding J).
#[test]
fn colon_runs_beyond_a_double_colon_die() {
    assert!(dies("|X:::Y.all()->take(1)"));
    assert!(dies("|meta:::pure::Thing.all()->take(1)"));
    // A `:` (single or `::`) demands an identifier segment, never a digit, and a
    // `::` separator carries no interior whitespace.
    assert!(dies("|X:5.all()->take(1)"));
    assert!(dies("|X::5.all()->take(1)"));
    assert!(dies("|meta:: pure::Thing.all()->take(1)"));
}

/// A pipeline source is a classpath that must be *produced* — followed by `.all()`,
/// an arm-A `->tableReference(…)` envelope, or a `::` classpath continuation. A bare
/// classpath (`|X `) or one abutting a value-completing delimiter never accepts
/// (finding: source must be followed by `.all()`/`->`).
#[test]
fn a_bare_source_classpath_without_a_production_dies() {
    assert!(dies("|X "));
    assert!(dies("|X"));
    assert!(dies("|spider::geo::Db "));
    assert!(dies("|spider::geo::Db"));
    assert!(dies("|X)"));
    // A `-` in source position must open `->`, never arithmetic minus.
    assert!(dies("|X-5.all()->take(1)"));
    assert!(dies("|spider::geo::Db- "));
    assert!(!dies("|X.all()->take(1)"));
    assert!(!dies(
        "|spider::geo::Db->tableReference('default','T')->tableToTDS()->limit(1)"
    ));
}

/// A `*` is only ever a `[*]` multiplicity token; it is never an arithmetic or
/// argument value (finding: keep `*` in multiplicity context).
#[test]
fn a_star_outside_a_multiplicity_bracket_dies() {
    assert!(dies("|X.all()->take(*)"));
    assert!(dies("|X.all()->take(1 + *)"));
    assert!(dies("|X.all()->filter(x|$x.a > *)"));
    assert!(dies("|X.all()->project([$x.a * *], ['c'])"));
    // …but the typed-binder `[*]` multiplicity still streams.
    assert!(!dies(
        "|db::Db->tableReference('default','T')->tableToTDS()\
         ->groupBy([], agg('C', row: meta::pure::tds::TDSRow[1]|$row, \
         y: meta::pure::tds::TDSRow[*]|$y->count()))"
    ));
}

/// A `join` brace lambda must begin with a typed binder identifier; a literal body
/// (`{1}`) is not a lambda (finding: require brace-lambda structure).
#[test]
fn a_brace_lambda_with_a_literal_body_dies() {
    let join = "|a::Db->tableReference('default','A')->tableToTDS()->join(\
                a::Db->tableReference('default','B')->tableToTDS(), \
                meta::relational::metamodel::join::JoinType.INNER, ";
    assert!(dies(&format!("{join}{{1}})")));
    assert!(dies(&format!("{join}{{'x'}})")));
    assert!(dies(&format!("{join}{{%2018}})")));
    // …but a real typed-binder brace lambda still streams.
    assert!(!dies(&format!(
        "{join}{{r1: meta::pure::tds::TDSRow[1], r2: meta::pure::tds::TDSRow[1]|\
         $r1.getInteger('x') == $r2.getInteger('y')}})"
    )));
}

/// A block-query binding is `let name = pipeline`; the `let` keyword is mandatory,
/// and no bare identifier may abut a completed statement (finding: track the
/// block-binding phase, do not accept any adjacent identifier under a brace).
#[test]
fn a_block_binding_without_let_or_with_trailing_junk_dies() {
    // Two bare identifiers then `=` — a binding missing its `let` keyword.
    assert!(dies("{|foo bar = X.all()->take(1);}"));
    // A completed pipeline followed by a stray identifier before the close.
    assert!(dies("{|X.all()->take(1) junk}"));
    assert!(dies("{|let m = X.all()->take(1); $m->take(1) junk;}"));
    // A single `=` inside a brace lambda body is a comparison typo, not a `let`.
    assert!(dies(
        "|a::Db->tableReference('default','A')->tableToTDS()->join(\
         a::Db->tableReference('default','B')->tableToTDS(), \
         meta::relational::metamodel::join::JoinType.INNER, \
         {r1: meta::pure::tds::TDSRow[1], r2: meta::pure::tds::TDSRow[1]|\
         $r1.getInteger('x') = $r2.getInteger('y')})"
    ));
    // …but real single- and multi-`let` blocks still stream.
    assert!(!dies("{|let m = X.all()->take(1); $m->take(1);}"));
    assert!(!dies(
        "{|let a = X.all()->take(1); let b = Y.all()->take(1); $a->take(1);}"
    ));
}

/// A `::` classpath separator must be contiguous; whitespace between the two colons
/// (`meta: :pure`) is a dead state, in both source and typed-binder position
/// (finding: reject whitespace inside `::`).
#[test]
fn whitespace_inside_a_double_colon_dies() {
    // Source-position classpath.
    assert!(dies("|meta: :pure::Thing.all()->take(1)"));
    // Typed-binder-position classpath (inside a filter lambda header).
    assert!(dies(
        "|db::Db->tableReference('default','T')->tableToTDS()\
         ->filter(row: meta: :pure::tds::TDSRow[1]|$row.getInteger('c') == 1)"
    ));
    // …but a typed-binder `:` with trailing whitespace before the type still streams.
    assert!(!dies(
        "|db::Db->tableReference('default','T')->tableToTDS()\
         ->filter(row: meta::pure::tds::TDSRow[1]|$row.getInteger('c') == 1)"
    ));
}

/// Structural closers still honour the frame stack and the source rule together —
/// a spot check that the tightenings did not reopen the delimiter invariants.
#[test]
fn delimiter_and_source_invariants_hold_together() {
    // Crossed closer under the new source rule.
    assert!(dies("|X.all()->take(2]"));
    // Unmatched trailing closer.
    assert!(dies("|X.all())"));
    // Unclosed call — incomplete, not dead.
    assert!(dies("|X.all()->take(2"));
}

/// The byte offset at which the recogniser refuses `text`, or [`None`] when it
/// streams to the end (whether or not it ends complete).
fn dead_offset(text: &str) -> Option<usize> {
    let grammar = l1_grammar();
    let mut session = DecoderSession::new(&grammar);
    for (offset, &byte) in text.as_bytes().iter().enumerate() {
        if let Err(DecodeError::DeadState { .. }) = session.accept_byte(byte) {
            return Some(offset);
        }
    }
    None
}

/// Assert `text` is refused **on the byte that opens `at`** — the substring the
/// rule under test is the one to reject.
///
/// The L1 twin of `l2_precision::assert_walk_is_masked`'s `closed_by`, and for
/// the same reason (issue #55, Phases 1-3): a frozen walk that merely "dies
/// somewhere" stops being evidence the moment a *different*, earlier rule takes
/// over its kill — the fixture keeps passing while the rule it was written for
/// quietly loses its only walk-level coverage. Naming the rejecting byte makes
/// that takeover redden this fixture at the moment it happens. `at` must occur
/// exactly once, so the anchor cannot drift either.
fn assert_dies_at(text: &str, at: &str) {
    assert_eq!(
        text.matches(at).count(),
        1,
        "the fixture anchor {at:?} is not unique in:\n  {text}"
    );
    let want = text.find(at).expect("anchor occurs");
    match dead_offset(text) {
        None => panic!("PRECISION GAP: the recogniser still streams past {at:?} in:\n  {text}"),
        Some(got) if got == want => {}
        Some(got) => panic!(
            "the walk was refused at byte {got} ({:?}), not on the {at:?} the rule under test \
             is supposed to reject:\n  {text}",
            &text[got..text.len().min(got + 12)]
        ),
    }
}

/// A block query's statements are `;`-separated, so a `,` at its statement level
/// has no element list to separate. Every walk here came verbatim out of the
/// live lane (issue #55 Phase 4) with the engine's own "Unexpected token ','.
/// Valid alternatives: \['&&', '||', '==', '!=', '->', '\[', '.', ';', '+', '*',
/// '-', '/', '<', '<=', '>', '>='\]" — note the absence of `,` and of `(`.
#[test]
fn a_block_query_statement_level_comma_dies() {
    for (walk, at) in [
        // world_1
        (
            "    \n      {\n  \n    \n    \n         |        spider::world_1::model::default::Countrylanguage.\n    \n    all(\n      ),'Language_T2'}",
            ",'Language_T2'}",
        ),
        // world_1
        (
            "     {\n           \n      |    spider::world_1::model::default::Countrylanguage.\n      \n          \n          \n        \n\n        \n        \n        \n      \n    all()<'GovernmentForm_T1_1'&&'Capital_T3_1'\n    ,'Code2_T1_3'}",
            ",'Code2_T1_3'}",
        ),
        // world_1
        (
            "    \n        \n        \n  {\n    \n     |\n  spider::world_1::model::default::Countrylanguage.    \n  \n      all()+'Republic','District_T3'('Code_T1_1'\n\n        *getFloat|'Region_T1_1'\n        )>'CountryCode_T2_2'\n    -'_v__t0sc0'}",
            ",'District_T3'",
        ),
        // car_1
        (
            "    \n      {\n  \n    \n    \n         |        spider::car_1::model::default::CarMakers.\n    \n    all(\n      ),'Make_t3_5'<'Continent_T2_2'}",
            ",'Make_t3_5'",
        ),
    ] {
        assert_dies_at(walk, at);
    }
}

/// A `|` that follows a completed term is a lambda binder pipe, and a lambda
/// needs an argument or element slot to sit in. At a block query's statement
/// level the query body is already open, so a second, bodiless pipe is a dead
/// state — live-attested on all four walks below. The anchor names the byte
/// *after* the pipe: the automaton consumes a `|` into its
/// `SawPipe` lookahead state (a `||` is still legal there), so the refusal lands
/// on the byte that would have opened the lambda body.
#[test]
fn a_lambda_binder_pipe_at_a_block_statement_level_dies() {
    for (walk, at) in [
        // world_1
        (
            "  \n        \n         {     \n        \n      \n    |spider::world_1::Db->renameColumns('Aruba'&&['GNPOld_T3_1'!='Continent_T1'!=.3000])|'SurfaceArea'}",
            "'SurfaceArea'}",
        ),
        // car_1
        (
            "   \n  {\n    \n        \n    |spider::car_1::Db->tableReference('MIN(Weight)'\n      )\n        /String.row|'Country_t1_3'.'CountryId_T1_2'}",
            "'Country_t1_3'.",
        ),
        // car_1
        (
            "            \n    \n      \n  \n        {\n  \n        \n        \n    |spider::car_1::Db->count('Edispl'.'150')|'AVG(Weight)'!='Weight_T1_1'&&8,'Year_t1'}",
            "'AVG(Weight)'",
        ),
        // car_1
        (
            "  \n      \n    \n\n  \n\n\n       \n  \n      {\n  \n        \n    \n        \n       \n  \n|spider::car_1::Db->tableReference('1970'=='Model_T2_2'*toOne. fk2DefaultCarMakers||'FullName_T1_1'=='_v__t0sc0').exists|'Maker_t1'}",
            "'Maker_t1'}",
        ),
    ] {
        assert_dies_at(walk, at);
    }
}

/// The symbolic milestoning literal is the `%latest` keyword and nothing else,
/// so every other `%`-plus-lowercase run the walk set used to reach is now a
/// dead state. All five walks came verbatim out of the live lane (issue #55
/// Phase 7) with the engine's own "no viable alternative at input '.all(%'" —
/// which it answers for `%a`, `%filter`, `%limit` and `%latestdate` alike. The
/// anchor is the byte that *diverges* from the keyword, so a chain link removed
/// or mis-spelled moves it and reddens the fixture.
#[test]
fn a_milestone_symbol_other_than_latest_dies() {
    for (walk, at) in [
        // world_1 (live walk 9)
        (
            "|spider::world_1::model::default::Countrylanguage.\nall(\n    %l<code:    b!=   \n    \n   \n      \n        \n         \n\n      \n    \n  \n      \n%160000)",
            "<code:    b!= ",
        ),
        // world_1 (live walk 53)
        (
            "\n        \n      \n  \n|spider::world_1::model::default::Country.\n          \n         \n  \n      \n\n  all(%a)",
            "a)",
        ),
        // car_1 (live walk 39)
        (
            "|spider::car_1::model::default::CarMakers.\n\nall(%spider::car_1::model::default::CarMakers&&\n    \n  \n   \n  \n      \n%m!=  \n     \n    \n    \n    \n  \n    \n      \n  \n      \n  \n      \n            \n        \n          %col)",
            "spider::car_1::model::default::CarMakers&",
        ),
        // car_1 (live walk 54)
        (
            "|spider::car_1::model::default::CarsData.  \n    \n  \n            \n      \n         \n    all(%filter)",
            "filter)",
        ),
        // car_1 (live walk 56)
        (
            "\n        \n      \n  \n|spider::car_1::model::default::CarMakers.\n          \n         \n  \n      \n\n  all(%limit.'AVG(Weight)',\n  \n      \n\n      \n       \n      %x\n  <=\n        \n      %between    \n        .'CountryName_T1'    )",
            "imit.'AVG(Weig",
        ),
    ] {
        assert_dies_at(walk, at);
    }
}

/// A typed binder's right-hand side is a classpath and then its multiplicity or
/// its pipe — never a navigation, an operator, or a closer. Four walks verbatim
/// from the live lane (issue #55 Phase 7); the engine rejects each with
/// "Unexpected token" naming exactly the byte anchored here.
#[test]
fn a_typed_binder_type_that_is_not_a_classpath_dies() {
    for (walk, at) in [
        // world_1 (live walk 61)
        (
            "    \n        \n    \n       \n      \n  \n            \n          \n      \n    \n    {|\n          \n  \n  spider::world_1::Db->tableReference('Population_t3'\n      ,'Continent')=='MAX(Percentage)'\n      &&row1.row1\n('Population_T3'=='GovernmentForm'!='Population_t3'==160000=='LifeExpectancy_t1'->renameColumns('_ord0'||extend\n  |language:fk1DefaultCountry.row  ['Region_T1_3'\n  ] ->max('CountryCode_T4_2'),if:tableReference->agg('CountryCode_T2'\n+'name'|'Continent_t1'>'Region_T1_3'))<'Name_T1_3'&&min    :concatenate&&'AVG(SurfaceArea)'<  asc|countryCode.'CountryCode_T2'\n    );}",
            ".row  ['Region",
        ),
        // world_1 (live walk 63)
        (
            "  \n    \n\n      {\n\n\n     \n         \n  \n      \n      \n        \n      \n \n          |spider::world_1::model::default::Countrylanguage.\nall(\n)->count()->sort('Name_t1'&&'Percentage_T2_2'    &&'CountryCode_city'>'LifeExpectancy_t1'<'GovernmentForm_T3_1'>'Percentage_T2'!=']==}INNER!name)[.)Float}spider::world_1::model::default::Countrylanguage}row%max*)a&5    '&&row1('_c0__t0r0'.'LifeExpectancy_T1':meta::pure::tds::TDSRow,'Capital_T1'!='asia'\n    \n+\n  fk1DefaultCountrylanguage.b\n        &&'_nn')->extend('ID_T2'))  !=('Code2_T1'->count()+'CountryCode_T2')||fk1DefaultCountrylanguage}",
            ",'Capital_T1'!",
        ),
        // car_1 (live walk 35)
        (
            "\n  \n          \n          { \n  \n      \n        |spider::car_1::model::default::CarMakers.\n      \n           all(\n  \n        )\n    ->groupBy(desc('Horsepower_T1'=='FullName_t1_1'||model/all:id/'model_list'    !=isNotEmpty:parseFloat    ==tableToTDS(id.'FullName_T2_3'    !='Id_T2_3'\n\n) &&countryId>='Weight_T4')  )+a||_    ::cylinders}",
            "/'model_list' ",
        ),
        // car_1 (live walk 44)
        (
            "  \n      {\n  \n    \n    \n         |        spider::car_1::model::default::CarMakers.\n    \n    all(\n      )\n        ->extend(getFloat:row)+'Country_T3'<'MakeId_T2_2'>'Weight_T1'}",
            ")+'Country_T3'",
        ),
    ] {
        assert_dies_at(walk, at);
    }
}

/// A binder type that has taken a `::` is a **package path**, which settles the
/// one ambiguity a bare binder type carries: it is not an arm-R column binding's
/// variable, so the multiplicity Legend requires of a typed binder is mandatory
/// and the lambda pipe cannot follow the type directly. Live-attested on this
/// branch — `->filter(row: meta::pure::tds::TDSRow|1)` and `->extend(a:b::c|1)`
/// are each "no viable alternative at input '…|'", while the same walks with a
/// `[1]` in front of the pipe parse.
///
/// The first two walks came to this fixture from
/// `a_binder_that_owes_its_lambda_pipe_dies` (issue #55 Phase 7), whose rule used
/// to take their kill one byte later, at the second `|` of the `||`; this rule
/// now rejects the first. That is a takeover, so the walks move with it rather
/// than having their anchors quietly re-pointed — the Phase 7 rule keeps its own
/// walk (a multiplicity-bearing binder, which this rule cannot reach) below.
#[test]
fn a_binder_type_classpath_that_skips_its_multiplicity_dies() {
    for (walk, at) in [
        // world_1 (issue #55 Phase 7 live walk 63)
        (
            "  \n    \n\n      {\n\n\n     \n         \n  \n      \n      \n        \n      \n \n          |spider::world_1::model::default::Countrylanguage.\nall(\n)->count()->sort('Name_t1'&&'Percentage_T2_2'    &&'CountryCode_city'>'LifeExpectancy_t1'<'GovernmentForm_T3_1'>'Percentage_T2'!=']==}INNER!name)[.)Float}spider::world_1::model::default::Countrylanguage}row%max*)a&5    '&&row1('_c0__t0r0'.'LifeExpectancy_T1':meta::pure::tds::TDSRow\n      \n||x.'Name_t1'\n+\n  fk1DefaultCountrylanguage.b\n        &&'_nn')->extend('ID_T2'))  !=('Code2_T1'->count()+'CountryCode_T2')||fk1DefaultCountrylanguage}",
            "||x.'Name_t1'",
        ),
        // car_1 (issue #55 Phase 7 live walk 35)
        (
            "\n  \n          \n          { \n  \n      \n        |spider::car_1::model::default::CarMakers.\n      \n           all(\n  \n        )\n    ->groupBy(desc('Horsepower_T1'=='FullName_t1_1'||model/all:id::col||desc>='Make_t2'<parseFloat|'Horsepower_T1_1'!=min.'FullName_T2_3'    !='Id_T2_3'\n\n) &&countryId>='Weight_T4')<asc.'Model_t1'}",
            "||desc>='Make_t",
        ),
        // car_1 (issue #55 Phase 8 live walk 42)
        (
            "  \n      {\n  \n    \n    \n         |        spider::car_1::model::default::CarMakers.\n    \n    all(\n      )\n        ->max(getFloat:row \n      ::weight\n    |'Model_T1_1'<'Horsepower_T2')\n      }",
            "|'Model_T1_1'",
        ),
    ] {
        assert_dies_at(walk, at);
    }
}

/// Once a binder's colon is open the binder owes exactly one pipe: a second `|`
/// is neither a boolean `||` (the binder is not an operand) nor a zero-arg
/// lambda. Verbatim from the live lane's second Phase 7 measurement, rejected as
/// "Unexpected token '||'. Valid alternatives: \['|'\]".
#[test]
fn a_binder_that_owes_its_lambda_pipe_dies() {
    // car_1 (live walk 45)
    assert_dies_at(
        "  \n      {\n  \n    \n    \n         |        spider::car_1::model::default::CarMakers.\n    \n    all(\n      )\n        ->extend(getFloat:row[    2\n        ]    \n        \n  \n  \n        ||desc\n      )}",
        "|desc\n      )}",
    );
}

/// A date literal's fractional seconds belong to its **time** half, and the
/// literal ends on a digit. Live-attested on this branch: `%1974.5`, `%0.0` and
/// `%2018-03-17.000` are each "no viable alternative at input", as are `%2018-`,
/// `%2018-03-17T` and `%2018-03-17T07:` — while `%1974`, `%1974-1-1` and
/// `%2018-03-17T07:13:53.000` all parse. The three walks are verbatim from the
/// live lane (issue #55 Phase 8); the anchor is the byte *after* the `.`, because
/// the `.` itself is still a legal navigation dot off a completed literal.
#[test]
fn a_date_literal_whose_fraction_has_no_seconds_dies() {
    for (walk, at) in [
        // car_1 (live walk 11)
        (
            "|spider::car_1::model::default::ModelList.\nall(\n    %3!=\n            %0.0!= \n    \n   \n      \n        \n         \n\n      \n    \n  \n      \n%4->agg('Continent_t1'=='Cylinders_T3'):max|'id'&&'Make')",
            "0!= \n",
        ),
        // car_1 (live walk 36)
        (
            "|spider::car_1::model::default::CarMakers.\n\nall(%4*\n    \n  \n   \n  \n      \n%4000!=  \n     \n    \n    \n    \n  \n    \n      \n  \n      \n  \n      \n            \n        \n          %0.0)",
            "0)",
        ),
        // car_1 (live walk 52) — a date literal may not end on its separator
        // either, so `%1974.` dies on the `)` the dangling dot can never reach.
        (
            "\n        \n      \n  \n|spider::car_1::model::default::CarMakers.\n          \n         \n  \n      \n\n  all(%1974.)",
            ")",
        ),
        // The date/time handover happens once, so a `T` past the time's first
        // `:` is dead where a `-` (a timezone offset) is not.
        ("|X.all(%2018-03-17T07:13:53T1)", "T1)"),
        ("|X.all(%20:18T3)", "T3)"),
    ] {
        assert_dies_at(walk, at);
    }
}

/// A `(` at a **value** position opens a parenthesised group, which holds one
/// expression and so has no `,` to separate — unlike the `(` that follows a name,
/// which opens a call's argument list. Live-attested on this branch:
/// `->limit((1,2))`, `->limit(('a','b'))`, `->limit(1+(2,3))` and
/// `->extend(('MPG_T2',extend))` are each "no viable alternative at input", while
/// `->limit((1))`, `->limit([1,2])`, `->limit((x|1))` and `->limit((a:b[1]|1))`
/// all parse — a group still opens a lambda and a typed-binder slot, which is why
/// only the comma moved.
#[test]
fn a_comma_inside_a_parenthesised_group_dies() {
    assert_dies_at(
        "    \n    \n        {      \n        \n        \n       \n  \n      \n      \n  \n        \n           \n      \n      \n      \n        |\n      \n      \n       \n        spider::car_1::model::default::ModelList. \n    \n        \n        \n          \n  all()\n\n      \n->extend(('MPG_T2',extend.'Model_T2_2'>=countryId|'FullName_T2_3'||Integer!=meta::relational::metamodel::join::JoinType*extend('CountryId_T2') .\n    true!='Accelerate'   \n)>'Cylinders_T1')>=m::col\n*b.'Continent_t1'}",
        ",extend.'Model_T2_2'",
    );
}

/// A lambda binder is named by an **identifier**, so a pipe off any other
/// completed term can only be the boolean `||`. Live-attested on this branch:
/// `->filter(f()|1)`, `->filter(1|1)`, `->filter($x.a|1)`, `->filter(x.y|1)`,
/// `->filter(extend.min|'d')`, `->filter([1]|1)` and `->filter(%2018-01-01|1)`
/// are each "no viable alternative at input '…|'", while `->filter(x|1)`,
/// `->filter('a'|1)` and `->filter(a&&b|1)` parse — the last because `b` is
/// itself a bare name in operand position. The anchor is the byte *after* the
/// pipe: the automaton holds the `|` open for a possible `||`.
#[test]
fn a_lambda_pipe_off_a_term_that_is_not_a_name_dies() {
    assert_dies_at(
        "  \n    \n\n      {\n\n\n     \n         \n  \n      \n      \n        \n      \n \n          |spider::world_1::model::default::Countrylanguage.\nall(\n)->filter('Name_city'\n    +'CountryCode_T2'.'LifeExpectancy_T1'&&extend.min|'default')\n  \n    }",
        "'default')",
    );
}

/// A `%` date literal opens on a digit; the `-`/`T`/`:` separators are interior
/// bytes only. Three walks verbatim from the live lane's second Phase 7
/// measurement — the probability mass the milestone keyword freed moved straight
/// onto `%->`, and the engine rejects every one at the `%` itself.
#[test]
fn a_date_literal_that_opens_on_a_separator_dies() {
    for (walk, at) in [
        // world_1 (live walk 14)
        (
            "\n  \n    {\n\n\n  \n    \n    \n    \n    \n     \n\n    \n  \n      \n        \n        \n            \n\n        \n        \n      |spider::world_1::model::default::Countrylanguage.    \n        \n    \n         \n    all()\n\n->sort(all(%1\n    !=  \n \n\n  \n      \n    \n        \n        \n\n   \n          \n\n      %->renameColumns(2\n  .INNER)>'Language_T2'&&2\n  )  )    }",
            "->renameColumn",
        ),
        // world_1 (live walk 53)
        (
            "\n        \n      \n  \n|spider::world_1::model::default::Country.\n          \n         \n  \n      \n\n  all(%->max('CountryCode_T1_1'\n=='LocalName_T1_3'=='IsOfficial_T2'),'LocalName_T1'&&b)",
            "->max('Country",
        ),
        // car_1 (live walk 57)
        (
            "\n        \n      \n  \n|spider::car_1::model::default::CarMakers.\n          \n         \n  \n      \n\n  all(%->join(extend(    r|'Make_T2'\n  ,horsepower('fiat'=='Accelerate_T3'+'amc hornet sportabout (sw)'\n        )\n  ->filter('Cylinders_T2'&&'Year_T3'  ))))",
            "->join(extend(",
        ),
    ] {
        assert_dies_at(walk, at);
    }
}

/// A call's `(` applies a *function*, which is named by an identifier: a
/// juxtaposed application off a call's own result, a string literal, or a number
/// is a dead state. Live-attested ("Unexpected token '('").
#[test]
fn a_juxtaposed_application_off_a_non_name_dies() {
    for (walk, at) in [
        // world_1
        (
            "  {|spider::world_1::Db->tableReference()(isEmpty:limit&&'CountryCode_t3')}",
            "(isEmpty:limit",
        ),
        // car_1
        (
            "     \n      {\n      \n  \n          \n  |\n            spider::car_1::model::default::CarsData. \n\n        all(\n        )('American Motor Company'\n    )-'Continent_T3'\n        \n}",
            "('American Motor Company'",
        ),
        // car_1
        (
            "  {\n          \n    \n\n      \n    \n\n    \n        \n        \n    \n\n\n  \n        \n         \n  \n    \n  \n          \n      \n  \n        \n  \n\n        \n  \n    \n                 \n        |\n      spider::car_1::Db->tableReference(6  (x:horsepower||x(1980    ).'Horsepower_T2_2'\n      &&'Ford Motor Company')<=maker(col:a>'Edispl_T2_2'))}",
            "(x:horsepower",
        ),
    ] {
        assert_dies_at(walk, at);
    }
}

/// A `[` after a completed term is the multiplicity of the type it annotates,
/// and nothing else — the engine has no positional index at all and says so:
/// "Bracket operation is not supported". Live-attested on all three walks.
#[test]
fn a_bracket_off_a_non_name_dies() {
    for (walk, at) in [
        // world_1
        (
            "  \n  \n    {\n\n\n  \n    \n    \n    \n    \n     \n\n    \n  \n      \n        \n        \n            \n\n        \n        \n      |spider::world_1::model::default::Countrylanguage.    \n        \n    \n         \n    all()['IsOfficial_t2']}",
            "['IsOfficial_t2']",
        ),
        // car_1
        (
            "  \n      \n    \n  \n        \n          \n      {\n  |\n\n      spider::car_1::model::default::ModelList. all()['MPG_T1_1']&&|'Id_T2'}",
            "['MPG_T1_1']",
        ),
        // car_1
        (
            "  \n   \n     {        \n              \n     \n        \n    \n    \n\n  \n        \n          \n        \n  \n        \n    \n        \n                   |  \n  \n        spider::car_1::model::default::ModelList.\n  all( \n        )\n  \n      ['CountryId_T1']\n       }",
            "['CountryId_T1']",
        ),
    ] {
        assert_dies_at(walk, at);
    }
}

/// The soundness counterfactual for the four tightenings above: every legal
/// shape they sit next to still streams, so none of them is passing by rejecting
/// its whole neighbourhood. Each of these is engine-verified `parse_ok` in
/// `corpus/differential_l1.jsonl`.
#[test]
fn the_shapes_the_name_and_frame_rules_sit_next_to_still_stream() {
    // A call's paren and a multiplicity bracket, separated from their name by
    // whitespace — the name position survives it.
    assert!(!dies("|X.all()->filter (x|$x.v > 1)"));
    assert!(!dies(
        "|X.all()->filter(row : meta::pure::tds::TDSRow [1]|$row.getInteger('x') > 0)"
    ));
    // A comma, a lambda pipe and a typed-binder colon inside the frames that do
    // take an element list.
    assert!(!dies("|X.all()->project([x|$x.a, x|$x.b], ['c','d'])"));
    assert!(!dies("|X.all()->groupBy(~[desk], ~'total': y|$y->sum())"));
    assert!(!dies(
        "|X.all()->groupBy([], agg('c',row: meta::pure::tds::TDSRow[1]|$row, \
         y: meta::pure::tds::TDSRow[*]|$y->count()))"
    ));
    // A block query's own `;`-separated statements, and a value-position `::`
    // classpath at a statement level — the one `:` continuation that stays legal
    // wherever a classpath is.
    assert!(!dies(
        "{|let m = X.all()->take(1); Y.all()->filter(b|$b.v == $m)->take(1);}"
    ));
    assert!(!dies(
        "{|X.all()->filter(x|$x.t == meta::relational::metamodel::join::JoinType.INNER);}"
    ));
}

/// A `:` that follows a completed term is either a `::` classpath separator —
/// legal wherever a classpath is — or a **typed binder**'s own colon, which needs
/// an argument or element slot to bind in. A block query's statement level has
/// neither, so the binder arms die there while `::` keeps streaming.
///
/// Off a **name or a string literal** the anchor names the byte *after* the
/// colon: a `::` may still follow there, so the automaton consumes the `:` into
/// its lookahead state and the refusal lands on the byte that would have opened
/// the binder. Off any **other** completed term a `::` cannot follow at all
/// (issue #55 Phase 9), so with no binder slot open the colon has no reading
/// left and the refusal lands on the colon itself — which is where the engine
/// points too ("Unexpected token ':'"). Only walk 1 is of the latter kind.
///
/// All four walks came verbatim out of the live lane (issue #55 Phase 4) with the
/// engine's "Unexpected token ':'".
#[test]
fn a_typed_binder_colon_at_a_block_statement_level_dies() {
    for (walk, at) in [
        // world_1
        (
            "  \n  \n    {\n\n\n  \n    \n    \n    \n    \n     \n\n    \n  \n      \n        \n        \n            \n\n        \n        \n      |spider::world_1::model::default::Countrylanguage.    \n        \n    \n         \n    all():language*meta::relational::metamodel::join::JoinType}",
            ":language*meta::",
        ),
        // car_1
        (
            "  \n    \n  \n      \n    \n        \n       \n      {    \n    \n    \n         \n  \n        \n      \n\n    \n          \n     \n  \n     \n      \n  |\n   spider::car_1::model::default::CarMakers.\n  \n\nall(\n  \n        )<Float:between}",
            "between}",
        ),
        // car_1
        (
            "   \n    \n  {|spider::car_1::Db->tableToTDS('Year_T1')>='CountryId_T2'\n    :tableToTDS }",
            "tableToTDS }",
        ),
        // world_1
        (
            "     {\n           \n      |    spider::world_1::model::default::Countrylanguage.\n      \n          \n          \n        \n\n        \n        \n        \n      \n    all()=='hasDutch'->filter('US Territory'&&'dutch')!='CountryCode_city'||'GNPOld_t1':row1}",
            "row1}",
        ),
    ] {
        assert_dies_at(walk, at);
    }
}

/// The counterfactual for the rule above: a value-position `::` classpath at the
/// very same block-statement level still streams, so the colon rule is narrowing
/// the binder arm and not the separator.
#[test]
fn a_value_position_classpath_at_a_block_statement_level_still_streams() {
    assert!(!dies(
        "{|X.all()->filter(x|$x.t == meta::relational::metamodel::join::JoinType.INNER)\
         ->take(1);}"
    ));
}

/// A `::` names a **package path**, and a package path is spelled from a bare
/// word or a quoted one. Off any other completed term — a call's `)`, a `]`, a
/// number, a date literal, a `$`-variable, a navigated `.property` or a
/// `->`-called name — the engine has no reading for it (issue #55 Phase 9).
///
/// Where the walk still has a binder slot open the `:` is consumed as a possible
/// typed-binder colon and the refusal lands on the second `:`; where it has none
/// the colon has no reading at all and the refusal lands on the first. Both
/// refusal points are frozen below.
///
/// All three walks came verbatim out of the live lane on this branch, each with
/// the pinned engine's own rejection: walk 1 "no viable alternative at input
/// '…->extend('Code_T1'&&code&&'Republic')::'", walks 2 and 3 "Unexpected token
/// '::'".
#[test]
fn a_classpath_separator_off_a_non_name_dies() {
    for (walk, at) in [
        // world_1 (live walk 18) — `::` off a call's `)`, no binder slot open
        (
            "  \n  \n      \n    \n  {|spider::world_1::Db->tableReference('HeadOfState_T1_3'    \n    \n    ,'english'\n        )!='CountryCode_T1_1'\n        ==String|||c+'Population_T1_1'\n        ->extend('Code_T1'&&code&&'Republic'\n  )::filter\n      \n  \n        *asc('HeadOfState_T3_1'  )<'CountryCode_T1_1'|||_<c::min}",
            "::filter",
        ),
        // car_1 (live walk 13) — the same shape, in a walk whose *legal* `::`es
        // off a bare name and off a string literal both stream past first
        (
            "  {\n      \n        \n      \n    \n      |spider::car_1::Db->tableReference('MPG_T2'\n\n        \n         \n      \n      \n    ,'null'\n    )!=mpg::getInteger\n  &&'europe'::makeId\n  (extend|  'cars_data'\n        )\n        ::extend||_}",
            "::extend||_}",
        ),
        // car_1 (live walk 55) — `::` off a navigated `.property`, inside a call
        // argument, so the refusal lands on the second colon
        (
            "      \n      {|\n        spider::car_1::Db->tableReference('Continent_t1'\n  \n    \n\n        \n      \n            \n        \n        \n  \n \n        \n             \n        \n      \n    \n       \n  ,'Accelerate_T3')->join('ContId_T1'&&'MPG_T1_1'.meta::pure::tds::TDSRow)}",
            ":pure::tds",
        ),
    ] {
        assert_dies_at(walk, at);
    }
}

/// The soundness counterfactual for the rule above: every `::` the engine *does*
/// accept off a completed term still streams. Each shape here was probed through
/// the pinned engine on this branch and parses; the classpath separator binds to
/// a term-start name or a string literal, across whitespace as well.
#[test]
fn a_classpath_separator_off_a_name_or_a_string_literal_still_streams() {
    assert!(!dies("|X.all()->filter(c|$c.name != mpg::getInteger)"));
    assert!(!dies(
        "|X.all()->filter(c|$c.name != meta::pure::tds::TDSRow)"
    ));
    assert!(!dies("|X.all()->filter(c|$c.name != 'europe'::makeId)"));
    assert!(!dies("|X.all()->filter(c|$c.name != 'a b'::c)"));
    assert!(!dies("|X.all()->filter(c|$c.name != mpg ::getInteger)"));
    // The typed binder's own type classpath, whose `:` also opens off a name.
    assert!(!dies(
        "|X.all()->filter(row: meta::pure::tds::TDSRow[1]|$row.getInteger('c') == 1)"
    ));
    // arm-R's second column colon legitimately follows a *completed* term — a
    // navigation in the first walk, a brace lambda's `}` in the second — so the
    // binder arms this rule leaves in place are load-bearing.
    assert!(!dies(
        "|X.all()->groupBy(~[K], ~'Agg': x|$x.v : y|$y->sum())"
    ));
    assert!(!dies(
        "|X.all()->project(~[N: x|$x.a])->extend(over(~N), ~[agg:{p,w,r|$r.v}:y|$y->sum()])"
    ));
}

/// `letBinding`'s value is `pipeline | scalarExpr` (issue #352, `docs/spec/grammar.md`
/// §5.1): a block query that binds a scalar once and threads it through milestoned
/// navigation (`{|let d = today(); T.all($d)->…}`) used to die at the initializer's
/// very first byte, even though `$d`, `today()` and a date literal were already
/// admitted at every *argument* position (`.all($d)`, `.all(today())`,
/// `.all(%2024-01-01)`). `today()`/`now()`/the date literal are live-attested here —
/// each round-tripped through the pinned 4.113.0 engine's own
/// `grammarToJson/lambda` during this fix and reached a real protocol AST, exactly
/// as `corpus/modern_dialect_seeds.jsonl`'s three new `issue-352/let-scalar:*` rows
/// record (`modern_dialect_soundness.rs` replays them). `%latest` is different: the
/// same probe shows the engine rejects it as a bare `let` value ("Unexpected token
/// '%latest'") — it is admitted here only because `milestoneLit` is *already*
/// admitted at every value position, `let`'s included, as the pre-existing residual
/// over-approximation §5.6 documents (the position/sigil phase a byte machine
/// cannot track); this is that same gap reached through one more hub, not a new one.
#[test]
fn a_let_binding_admits_the_scalarexpr_forms_issue_352_asks_for() {
    // The pre-existing pipeline-valued form still streams (regression anchor).
    assert!(!dies("{|let a = t::A.all(); $a->filter(x|$x.n > 1);}"));
    // Live-attested against the pinned engine (see doc comment above).
    assert!(!dies(
        "{|let d = today(); t::A.all($d)->filter(x|$x.n > 1);}"
    ));
    assert!(!dies("{|let d = now(); t::A.all($d)->filter(x|$x.n > 1);}"));
    assert!(!dies(
        "{|let d = %2024-01-01; t::A.all($d)->filter(x|$x.p($d,$d).n > 1);}"
    ));
    // `%latest` streams too — the documented over-approximation, not a live-attested
    // shape (the engine itself rejects a bare `let` value of `%latest`).
    assert!(!dies(
        "{|let d = %latest; t::A.all($d)->filter(x|$x.n > 1);}"
    ));
}

/// The scalar-call half of `scalarExpr` is deliberately **nullary only** — `let
/// d = today();`, never `let d = today(1)` or `let d = today($x)`. Unlike most of
/// this grammar's argument positions (§5.6: arity is generally left to the
/// compiler), a byte-PDA can enforce "zero arguments" cheaply — require the `)`
/// immediately — so the tighter, evidence-scoped reading is the one issue #352
/// actually asks for, not the maximal one. A qualified call (`ns::today()`) and a
/// bare `$`-var initializer (`let d = $other`) are real Legend Pure too (both
/// parse against the pinned engine) but are outside the three scalarExpr shapes
/// issue #352 names, so they stay unadmitted pending their own evidenced ask. A
/// bare top-level scalar query (`|today()`) stays unadmitted for a different
/// reason: `simpleQuery` is `pipeline`, never a bare value, and widening the whole
/// query envelope is no part of this fix.
#[test]
fn a_let_binding_scalar_call_stays_arity_and_qualification_restricted() {
    assert!(dies(
        "{|let d = today(1); t::A.all($d)->filter(x|$x.n > 1);}"
    ));
    assert!(dies(
        "{|let d = today($x); t::A.all($d)->filter(x|$x.n > 1);}"
    ));
    assert!(dies(
        "{|let d = ns::today(); t::A.all($d)->filter(x|$x.n > 1);}"
    ));
    assert!(dies("{|let d = $other; t::A.all($d)->filter(x|$x.n > 1);}"));
    assert!(dies("|today()"));
}
