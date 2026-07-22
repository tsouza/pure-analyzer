//! The precision (negative) corpus — the pin no other gate can replace
//! (`specs/m1-l1-grammar.md`, Fix 1a; ADR-0004).
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

/// A date literal must carry at least one date character (finding F).
#[test]
fn an_empty_date_literal_dies() {
    assert!(dies("|X.all()->take(%)"));
    assert!(dies("|X.all()->filter(x|$x.d < %)"));
}

/// The symbolic milestoning literal (`%latest`/`%latestdate`, gap report G2) is a
/// `%` followed by **lowercase letters only**. A bare `%`, an uppercase or digit
/// first byte after `%`, or a digit/uppercase byte mid-literal all die — so the
/// widening admits the milestone symbols without opening `%<anything>`.
#[test]
fn a_milestoning_literal_is_lowercase_letters_only() {
    // Bare `%` still dies (shared with the date-literal pin above).
    assert!(dies("|X.all()->take(%)"));
    // Uppercase first byte after `%` is not a milestone symbol.
    assert!(dies("|X.all()->take(%Latest)"));
    // A digit or uppercase byte terminates the lowercase run, and the trailing
    // byte has no legal continuation in an argument position.
    assert!(dies("|X.all()->take(%latest1)"));
    assert!(dies("|X.all()->take(%latestX)"));
    // …but the real milestone literals stream (both source and operand position).
    assert!(!dies("|X.all(%latest)->take(1)"));
    assert!(!dies("|X.all(%latest, %latest)->take(1)"));
    assert!(!dies("|X.all(%latestdate)->take(1)"));
    assert!(!dies("|X.all()->filter(x|$x.d == %latest)"));
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
