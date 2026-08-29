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
/// neither, so the binder arms die there while `::` keeps streaming. The anchor
/// names the byte *after* the colon: the automaton consumes a `:` into its
/// lookahead state (a second `:` is still legal there), so the refusal lands on
/// the byte that would have opened the binder.
///
/// All four walks came verbatim out of the live lane (issue #55 Phase 4) with the
/// engine's "Unexpected token ':'".
#[test]
fn a_typed_binder_colon_at_a_block_statement_level_dies() {
    for (walk, at) in [
        // world_1
        (
            "  \n  \n    {\n\n\n  \n    \n    \n    \n    \n     \n\n    \n  \n      \n        \n        \n            \n\n        \n        \n      |spider::world_1::model::default::Countrylanguage.    \n        \n    \n         \n    all():language*meta::relational::metamodel::join::JoinType}",
            "language*meta::",
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
