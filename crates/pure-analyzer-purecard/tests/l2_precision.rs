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
    let mut session = DecoderSession::with_schema(&grammar, schema);
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
    let mut session = DecoderSession::with_schema(&grammar, schema);
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
