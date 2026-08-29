//! L2 precision counterfactuals (`docs/spec/schema.md` §8.3, spec M3 G2/G3/G5).
//!
//! Soundness proves L2 never masks a real token; precision proves it *does* mask
//! the phantom and type-mismatched tokens L2 exists to eliminate. Each case is
//! derived mechanically from a gold query by swapping **one** token — a real
//! property for a phantom, a real class for a non-existent one, a matching literal
//! for a type-mismatched one, a real column for an unemitted name — and asserts
//! that at the decision point the real token is still admissible while the swapped
//! token is cleared. The out-of-sample (OOS) cases run on the three held-out
//! schemas, so precision is shown to generalize to schemas no rule was authored
//! against (G5).
#![forbid(unsafe_code)]

#[path = "support/l2.rs"]
mod l2;
#[path = "support/lex.rs"]
mod lex;

use l2::{TokenVocab, lex, load_schema};
use purecard::{CompiledGrammar, DecoderSession};

/// Replay a full `query` token-by-token through a schema-aware session for
/// `db_id`, asserting the killer L2-soundness property on every step: the real
/// token is admissible, is accepted, and the stream ends complete. The gold
/// corpus is arm-A/arm-C only, so this is the arm-R L2 soundness net for a
/// realistic `filter→project→groupBy→sort` aggregation pipeline.
fn assert_streams_soundly_under_l2(db_id: &str, query: &str) {
    let vocab = TokenVocab::build(&[query], &[]);
    let grammar = CompiledGrammar::compile(vocab.vocab());
    let schema = load_schema(db_id);
    let mut session =
        DecoderSession::with_schema(&grammar, schema).expect("grammar is fixed-engine");
    for (step, token) in lex(query).into_iter().enumerate() {
        let id = vocab
            .id_of(&token)
            .unwrap_or_else(|| panic!("token not in vocab: {:?}", bytes_str(&token)));
        assert!(
            session.allowed_mask().test(id),
            "L2 SOUNDNESS: arm-R rule masked a real token at step {step} ({:?}) in:\n  {query}",
            bytes_str(&token)
        );
        session
            .accept_token(id)
            .unwrap_or_else(|err| panic!("real token rejected at step {step}: {err}\n  {query}"));
    }
    assert!(
        session.is_complete(),
        "L2 SOUNDNESS: arm-R pipeline did not complete:\n  {query}"
    );
}

#[test]
fn a_full_arm_r_aggregation_pipeline_streams_soundly_under_l2() {
    // A realistic arm-R pipeline exercising, under an active schema, N1 members
    // inside `project` (`$x.cylinders`/`$x.horsepower` on CarsData), the new
    // relation-row column access in `groupBy` (`$x.Hp`, an emitted column), the
    // `~`-column key/ref positions, and the reducer arrow. Every real token must
    // stream — the arm-R analogue of the 269-gold `l2_soundness` replay.
    let query = "|spider::car_1::model::default::CarsData.all()\
        ->filter(x|$x.cylinders >= 0)\
        ->project(~[Cyl: x|$x.cylinders, Hp: x|$x.horsepower])\
        ->groupBy(~[Cyl], ~'TotalHp': x|$x.Hp : y|$y->sum())\
        ->sort([ascending(~Cyl)])";
    assert_streams_soundly_under_l2("car_1", query);
}

#[test]
fn a_nested_arm_r_subquery_does_not_taint_the_outer_arm_a_pipeline() {
    // Soundness: an arm-A/TDS pipeline whose filter predicate contains an *inner*
    // arm-R aggregation subquery. The inner `~[` must not latch the outer pipeline
    // as arm-R: the inner class navigation `$z.cylinders` stays admissible, the
    // inner relation column `$w.K` narrows to the inner universe, and — after the
    // subquery — the outer TDS getter `$r.getInteger('Cyl')` is NOT masked as a
    // phantom column. (Without pipeline-arm scoping, `$z.cylinders` was masked.)
    let query = "|spider::car_1::model::default::CarsData.all()\
        ->project([x|$x.cylinders], ['Cyl'])\
        ->filter(q|spider::car_1::model::default::CarsData.all()\
            ->project(~[K: z|$z.cylinders])\
            ->groupBy(~[K], ~'v': w|$w.K : y|$y->sum())\
            ->isEmpty())\
        ->filter(r|$r.getInteger('Cyl') > 0)";
    assert_streams_soundly_under_l2("car_1", query);
}

#[test]
fn a_navigation_headed_arm_r_subquery_does_not_taint_the_outer_pipeline() {
    // Soundness: the nested arm-R subquery is headed by a *navigation*
    // (`$r.cylinders->groupBy(~[…])`) rather than `Class.all()`. Its `~[` still must
    // not leak `saw_tilde_bracket`/`rel_explicit` to the enclosing arm-A pipeline —
    // otherwise the later TDS binder `s` is misclassified as a relation row and the
    // valid getter `$s.getInteger('Cyl')` is masked as a phantom column. Scoping the
    // arm state to the lambda body (not just to `all()` entry) closes the leak.
    let query = "|spider::car_1::model::default::CarsData.all()\
        ->project([x|$x.cylinders], ['Cyl'])\
        ->filter(r|$r.cylinders\
            ->groupBy(~[K], ~'v': w|$w.K : y|$y->sum())\
            ->isEmpty())\
        ->filter(s|$s.getInteger('Cyl') > 0)";
    assert_streams_soundly_under_l2("car_1", query);
}

#[test]
fn a_shadowed_binder_is_restored_when_the_inner_arm_r_scope_closes() {
    // Soundness: a nested arm-R subquery reuses the outer filter's binder name `x`
    // and classifies it as a relation row; when that inner scope closes, `x` must be
    // restored to the outer CarsData binding, so the outer `$x.cylinders` still
    // narrows as an N1 member and is not masked as a phantom column against the inner
    // relation's `{K, v}` universe. (Without binder-scope restoration, `cylinders`
    // was masked here.)
    let query = "|spider::car_1::model::default::CarsData.all()\
        ->filter(x|spider::car_1::model::default::CarsData.all()\
            ->project(~[K: z|$z.cylinders])\
            ->groupBy(~[K], ~'v': x|$x.K : y|$y->sum())\
            ->isEmpty() && $x.cylinders > 0)";
    assert_streams_soundly_under_l2("car_1", query);
}

/// Drive `prefix` (a valid partial query) through a schema-aware session for
/// `db_id`, then report, for each token in `probes`, whether it is admissible at
/// the resulting position. `probes` tokens are injected into the vocabulary so
/// they have ids even when absent from `prefix`.
fn admissible_after(db_id: &str, prefix: &str, probes: &[&[u8]]) -> Vec<bool> {
    let extras: Vec<Vec<u8>> = probes.iter().map(|p| p.to_vec()).collect();
    let vocab = TokenVocab::build(&[prefix], &extras);
    let grammar = CompiledGrammar::compile(vocab.vocab());
    let schema = load_schema(db_id);
    let mut session =
        DecoderSession::with_schema(&grammar, schema).expect("grammar is fixed-engine");
    for token in lex(prefix) {
        let id = vocab
            .id_of(&token)
            .unwrap_or_else(|| panic!("prefix token not in vocab: {:?}", bytes_str(&token)));
        session
            .accept_token(id)
            .unwrap_or_else(|err| panic!("prefix token rejected: {err}"));
    }
    let mask = session.allowed_mask();
    probes
        .iter()
        .map(|p| {
            let id = vocab.id_of(p).expect("probe token in vocab");
            mask.test(id)
        })
        .collect()
}

fn bytes_str(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

/// Assert the real token stays admissible and the phantom is masked at a position.
fn assert_precision(db_id: &str, prefix: &str, real: &[u8], phantom: &[u8]) {
    let verdicts = admissible_after(db_id, prefix, &[real, phantom]);
    assert!(
        verdicts[0],
        "precision regression: real token {:?} was masked after prefix in {db_id}:\n  {prefix}",
        bytes_str(real)
    );
    assert!(
        !verdicts[1],
        "precision GAP: phantom token {:?} survived after prefix in {db_id}:\n  {prefix}",
        bytes_str(phantom)
    );
}

#[test]
fn n3_masks_a_phantom_source_class() {
    // A real class heads `.all()`; a non-existent path in the same namespace does
    // not — N3 clears it at the source position.
    assert_precision(
        "car_1",
        "|",
        b"spider::car_1::model::default::CarsData",
        b"spider::car_1::model::default::DoesNotExist",
    );
    // The store path is a legal source (arm-A), a phantom store is not.
    assert_precision("car_1", "|", b"spider::car_1::Db", b"spider::car_1::Nope");
}

#[test]
fn source_method_arg_masks_a_phantom_argument_but_keeps_the_closer_and_a_milestone_date() {
    // `Class.all(...)` ordinarily takes no argument at all; a real class's own
    // call must still close cleanly, but a phantom identifier/string argument —
    // exactly the shape the schema walker was observed emitting
    // (`Class.all('French')`, `Class.all(all)`, both confirmed live to fail
    // Legend compilation) — must be masked at the argument position.
    let prefix = "|spider::car_1::model::default::CarsData.all(";
    assert_precision("car_1", prefix, b")", b"'French'");
    assert_precision("car_1", prefix, b")", b"all");
    // Bitemporal milestoning's exception: a milestone/date literal is a real
    // argument here (corpus `differential_l1.jsonl`'s `Firm.all(%latest)`), so it
    // must stay admissible — the phantom above is the identifier/string shape,
    // never the milestoning one.
    assert_precision("car_1", prefix, b"%latest", b"'French'");
}

#[test]
fn n1_masks_a_phantom_property_after_a_bound_var() {
    // `$x` is bound to CarsData; `cylinders` is a real property, `sallary` is not.
    let prefix = "|spider::car_1::model::default::CarsData.all()->filter(x|$x.";
    assert_precision("car_1", prefix, b"cylinders", b"sallary");
    // A sibling class's property is equally phantom on CarsData (`maker` is a
    // CarMakers/ModelList property, not a CarsData one).
    assert_precision("car_1", prefix, b"horsepower", b"maker");
}

#[test]
fn n1_masks_a_phantom_property_fused_with_the_nav_dot() {
    // Byte-level BPE fuses the navigation `.` with the property's first character
    // into a single token (`.z`, `.theme`, `.maker` are each one token). N1 must
    // narrow the post-dot identifier even when it rides in on the dot's token —
    // otherwise a phantom whose first char begins no legal property streams
    // unconstrained (the mask is read at the pre-dot anchor, where the member
    // position is not yet active). Prefix ends at `$c` (the dot is NOT a separate
    // token), so the decision point is the fused `.<char>` token itself.
    let prefix = "|spider::concert_singer::model::default::Concert.all()->filter(c|$c";
    // Real Concert properties fused with the dot stay admissible…
    assert_precision("concert_singer", prefix, b".theme", b".zzz");
    // …and a phantom whose leading char begins no property (`m…`) is masked — the
    // exact class the split-token path (`$c.` then `maker`) already catches.
    assert_precision("concert_singer", prefix, b".concertName", b".maker");
}

#[test]
fn a_fused_navdot_float_operand_is_never_masked() {
    // SOUNDNESS guard for the fused-navdot pass: a value-position leading-dot float
    // (`.5`) shares its shape with a fused member token but routes through the
    // number states, not `AfterDot`. Even where a bare class-bound `$var` operand
    // leaves a stale nav target, the ident-START gate must keep `.5` admissible.
    let prefix = "|spider::concert_singer::model::default::Concert.all()->filter(c|$c.year > $c.stadiumId + ";
    let verdict = admissible_after("concert_singer", prefix, &[b".5"]);
    assert!(
        verdict[0],
        "L2 SOUNDNESS: fused leading-dot float `.5` masked by the nav-dot pass"
    );
}

#[test]
fn n1_masks_a_phantom_property_fused_after_a_class_navigation() {
    // Nested navigation: an association step reaches a class, and the *next* nav dot
    // is fused with the following property. With `$x.fk0DefaultConcert` still open
    // (the member the coming dot closes), the fused pass must resolve it to Concert
    // and narrow the second, fused hop — a real Concert property stays admissible, a
    // phantom is masked.
    let prefix = "|spider::concert_singer::model::default::SingerInConcert.all()\
        ->filter(x|$x.fk2DefaultConcert";
    assert_precision("concert_singer", prefix, b".theme", b".zzz");
}

#[test]
fn n2_masks_a_phantom_after_an_association_step() {
    // `$x.fk2DefaultCarMakers` advances ModelList → CarMakers; `fullName` is a
    // real CarMakers property, `cylinders` (a CarsData property) is not.
    let prefix =
        "|spider::car_1::model::default::ModelList.all()->filter(x|$x.fk2DefaultCarMakers.";
    assert_precision("car_1", prefix, b"fullName", b"cylinders");
}

#[test]
fn t1_masks_a_type_mismatched_comparison_operand() {
    // `cylinders` is Integer (numeric): a bare number is admissible, a
    // single-quoted string literal is masked.
    let numeric = "|spider::car_1::model::default::CarsData.all()->filter(x|$x.cylinders == ";
    assert_precision("car_1", numeric, b"4", b"'four'");
    // The `horsepower:String` lever (§6.2.2 declared-type caveat): a string
    // literal is admissible, a number literal is masked — the SQL-numeric column
    // is correctly constrained as String by the model.
    let string = "|spider::car_1::model::default::CarsData.all()->filter(x|$x.horsepower == ";
    assert_precision("car_1", string, b"'150'", b"150");
}

#[test]
fn t2_masks_an_ordered_comparator_on_a_non_ordered_operand() {
    // `cylinders` is Integer (numeric): ordered comparators are legal, so `<`
    // stays admissible after the property navExpr.
    let numeric = "|spider::car_1::model::default::CarsData.all()->filter(x|$x.cylinders ";
    assert_precision("car_1", numeric, b"<", b"<<");
    // `horsepower` is String (declared-type caveat, §6.2.2): T2 restricts ordered
    // comparators to numeric/temporal operands, so `<` is masked while the
    // equality comparator `==` stays admissible.
    let string = "|spider::car_1::model::default::CarsData.all()->filter(x|$x.horsepower ";
    assert_precision("car_1", string, b"==", b"<");
}

#[test]
fn t3_masks_a_type_mismatched_aggregation_reducer() {
    // `getInteger('Cylinders')` types the reduce lambda's `y: Integer[*]`
    // element as numeric: every reducer, including `sum`, stays admissible.
    let numeric = "|spider::car_1::model::default::CarsData.all()->groupBy([], \
        [agg('X', row: meta::pure::tds::TDSRow[1]|$row.getInteger('Cylinders'), \
        y: Integer[*]|$y->";
    assert_precision("car_1", numeric, b"sum", b")");
    // `getString('Horsepower')` types the reduce lambda's `y: String[*]`
    // element as String: `sum` (numeric-only) is masked. `min` stays
    // admissible — a real gold query uses `->min()` on a `String[*]` element
    // (lexicographic ordering), so `min`/`max`/`count` are deliberately left
    // unconstrained (see `narrow::keeps_reducer`'s doc comment).
    let string = "|spider::car_1::model::default::CarsData.all()->groupBy([], \
        [agg('X', row: meta::pure::tds::TDSRow[1]|$row.getString('Horsepower'), \
        y: String[*]|$y->";
    assert_precision("car_1", string, b"min", b"sum");
}

#[test]
fn n6_masks_an_unemitted_relation_column() {
    // After `project(...,['Name','Result'])` the relation columns are exactly
    // those names; a getInteger of an emitted name is admissible, of an unemitted
    // one is masked.
    let prefix = "|spider::battle_death::model::default::Battle.all()\
        ->project([x|$x.name, x|$x.result], ['Name', 'Result'])\
        ->filter(r|$r.getInteger(";
    assert_precision("battle_death", prefix, b"'Name'", b"'Ghost'");
}

#[test]
fn arm_r_groupby_map_lambda_binder_does_not_mask_a_projected_column() {
    // Soundness regression (L2 gap report): the arm-R aggregation map lambda binds
    // its variable after a colon (`~'Total': x|$x.Cyl`). A preceding `filter(x|…)`
    // bound `x` to the source class; without re-binding at the groupBy map lambda's
    // `|`, `$x.Cyl` was narrowed to CarsData members and the projected column `Cyl`
    // (not a CarsData property) was masked — a real token the model emits.
    //
    // End-to-end through the shipped grammar + scope + narrower: the binder now
    // degrades to the post-project relation row, so `Cyl` streams unmasked.
    let prefix = "|spider::car_1::model::default::CarsData.all()\
        ->filter(x|$x.cylinders >= 0)\
        ->project(~[Cyl: x|$x.cylinders])\
        ->groupBy(~[Cyl], ~'Total': x|$x.";
    let verdicts = admissible_after("car_1", prefix, &[b"Cyl"]);
    assert!(
        verdicts[0],
        "L2 SOUNDNESS: the projected column `Cyl` was masked at the groupBy map \
         lambda's member position in car_1:\n  {prefix}"
    );
}

#[test]
fn arm_r_groupby_map_lambda_binder_masks_a_phantom_column() {
    // The precision upgrade: on an arm-R relation-row binder, `$x.<Col>` is a
    // bare-ident column access narrowed against the emitted-column universe. The
    // real projected column `Cyl` streams (soundness); a name that no `~`-construct
    // emitted is a phantom and is masked (precision) — end-to-end through the
    // shipped grammar + scope + narrower.
    let prefix = "|spider::car_1::model::default::CarsData.all()\
        ->filter(x|$x.cylinders >= 0)\
        ->project(~[Cyl: x|$x.cylinders])\
        ->groupBy(~[Cyl], ~'Total': x|$x.";
    assert_precision("car_1", prefix, b"Cyl", b"Zzz");
}

#[test]
fn arm_r_groupby_map_binder_narrows_a_fused_relation_column() {
    // The RelationColumn *fused* branch end-to-end: on an arm-R relation-row binder a
    // fused `.<Col>` token — the nav dot and the column's first byte packed into one
    // BPE token — must narrow against the emitted-column universe, not stream on the
    // strength of the pre-dot anchor (where the column position is not yet active).
    // The prefix ends at `$x` (the dot is NOT a separate token), so the decision point
    // is the fused `.<char>` token itself: the real projected column `.Cyl` stays
    // admissible while a name no `~`-construct emitted (`.Zzz`) is masked — the fused
    // single-token analogue of the split-token `$x.` column pass.
    let prefix = "|spider::car_1::model::default::CarsData.all()\
        ->filter(x|$x.cylinders >= 0)\
        ->project(~[Cyl: x|$x.cylinders])\
        ->groupBy(~[Cyl], ~'Total': x|$x";
    assert_precision("car_1", prefix, b".Cyl", b".Zzz");
}

#[test]
fn arm_r_project_map_lambda_binder_stays_narrowed_to_the_source() {
    // The dual: inside `project(~[Cyl: x|$x.` the binder `x` is a row of the source
    // relation, so N1 must still narrow `$x.<prop>` against CarsData — the rebinding
    // fix must not degrade this still-typed position to pass-through. A preceding
    // filter (the exact trigger of the soundness bug) must not perturb it.
    let prefix = "|spider::car_1::model::default::CarsData.all()\
        ->filter(x|$x.cylinders >= 0)\
        ->project(~[Cyl: x|$x.";
    assert_precision("car_1", prefix, b"cylinders", b"sallary");
}

#[test]
fn precision_generalizes_to_oos_held_out_schemas() {
    // world_1: Country is a real class; a phantom is masked at the source, and a
    // phantom property is masked after a bound var — on a schema no rule was
    // authored against (G5).
    assert_precision(
        "world_1",
        "|",
        b"spider::world_1::model::default::Country",
        b"spider::world_1::model::default::Nation",
    );
    let prefix = "|spider::world_1::model::default::Country.all()->filter(x|$x.";
    assert_precision("world_1", prefix, b"name", b"gdp");

    // dog_kennels: a phantom property after a bound var is masked.
    let dk = "|spider::dog_kennels::model::default::Professionals.all()->filter(x|$x.";
    assert_precision("dog_kennels", dk, b"lastName", b"salary");

    // student_transcripts_tracking: same, on the third held-out schema.
    let st = "|spider::student_transcripts_tracking::model::default::Transcripts.all()\
        ->filter(x|$x.";
    assert_precision(
        "student_transcripts_tracking",
        st,
        b"transcriptDate",
        b"nonexistent",
    );
}

/// Replay `walk`'s tokens through a schema-aware session and assert the stream
/// **cannot** be produced: at some step the walk's own next token is absent from
/// `allowed_mask`, so `accept_token` would be the only way forward, and that the
/// token which closed it is exactly `closed_by` — the rule under test's own kill,
/// not some later rule's.
///
/// The assertion is on the frozen walk **string**, never a walk index: any rule
/// change reshuffles the whole chained-SplitMix64 exploration stream, so an index
/// names a different query after the very next PR while the text stays exactly
/// the degenerate shape that was proven to fail live.
///
/// **`closed_by` is not decoration.** Three phases running, a newly landed rule
/// has silently taken over a *previous* phase's frozen kills — the walk still
/// fails to stream, the fixture still passes, and the rule it was written for
/// stops being exercised at all. In-diff mutation testing cannot see that class:
/// the code that lost its coverage is untouched by the diff. Pinning the closing
/// token turns the takeover into a red test at the moment it happens (issue #55,
/// Phase 3; constitution §5 — fix the class, not the instance).
fn assert_walk_is_masked(db_id: &str, walk: &str, closed_by: &str) {
    let vocab = TokenVocab::build(&[walk], &[]);
    let grammar = CompiledGrammar::compile(vocab.vocab());
    let schema = load_schema(db_id);
    let mut session =
        DecoderSession::with_schema(&grammar, schema).expect("grammar is fixed-engine");
    for (step, token) in lex(walk).into_iter().enumerate() {
        let id = vocab
            .id_of(&token)
            .unwrap_or_else(|| panic!("token not in vocab: {:?}", bytes_str(&token)));
        if !session.allowed_mask().test(id) {
            assert_eq!(
                bytes_str(&token),
                closed_by,
                "the walk was closed by a different token than the rule under test's:\n  {walk}"
            );
            return;
        }
        session.accept_token(id).unwrap_or_else(|err| {
            panic!("L1 rejected an L2-admitted token at step {step}: {err}\n  {walk}")
        });
    }
    panic!(
        "PRECISION GAP: {db_id} walk still streams end-to-end — the rule that was \
         supposed to close it does not fire:\n  {walk}"
    );
}

/// Issue #55 bucket A — a fabricated `::` segment glued onto a source classpath
/// that had already ended. Every one of these nine `world_1` walks was produced
/// by the schema walker and rejected by a live Legend engine with "Can't find the
/// packageable element '<path>'"; N3's classpath-continuation rule now clears the
/// `:` that starts the phantom segment, so none of them can be emitted at all.
///
/// Frozen verbatim (issue #55 Phase 1, gate (b)): these strings are the
/// class-killing evidence constitution §5 requires, and they must survive any
/// future corpus or vocabulary reshuffle.
#[test]
fn n3_masks_every_fabricated_classpath_extension_walk() {
    for (walk, closed_by) in [
        (
            "{|spider::world_1::Db::desc->min('_v')}",
            "spider::world_1::Db::desc",
        ),
        (
            "\n|spider::world_1::model::default::Countrylanguage::name\
             ->distinct('asia'!='GovernmentForm_T1_3')",
            "spider::world_1::model::default::Countrylanguage::name",
        ),
        (
            "{\n    |spider::world_1::model::default::Countrylanguage::pair\
             ->concatenate(c)+Integer}",
            "spider::world_1::model::default::Countrylanguage::pair",
        ),
        (
            "|spider::world_1::model::default::Countrylanguage::limit\
             ->isEmpty('GovernmentForm_T1')",
            "spider::world_1::model::default::Countrylanguage::limit",
        ),
        (
            "{     \n    |spider::world_1::Db::name::language->distinct(row!=.3000)*limit}",
            "spider::world_1::Db::name::language",
        ),
        (
            "|spider::world_1::model::default::Country::distinct::Y->min()",
            "spider::world_1::model::default::Country::distinct::Y",
        ),
        ("{\n    |l::filter->project(renameColumns)}", "l::filter"),
        (
            "|spider::world_1::model::default::Country::min->limit('Capital_t1')",
            "spider::world_1::model::default::Country::min",
        ),
        (
            "|spider::world_1::model::default::Country::row1\
             ::spider::world_1::model::default::Countrylanguage::pair::groupBy\
             ->restrict('IndepYear'&&'_nn'!='_ord0'+'Percentage_T2_2'!='GNPOld_T1_1')",
            "spider::world_1::model::default::Country::row1\
             ::spider::world_1::model::default::Countrylanguage::pair::groupBy",
        ),
    ] {
        assert_walk_is_masked("world_1", walk, closed_by);
    }
}

/// Issue #55 bucket C — a `$var` reference to a name nothing in the stream ever
/// bound. Both `world_1` walks were rejected by a live Legend engine with "Can't
/// find variable class for variable '<name>' in the graph"; S2
/// (`L2Position::RefVar`) now admits only names the tracker has actually seen
/// bound, and neither walk binds anything at all.
///
/// Frozen verbatim (issue #55 Phase 1, gate (b)) — see
/// [`n3_masks_every_fabricated_classpath_extension_walk`] on why the string, not
/// the walk index, is the fixture.
#[test]
fn s2_masks_every_unbound_refvar_walk() {
    for (walk, name) in [
        ("{|\n        $code\n      /'IsOfficial_t2'}", "code"),
        ("{\n|      $name}", "name"),
    ] {
        assert_walk_is_masked("world_1", walk, name);
    }
}

/// S2's soundness edge, pinned alongside its precision: the *bound* name stays
/// admissible at exactly the position where the unbound one is cleared. Without
/// this the rule could pass its precision fixtures by masking every `$`
/// reference — the failure mode gate (a)'s gold replay catches in bulk, pinned
/// here as a directed counterfactual.
#[test]
fn s2_keeps_a_bound_binder_and_masks_its_neighbours() {
    assert_precision(
        "world_1",
        "|spider::world_1::model::default::Country.all()->filter(x|$",
        b"x",
        b"y",
    );
    // A `let` binder is bound by position with no pipe to confirm it, and it
    // outlives its own statement — the gold corpus's `->in($topStates)` shape.
    assert_precision(
        "world_1",
        "{|let topStates = spider::world_1::model::default::Country.all();\n  $",
        b"topStates",
        b"nowhereBound",
    );
}

/// Stream every token of `walk` (each must be admissible) and return whether the
/// L2 overlay lets the stream **end** there — [`DecoderSession::is_complete`] and
/// the published EOS bit, asserted to agree.
///
/// The completion counterpart of [`assert_walk_is_masked`]: a walk can be
/// perfectly admissible token-by-token and still be a query no host may stop on,
/// because the last lexeme is only half a name or owes a mandatory call.
fn walk_may_end(db_id: &str, walk: &str) -> bool {
    let vocab = TokenVocab::build(&[walk], &[]);
    let grammar = CompiledGrammar::compile(vocab.vocab());
    let schema = load_schema(db_id);
    let mut session =
        DecoderSession::with_schema(&grammar, schema).expect("grammar is fixed-engine");
    for (step, token) in lex(walk).into_iter().enumerate() {
        let id = vocab
            .id_of(&token)
            .unwrap_or_else(|| panic!("token not in vocab: {:?}", bytes_str(&token)));
        assert!(
            session.allowed_mask().test(id),
            "this fixture is about completion, not admissibility, but the rule \
             masked a token at step {step} ({:?}) in:\n  {walk}",
            bytes_str(&token)
        );
        session
            .accept_token(id)
            .unwrap_or_else(|err| panic!("token rejected at step {step}: {err}\n  {walk}"));
    }
    let complete = session.is_complete();
    let eos = session.allowed_mask().test(grammar.vocab().len() as u32);
    assert_eq!(
        complete, eos,
        "is_complete and the published EOS bit disagree for:\n  {walk}"
    );
    complete
}

/// Issue #55 Phase 2, piece 1 — **mask-aware completion**. Both shapes are
/// L1-accepting (`InIdent` over an empty stack: an identifier has no
/// self-terminating byte, so L1's lookahead calls any partial name completable)
/// and both are rejected by a live Legend engine — `Class.a` and `Class.all`
/// alike with "Can't find property '<name>' in class
/// 'meta::pure::metamodel::type::Class'". The overlay now clears the EOS bit at a
/// trie cursor that has reached only a strict prefix of a legal name, and after a
/// whole niladic method name whose call parens are still owed.
///
/// Frozen verbatim (gate (b)); the counterfactual below keeps this from passing
/// by simply never completing.
#[test]
fn a_half_typed_or_uncalled_source_method_can_never_end_a_query() {
    for walk in [
        "|spider::world_1::model::default::Country.a",
        "|spider::world_1::model::default::Country.all",
    ] {
        assert!(
            !walk_may_end("world_1", walk),
            "PRECISION GAP: the overlay still lets a query end here:\n  {walk}"
        );
    }
    // The counterfactual: the same source, called, compiles live
    // (=> `spider::world_1::model::default::Country`) and does end.
    assert!(walk_may_end(
        "world_1",
        "|spider::world_1::model::default::Country.all()"
    ));
}

/// Issue #55 Phase 2, piece 2 — the **resolved-method must-call veto**. After the
/// whole name `all`, nothing but its own `(` continues: not EOS, not another hop,
/// and not whitespace (a space would close the lexeme and hand the escape
/// straight back).
#[test]
fn a_resolved_source_method_admits_nothing_but_its_call() {
    let walk = "|spider::world_1::model::default::Country.all";
    for probe in ["->", " ", ".", ")", "x"] {
        assert_precision("world_1", walk, b"(", probe.as_bytes());
    }
}

/// Issue #55 Phase 2 — the `let`-candidate states are inside a source
/// identifier. This `world_1` walk was produced by the schema walker and
/// rejected live with "Can't find the packageable element 'l'": `l` is a strict
/// prefix of the `let` keyword N3 admits, and N3 used to go dark the moment the
/// byte-PDA entered `LetL`, so a bare `l` read as a finished source.
#[test]
fn n3_masks_a_source_that_is_only_a_prefix_of_the_let_keyword() {
    assert_walk_is_masked("world_1", "{|l->pair(col>'SUM(SurfaceArea)')}", "->");
}

/// Issue #55 Phase 2, piece 3 — **N7**, a bare unresolved identifier in a value
/// position. Every one of these `world_1` walks was produced by the schema walker
/// and rejected by a live Legend engine with "Can't find the packageable element
/// '<word>'": a bare word that no lambda arrow, package separator, navigation, or
/// call ever gives a meaning to resolves to nothing.
///
/// **Phase 3 took every one of these kills over.** Each walk arrows straight off
/// a bare class or store source, so N3c now closes it at that arrow — before the
/// dangling word is ever reached. They stay frozen (they are still walks the
/// decoder must never emit), with the closing token pinned so the takeover is
/// recorded rather than silent; N7's own walk-level evidence moved to
/// [`n7_masks_a_dangling_value_identifier_behind_a_real_extent`], whose walks
/// reach the value position through a legal `Class.all()`.
///
/// Frozen verbatim (gate (b)) — see
/// [`n3_masks_every_fabricated_classpath_extension_walk`] on why the string, not
/// the walk index, is the fixture.
#[test]
fn n7_masks_every_dangling_value_identifier_walk() {
    for (walk, closed_by) in [
        (
            "|spider::world_1::model::default::Country->max(language)",
            "->",
        ),
        (
            "|spider::world_1::model::default::Countrylanguage->pair(code    \n!='Name_T2')",
            "->",
        ),
        (
            "|spider::world_1::model::default::Country->filter('Percentage_T2_4'<average)",
            "->",
        ),
        (
            "|spider::world_1::model::default::Countrylanguage->between(renameColumns>'hasDutch')",
            "->",
        ),
        (
            "{|spider::world_1::model::default::Country->between(join)<LEFT_OUTER}",
            "->",
        ),
        (
            "|spider::world_1::model::default::Country->tableReference(restrict)",
            "->",
        ),
        (
            "|spider::world_1::model::default::Countrylanguage\
             ->groupBy('Gelderland'||'Population_T1_1'&&asc)",
            "->",
        ),
        (
            "{|spider::world_1::Db->concatenate('IndepYear_T1_1',desc-col=='IndepYear_country')}",
            "concatenate",
        ),
        (
            "|spider::world_1::model::default::Countrylanguage->tableReference(pair)",
            "->",
        ),
        (
            "|spider::world_1::model::default::Country->pair(tableReference)&&5",
            "->",
        ),
        (
            "|spider::world_1::model::default::Country->col(between\n*'District_city')",
            "->",
        ),
        (
            "|spider::world_1::model::default::Country->filter('SUM(SurfaceArea)'<agg/'_nn__t0anti1')",
            "->",
        ),
    ] {
        assert_walk_is_masked("world_1", walk, closed_by);
    }
}

/// N7's walk-level evidence, re-rooted through a real extent so no source-level
/// rule can reach it first: the same dangling-word payloads, behind a legal
/// `Class.all()`. Each was sent through the live engine on this branch and
/// rejected with the same "Can't find the packageable element '<word>'" as its
/// bare-source sibling — the dangling word, not the source, is what the engine
/// objects to:
///
/// ```text
/// |…::Country.all()->max(language)                        => …element 'language'
/// |…::Country.all()->filter('SUM(SurfaceArea)'<agg/'…')   => …element 'agg'
/// |…::Country.all()->pair(tableReference)&&5              => …element 'tableReference'
/// |…::Countrylanguage.all()->pair(code    \n!='Name_T2')   => …element 'code'
/// |…::Country.all()->col(between\n*'District_city')        => …element 'between'
/// ```
#[test]
fn n7_masks_a_dangling_value_identifier_behind_a_real_extent() {
    for (walk, closed_by) in [
        (
            "|spider::world_1::model::default::Country.all()->max(language)",
            ")",
        ),
        (
            "|spider::world_1::model::default::Country.all()\
             ->filter('SUM(SurfaceArea)'<agg/'_nn__t0anti1')",
            "/",
        ),
        (
            "|spider::world_1::model::default::Country.all()->pair(tableReference)&&5",
            ")",
        ),
        (
            "|spider::world_1::model::default::Countrylanguage.all()\
             ->pair(code    \n!='Name_T2')",
            "    \n",
        ),
        (
            "|spider::world_1::model::default::Country.all()->col(between\n*'District_city')",
            "\n",
        ),
    ] {
        assert_walk_is_masked("world_1", walk, closed_by);
    }
}

/// N7's soundness edge, pinned alongside its precision (the same counterfactual
/// discipline S2 carries): at exactly the position where a dangling word's closer
/// is cleared, every shape that *does* give a bare word a meaning stays
/// admissible — the lambda arrow, a `::` package separator, a navigation dot, and
/// a call's own `(`. Without this the rule could pass its fixtures by masking
/// everything after any identifier.
#[test]
fn n7_keeps_every_continuation_that_gives_a_bare_word_a_meaning() {
    let walk = "|spider::world_1::model::default::Country.all()->filter(x";
    for legal in ["|", ":", ".", "("] {
        assert_precision("world_1", walk, legal.as_bytes(), b")");
    }
    // A boolean literal is a complete value in its own right, so N7 leaves it and
    // whatever follows it alone — the gold corpus's own `… == true }` shape.
    // Stated as a contrast on the *same* closer, since the keyword exemption
    // means there is no phantom to probe at that one position.
    let lambda = "|spider::world_1::model::default::Country.all()->filter(x|";
    assert!(
        admissible_after("world_1", &format!("{lambda}true"), &[b")"])[0],
        "N7 masked the closer after a complete boolean literal"
    );
    assert!(
        !admissible_after("world_1", &format!("{lambda}agg"), &[b")"])[0],
        "N7 GAP: the same closer survived after a dangling bare word"
    );
}

/// Issue #55 Phase 3 — **N3c, the class arm**. A pipeline source that resolves
/// to a real class denotes the `Class<T>[1]` metatype, not the `T[*]` extent
/// `.all()` produces, so a method arrowed straight off it mismatches every
/// signature by construction. Live-attested on the criterion engine:
///
/// ```text
/// |…::Country->groupBy('CountryCode_T2_2')
///   => Can't find a match for function 'groupBy(Class<Country>[1],String[1])'
/// |…::Country->sort('Name_t1')
///   => Can't find a match for function 'sort(Class<Country>[1],String[1])'
/// |…::Country->restrict()
///   => Can't find a match for function 'restrict(Class<Country>[1])'
/// ```
///
/// Each walk below came verbatim out of the live lane's own walk set at the
/// pre-Phase-3 rule state (37 `world_1` and 32 `car_1` exploration walks shared
/// this one shape); one is frozen per distinct arrowed method name, since the
/// method name is the only thing that varies — the mask bit N3c clears is the
/// same `->` in every one of them.
///
/// Frozen verbatim (gate (b)) — see
/// [`n3_masks_every_fabricated_classpath_extension_walk`] on why the string, not
/// the walk index, is the fixture.
#[test]
fn n3c_masks_every_arrow_off_a_bare_class_source_walk() {
    for (db_id, walk) in [
        (
            "world_1",
            "\n        |spider::world_1::model::default::Countrylanguage->agg('Central Africa')",
        ),
        (
            "world_1",
            "\n\n \n       \n           |spider::world_1::model::default::Country->col(between.'HeadOfState_T1'!='Brazil')",
        ),
        (
            "world_1",
            "\n              \n    \n        \n  \n    \n        |spider::world_1::model::default::Countrylanguage->count('Beatrix'&&'AVG(LifeExpectancy)')",
        ),
        (
            "world_1",
            "\n\n    \n|spider::world_1::model::default::Country->distinct('Angola')",
        ),
        (
            "world_1",
            "\n    \n  \n        \n        |spider::world_1::model::default::Country->filter('Percentage_T2_4'<average.'HeadOfState_country')",
        ),
        (
            "world_1",
            "\n|spider::world_1::model::default::Country->groupBy('CountryCode_T2_2')",
        ),
        (
            "world_1",
            "|spider::world_1::model::default::Countrylanguage->isEmpty('_k0')",
        ),
        (
            "world_1",
            "  |spider::world_1::model::default::Countrylanguage->join('District_city'\n  .renameColumns||'Europe')",
        ),
        (
            "world_1",
            "|spider::world_1::model::default::Countrylanguage->max(b.'Region_T3_1')",
        ),
        (
            "world_1",
            "\n             |spider::world_1::model::default::Country->restrict()",
        ),
        (
            "world_1",
            "\n    \n    |spider::world_1::model::default::Country->sort('Population_T3_1')",
        ),
        (
            "world_1",
            "\n    \n         \n  \n        \n      |spider::world_1::model::default::Countrylanguage->sum('GNPOld_T1_3'>'country')",
        ),
        (
            "world_1",
            "\n           |spider::world_1::model::default::Countrylanguage->tableReference(pair|'Angola'\n    )",
        ),
        (
            "car_1",
            "|spider::car_1::model::default::CarsData->col(3,'FullName_t1_1')",
        ),
        (
            "car_1",
            "\n\n    |spider::car_1::model::default::CarsData->count('CountryName'<='Model_T1')",
        ),
        (
            "car_1",
            "|spider::car_1::model::default::CarMakers->extend('Continent_T3')",
        ),
        (
            "car_1",
            "  \n      \n        \n  \n        \n    \n\n \n       \n           |spider::car_1::model::default::ModelList->filter('null')",
        ),
        (
            "car_1",
            "    |spider::car_1::model::default::ModelList->groupBy('Maker_T2_3'>|'Accelerate_T2_2'    )",
        ),
        (
            "car_1",
            "\n        \n    \n      \n\n\n        \n  |spider::car_1::model::default::CarsData->project('CountryName')",
        ),
        (
            "car_1",
            "|spider::car_1::model::default::ModelList->restrict(fk4DefaultCarsData.'Maker_t2_4')",
        ),
        (
            "car_1",
            "\n        \n  |spider::car_1::model::default::CarsData->year('Horsepower_T1'\n=='cars_data'||'_c0__t0l0'!='cars_data'\n  *'Country_T1'    )",
        ),
    ] {
        assert_walk_is_masked(db_id, walk, "->");
    }
}

/// Issue #55 Phase 3 — **N3c, the store arm**. The mirror image: a source path
/// that resolves to the schema's store denotes a
/// `meta::relational::metamodel::Database`, which has no extent, so `.all()` on
/// it is the one continuation it can never take. Live-attested on both criterion
/// and generalization schemas:
///
/// ```text
/// |spider::world_1::Db.all()
///   => Can't find a match for function 'getAll(Database[1])'
/// |spider::car_1::Db.all()
///   => Can't find a match for function 'getAll(Database[1])'
/// ```
///
/// Frozen verbatim (gate (b)).
#[test]
fn n3c_masks_every_all_call_on_a_store_source_walk() {
    for (db_id, walk) in [
        (
            "world_1",
            "\n    {\n          \n          |\n        \n    spider::world_1::Db.\n    \n    \n      \n  all()}",
        ),
        (
            "car_1",
            "          \n    \n      \n  \n        {\n  \n        \n        \n    |spider::car_1::Db.all()}",
        ),
    ] {
        assert_walk_is_masked(db_id, walk, ".");
    }
}

/// Issue #55 Phase 3 — **N3c's store-method set**. A store *is* arrowed into,
/// but only into a store method. Every one of these walks came verbatim out of
/// the live lane's pre-Phase-3 walk set and reaches the engine as a bare
/// `Database[1]` receiver:
///
/// ```text
/// |spider::world_1::Db->tableToTDS()
///   => Can't find a match for function 'tableToTDS(Database[1])'
/// ```
///
/// The legal set is read off the corpus, not invented: across the 5034 gold
/// queries a store path is followed by `->tableReference` 8455 times and by
/// nothing else.
#[test]
fn n3c_masks_every_store_arrowed_into_a_non_store_method_walk() {
    for (db_id, walk, method) in [
        (
            "world_1",
            "{|spider::world_1::Db->max('CountryCode_T2_2')(isEmpty:limit&&'CountryCode_t3')}",
            "max",
        ),
        (
            "world_1",
            "\n  \n      \n{\n      |spider::world_1::Db->String('GNP_t1'*'HeadOfState_T3_1' )!=getFloat('CountryCode_t2'  \n      )==getInteger|'AVG(GNP)'!='Continent_T1_1'}",
            "String",
        ),
        (
            "car_1",
            "\n    \n\n        {\n\n      |spider::car_1::Db->project('MakeId_T1'\n         =='MPG')  }",
            "project",
        ),
        (
            "car_1",
            "\n    {|spider::car_1::Db->exists()|year('ModelId'<='MPG_T1'.'Country'-weight('Id_T1_1','volvo'\n    )&&'Model_T2'+'car_names'    !='$)a)parseFloat<,4000}(tableToTDS)]}else",
            "exists",
        ),
    ] {
        assert_walk_is_masked(db_id, walk, method);
    }
}

/// Issue #55 Phase 3 — the **disclosed precision cost**, pinned so it can never
/// be quietly re-opened. These five walks *did* compile live at the pre-Phase-3
/// rule state, and N3c masks them anyway:
///
/// ```text
/// |…::Country->pair('US Territory')  => meta::pure::functions::collection::Pair
/// |…::Country->limit(1930)          => meta::pure::metamodel::type::Class
/// ```
///
/// Each one only "compiled" because a loose builtin signature happily accepts a
/// `Class<T>[1]` metatype and hands it (or a `Pair` of it) straight back — a
/// query that returns the class object it was given, never a result set. Losing
/// a compile that only ever worked because the receiver type was wrong is the
/// precision win, not a regression: the live compile-rate lane's own numbers
/// (`world_1` exploration 12/59 → 34/59, `car_1` 17/58 → 34/58) are net of these
/// five.
#[test]
fn n3c_masks_the_accidental_metatype_compiles_it_costs() {
    for (db_id, walk) in [
        (
            "world_1",
            "\n        |spider::world_1::model::default::Country->pair('US Territory')",
        ),
        (
            "world_1",
            "\n        |spider::world_1::model::default::Country->limit(1930)",
        ),
        (
            "car_1",
            "\n        \n    |spider::car_1::model::default::ModelList->max()",
        ),
        (
            "car_1",
            "\n  |spider::car_1::model::default::CarsData->concatenate(3  )",
        ),
        (
            "car_1",
            "\n    \n      \n      \n        \n        |\n      spider::car_1::model::default::ModelList->concatenate('CountryId_T1'+'Id_T2_3'\n  )",
        ),
    ] {
        assert_walk_is_masked(db_id, walk, "->");
    }
}

/// N3c's soundness edge, pinned alongside its precision (the same counterfactual
/// discipline S2 and N7 carry): at exactly the positions where the wrong
/// continuation is cleared, the right one survives — and so does a longer class
/// path that merely *begins* with a shorter one, which is the trap a
/// whole-name-only close policy would spring (`…::Country` is a strict byte
/// prefix of `…::Countrylanguage`).
#[test]
fn n3c_keeps_the_one_continuation_each_source_kind_owes() {
    // The class arm: the `.all()` dot survives where the arrow is cleared.
    assert_precision(
        "world_1",
        "|spider::world_1::model::default::Country",
        b".",
        b"->",
    );
    // The store arm, exactly inverted.
    assert_precision("world_1", "|spider::world_1::Db", b"->", b".");
    // …and the real store method survives where a phantom one is cleared.
    assert_precision(
        "world_1",
        "|spider::world_1::Db->",
        b"tableReference",
        b"tableToTDS",
    );
    // The prefix trap: at the terminal node of `…::Country` the trie must still
    // walk on into `…::Countrylanguage`.
    assert_precision(
        "world_1",
        "|spider::world_1::model::default::Country",
        b"language",
        b"->",
    );
    // Both real shapes stream end to end and may end where they should.
    assert!(walk_may_end(
        "world_1",
        "|spider::world_1::model::default::Countrylanguage.all()"
    ));
    assert_streams_soundly_under_l2(
        "world_1",
        "|spider::world_1::Db->tableReference(\n  'default',\n  'city'\n)->tableToTDS()",
    );
    // A bare source path — either kind — can never *end* a query, so masking the
    // wrong continuation cannot be satisfied by simply stopping there.
    for walk in [
        "|spider::world_1::model::default::Country",
        "|spider::world_1::Db",
    ] {
        assert!(
            !walk_may_end("world_1", walk),
            "PRECISION GAP: a bare source path ended a query:\n  {walk}"
        );
    }
}
