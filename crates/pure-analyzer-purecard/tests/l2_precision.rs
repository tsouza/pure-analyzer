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
//!
//! Every fixture that freezes a *kill* — a walk the decoder must never emit, or
//! a decision point where a phantom must be cleared — lives in one table,
//! [`FROZEN_KILLS`], and records the rule that closes it, together with whether
//! the overlay or the byte-PDA is what does the refusing ([`Closer`]). The
//! per-rule tests
//! read slices of that table; [`every_rule_kind_has_a_frozen_walk_that_it_closes`]
//! reads all of it and fails when a shipped rule has no frozen evidence left.
//! See [`FrozenKill`] for why (issue #55: four repeats of one regression class).
//! The remaining tests here are soundness *edges* — directed contrasts pinned
//! beside a rule's precision so it cannot pass by masking everything — and stay
//! outside the table, because a contrast has no closing mechanism to record.
#![forbid(unsafe_code)]

use std::collections::BTreeSet;

#[path = "support/l2.rs"]
mod l2;
#[path = "support/l2_rules.rs"]
mod l2_rules;
#[path = "support/lex.rs"]
mod lex;

use l2::{TokenVocab, lex, load_schema};
use l2_rules::{ALL_RULE_KINDS, rule_kind};
use purecard::{CompiledGrammar, DecoderSession, Schema};

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
    probe_at(db_id, prefix, probes).0
}

/// [`admissible_after`]'s full result: the per-probe verdicts, plus the rule
/// kind active at that decision point — the mechanism a [`FrozenKill`] records.
fn probe_at(db_id: &str, prefix: &str, probes: &[&[u8]]) -> (Vec<bool>, Option<&'static str>) {
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
    let kind = session.active_l2_position().as_ref().and_then(rule_kind);
    let mask = session.allowed_mask();
    let verdicts = probes
        .iter()
        .map(|p| {
            let id = vocab.id_of(p).expect("probe token in vocab");
            mask.test(id)
        })
        .collect();
    (verdicts, kind)
}

fn bytes_str(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

/// Assert the real token stays admissible and the phantom is masked at a
/// position, discarding the rule kind that cleared it — for the directed
/// soundness-edge counterfactuals, which pin a *contrast* at a position rather
/// than freeze a kill, and so have no mechanism worth recording. A frozen kill
/// belongs in [`FROZEN_KILLS`], where the mechanism is checked.
fn assert_precision(db_id: &str, prefix: &str, real: &[u8], phantom: &[u8]) {
    probe_closer(db_id, prefix, real, phantom);
}

/// A real class heads `.all()`; a non-existent path in the same namespace does
/// not — N3 clears it at the source position.
///
/// The store path is a legal source (arm-A), a phantom store is not.
#[test]
fn n3_masks_a_phantom_source_class() {
    assert_frozen("n3-source-class");
}

/// `Class.all(...)` ordinarily takes no argument at all; a real class's own
/// call must still close cleanly, but a phantom identifier/string argument —
/// exactly the shape the schema walker was observed emitting
/// (`Class.all('French')`, `Class.all(all)`, both confirmed live to fail
/// Legend compilation) — must be masked at the argument position.
///
/// Bitemporal milestoning's exception: a milestone/date literal is a real
/// argument here (corpus `differential_l1.jsonl`'s `Firm.all(%latest)`), so it
/// must stay admissible — the phantom above is the identifier/string shape,
/// never the milestoning one.
#[test]
fn source_method_arg_masks_a_phantom_argument_but_keeps_the_closer_and_a_milestone_date() {
    assert_frozen("source-method-arg");
}

/// `$x` is bound to CarsData; `cylinders` is a real property, `sallary` is not.
///
/// A sibling class's property is equally phantom on CarsData (`maker` is a
/// CarMakers/ModelList property, not a CarsData one).
#[test]
fn n1_masks_a_phantom_property_after_a_bound_var() {
    assert_frozen("n1-member");
}

/// Byte-level BPE fuses the navigation `.` with the property's first character
/// into a single token (`.z`, `.theme`, `.maker` are each one token). N1 must
/// narrow the post-dot identifier even when it rides in on the dot's token —
/// otherwise a phantom whose first char begins no legal property streams
/// unconstrained (the mask is read at the pre-dot anchor, where the member
/// position is not yet active). Prefix ends at `$c` (the dot is NOT a separate
/// token), so the decision point is the fused `.<char>` token itself.
///
/// Real Concert properties fused with the dot stay admissible…
/// …and a phantom whose leading char begins no property (`m…`) is masked — the
/// exact class the split-token path (`$c.` then `maker`) already catches.
#[test]
fn n1_masks_a_phantom_property_fused_with_the_nav_dot() {
    assert_frozen("n1-fused-navdot");
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

/// Issue #354 (end-to-end mask replay of the issue's own reproduction, a
/// synthetic single-class schema not tied to any `FIXTURE_DBS` entry): a
/// class-typed lambda binder (`{y: t::A[1]|$y.`) whose annotation resolves in
/// the schema must narrow `$y.` exactly as an untyped binder narrows against
/// the pipeline's own class — a class in the schema fully determines its
/// member set, whichever multiplicity (`[1]`/`[*]`) the binder carries.
#[test]
fn n1_masks_a_phantom_member_after_a_known_class_typed_binder() {
    const SCHEMA_JSON: &str = r#"{
      "db_id": "t", "db_path": "t::Db",
      "classes": {
        "t::A": { "simple_name": "A", "properties": [
          {"name": "alpha", "type": {"kind": "primitive", "name": "Integer"}, "mult": {"lower": 1, "upper": 1}}
        ], "qualified_properties": [], "super_types": [] }
      },
      "associations": [], "enums": {}
    }"#;
    let real: &[u8] = b"alpha";
    let phantoms: [&[u8]; 2] = [b"zzz", b"beta"];

    for prefix in [
        "|t::A.all()->filter({y: t::A[1]|$y.",
        "|t::A.all()->filter({y: t::A[*]|$y.",
    ] {
        let extras: Vec<Vec<u8>> = std::iter::once(real.to_vec())
            .chain(phantoms.iter().map(|p| p.to_vec()))
            .collect();
        let vocab = TokenVocab::build(&[prefix], &extras);
        let grammar = CompiledGrammar::compile(vocab.vocab());
        let schema = Schema::from_json(SCHEMA_JSON).expect("synthetic schema parses");
        let mut session =
            DecoderSession::with_schema(&grammar, schema).expect("grammar is fixed-engine");
        for token in lex(prefix) {
            let id = vocab
                .id_of(&token)
                .unwrap_or_else(|| panic!("prefix token not in vocab: {:?}", bytes_str(&token)));
            session
                .accept_token(id)
                .unwrap_or_else(|err| panic!("prefix token rejected: {err}\n  {prefix}"));
        }
        let mask = session.allowed_mask();
        let real_id = vocab.id_of(real).expect("real token in vocab");
        assert!(
            mask.test(real_id),
            "L2 SOUNDNESS: a known-class typed binder masked its own real member at:\n  {prefix}"
        );
        for phantom in &phantoms {
            let id = vocab.id_of(phantom).expect("phantom token in vocab");
            assert!(
                !mask.test(id),
                "L2 PRECISION: a known-class typed binder left phantom {:?} admissible at:\n  {prefix}",
                bytes_str(phantom)
            );
        }
    }
}

/// The issue's fourth row: a binder typed with a class the schema does not
/// know (`t::NOPE`) must keep passing through unconstrained — masking here
/// would invent a member set the overlay cannot actually derive (§4).
#[test]
fn n1_admits_every_name_after_an_unresolved_class_typed_binder() {
    const SCHEMA_JSON: &str = r#"{
      "db_id": "t", "db_path": "t::Db",
      "classes": {
        "t::A": { "simple_name": "A", "properties": [
          {"name": "alpha", "type": {"kind": "primitive", "name": "Integer"}, "mult": {"lower": 1, "upper": 1}}
        ], "qualified_properties": [], "super_types": [] }
      },
      "associations": [], "enums": {}
    }"#;
    let prefix = "|t::A.all()->filter({y: t::NOPE[1]|$y.";
    let candidates: [&[u8]; 3] = [b"alpha", b"zzz", b"beta"];
    let extras: Vec<Vec<u8>> = candidates.iter().map(|p| p.to_vec()).collect();
    let vocab = TokenVocab::build(&[prefix], &extras);
    let grammar = CompiledGrammar::compile(vocab.vocab());
    let schema = Schema::from_json(SCHEMA_JSON).expect("synthetic schema parses");
    let mut session =
        DecoderSession::with_schema(&grammar, schema).expect("grammar is fixed-engine");
    for token in lex(prefix) {
        let id = vocab
            .id_of(&token)
            .unwrap_or_else(|| panic!("prefix token not in vocab: {:?}", bytes_str(&token)));
        session
            .accept_token(id)
            .unwrap_or_else(|err| panic!("prefix token rejected: {err}\n  {prefix}"));
    }
    let mask = session.allowed_mask();
    for candidate in &candidates {
        let id = vocab.id_of(candidate).expect("candidate token in vocab");
        assert!(
            mask.test(id),
            "L2 SOUNDNESS: an unresolved-class typed binder masked {:?} — must pass through:\n  {prefix}",
            bytes_str(candidate)
        );
    }
}

/// Nested navigation: an association step reaches a class, and the *next* nav dot
/// is fused with the following property. With `$x.fk0DefaultConcert` still open
/// (the member the coming dot closes), the fused pass must resolve it to Concert
/// and narrow the second, fused hop — a real Concert property stays admissible, a
/// phantom is masked.
#[test]
fn n1_masks_a_phantom_property_fused_after_a_class_navigation() {
    assert_frozen("n1-fused-nav-hop");
}

/// `$x.fk2DefaultCarMakers` advances ModelList → CarMakers; `fullName` is a
/// real CarMakers property, `cylinders` (a CarsData property) is not.
#[test]
fn n2_masks_a_phantom_after_an_association_step() {
    assert_frozen("n2-association");
}

/// `cylinders` is Integer (numeric): a bare number is admissible, a
/// single-quoted string literal is masked.
///
/// The `horsepower:String` lever (§6.2.2 declared-type caveat): a string
/// literal is admissible, a number literal is masked — the SQL-numeric column
/// is correctly constrained as String by the model.
#[test]
fn t1_masks_a_type_mismatched_comparison_operand() {
    assert_frozen("t1-revalue");
}

/// `cylinders` is Integer (numeric): ordered comparators are legal, so `<`
/// stays admissible after the property navExpr.
///
/// `horsepower` is String (declared-type caveat, §6.2.2): T2 restricts ordered
/// comparators to numeric/temporal operands, so `<` is masked while the
/// equality comparator `==` stays admissible.
#[test]
fn t2_masks_an_ordered_comparator_on_a_non_ordered_operand() {
    assert_frozen("t2-comparator");
}

/// `getInteger('Cylinders')` types the reduce lambda's `y: Integer[*]`
/// element as numeric: every reducer, including `sum`, stays admissible.
///
/// `getString('Horsepower')` types the reduce lambda's `y: String[*]`
/// element as String: `sum` (numeric-only) is masked. `min` stays
/// admissible — a real gold query uses `->min()` on a `String[*]` element
/// (lexicographic ordering), so `min`/`max`/`count` are deliberately left
/// unconstrained (see `narrow::keeps_reducer`'s doc comment).
#[test]
fn t3_masks_a_type_mismatched_aggregation_reducer() {
    assert_frozen("t3-reducer");
}

/// T6: `< > <= >=` dispatch to `lessThan`/`greaterThan`/…, which the engine
/// declares only over scalar primitive operands. Each of the three navExpr
/// shapes that is not one masks them while leaving `==` admissible — Pure's
/// `equal` is `Any[*]`-generic, and all three compile with it live:
///
/// - `$c.fk1DefaultCountrylanguage` is `Countrylanguage[1..*]`
///   (`lessThan(Countrylanguage[1..*],Integer[1])`);
/// - `$x.fk3DefaultCarNames.model` is a `String` mapped over a to-many step,
///   so the chain yields `String[*]` even though `model` is declared `[0..1]`
///   on its own class (`lessThan(String[*],String[1])`);
/// - `Country.all().gnp` navigates the extent, itself a `T[*]`
///   (`lessThan(Float[*],Integer[1])`);
/// - `$c.fk1DefaultCountry` is `Country[1]` — to-one, but class-typed, and a
///   class is no ordered operand at any multiplicity
///   (`lessThan(Country[1],Integer[1])`).
#[test]
fn t6_masks_an_ordered_comparator_on_a_non_scalar_nav_expr() {
    assert_frozen("t6-ordered-operand");
}

/// Issue #55 Phase 10 widens T6's position from the ordered comparators to every
/// operator family the engine declares over scalar operands only, and adds the
/// fourth navExpr shape that reaches it. Live, on the same stack:
///
/// ```text
/// {|…::ModelList.all().maker*'MPG'}          Collection element must have a multiplicity [1]
/// {|…::CarMakers.all().id&&'Model_T1_1'}     and(Integer[*],String[1])
/// {|…::CarMakers.all().id/2}                 divide(Integer[*],Integer[1])
/// {|…->filter(c|$c.fk2DefaultModelList||true)} or(ModelList[1..*],Boolean[1])
/// {|…::CarMakers.all()->toOne()<4}           lessThan(CarMakers[1],Integer[1])
/// ```
///
/// The last is the fourth shape: a `toOne` off the class extent collapses the
/// multiplicity and keeps the class, so it reaches the position through a
/// completed *call* rather than through a member name.
#[test]
fn t6_masks_the_logical_and_arithmetic_operators_on_a_non_scalar_nav_expr() {
    assert_frozen("t6-nonscalar-operator");
}

/// T6's widened soundness counterfactual: the families it clears are exactly the
/// scalar-only ones, so everything a non-scalar navExpr really does take stays —
/// equality (`equal` is `Any[*]`-generic), the step arrow that opens the collapse
/// the spec names, and `+`, which `plus(String[*])` declares over a collection.
#[test]
fn t6_still_admits_every_continuation_a_non_scalar_nav_expr_really_takes() {
    for query in [
        "|spider::car_1::model::default::CarMakers.all().id == 1",
        "|spider::car_1::model::default::CarMakers.all().id->count()",
        "|spider::car_1::model::default::CarMakers.all()->toOne() == 1",
        "|spider::car_1::model::default::CarMakers.all()->toOne().id",
        "|spider::car_1::model::default::CarMakers.all()->toOne()->count()",
    ] {
        assert_streams_soundly_under_l2("car_1", query);
    }
}

/// Issue #55 Phase 10 — N3h, the argument half of N3f's position. `groupBy` and
/// `limit` are both legal names on a `T[*]` class extent, so no receiver-category
/// rule reaches them; each is wrong only in what its first slot is filled with,
/// and the engine states the shape back in its own rejection:
///
/// ```text
/// {|…::CarMakers.all()->groupBy('Edispl_T4')}          groupBy(CarMakers[*],String[1])
/// {|…::Countrylanguage.all()->groupBy('Percentage_t2'<'country')}
///                                                      groupBy(Countrylanguage[*],Boolean[1])
/// {|…::CarMakers.all()->limit('MPG_T2_2')}             limit(CarMakers[*],String[1])
/// ```
///
/// The rule claims the argument's **shape** and no part of its arity, though both
/// bounds were probed. See `narrow::fill_extent_method_arg` for why each was left
/// out.
#[test]
fn n3h_masks_a_literal_the_extent_methods_first_argument_cannot_be() {
    assert_frozen("n3h-extent-method-arg");
}

/// N3h's soundness counterfactual: the rule clears only the literals the
/// signature refuses, at the first slot alone. `limit(3)`, the corpus's own
/// list-headed `.all()->groupBy([…], […], […])` — all 37 of its three-argument
/// gold occurrences open on a `[` — and the seeds' two-argument arm-R spelling
/// all stream untouched, and `groupBy`'s later slots, which do take string
/// literals, are never narrowed.
#[test]
fn n3h_still_admits_every_first_argument_the_signature_takes() {
    for query in [
        "|spider::car_1::model::default::CarMakers.all()->limit(3)",
        "|spider::car_1::model::default::CarMakers.all()\
         ->groupBy([x|$x.id], [], ['id'])",
    ] {
        assert_streams_soundly_under_l2("car_1", query);
    }
}

/// The tokens that **terminate** a `project`/`groupBy` column/key lambda body
/// stay admissible on a class-typed and a to-many navExpr — the guard that
/// keeps §6.6 T7 retired (issue #116).
///
/// T7 proposed masking the body's own closer once the lambda body had
/// resolved to a class or a to-many collection. The pinned Legend
/// stack **refutes** the premise: every such shape compiles, including the
/// spec's own literal counter-example `project([x|$x.fk0DefaultCountries], …)`
/// and even a body left at the bare bound instance (`project([x|$x], ['col'])`).
/// `project`'s column lambda is declared over `Any`, so a class-typed or
/// to-many body is a *legal* projection, not a phantom — masking its closer
/// would be a soundness violation. See `docs/spec/schema.md` §6.6 T7 for the
/// full probe table.
///
/// Probed at **both** anchors a T7 could be written at, because they are
/// different rules and only one of them is where T6 lives:
///
/// - the **member** anchor, `…$x.fk1DefaultCountrylanguage` with the property's
///   lexeme still open — the spelling real emitted Pure actually uses (`$x.foo]`
///   has no space in it, and no gold query contains `$x.foo ]`). The active rule
///   here is `Member`, and the tracker has already resolved the member to a
///   class, so this is the *natural* place to write a T7 and must be covered.
/// - the **completed-term** anchor, one space later, where T2/T6 are read. The
///   active rule here is `OrderedOperand` on the non-scalar body and
///   `Comparator` on the scalar control — the two the contrast below separates.
///
/// At the completed-term anchor the ordered comparator is additionally asserted
/// masked on the non-scalar body and admissible on the scalar one, and the
/// active rule kind is pinned. That is the anti-vacuity contrast: it proves the
/// overlay genuinely knows "the body is non-scalar here", so T7's abstention is
/// a decision on the evidence rather than a gap in the tracker.
#[test]
fn t7_keeps_a_projection_lambda_closer_on_a_class_typed_or_to_many_body() {
    /// The body-terminating tokens T7 proposed to mask: the column/key list's
    /// own `]`, and the `,` that ends one body and opens the next. The call's
    /// `)` is not among them — L1 already masks it while the list's `[` is open,
    /// on a scalar body just as much as a non-scalar one, so it carries no L2
    /// signal to assert on.
    const CLOSERS: &[&[u8]] = &[b"]", b","];
    /// The rule the overlay must still be running at the completed-term anchor
    /// on a non-scalar body — T6's. Named so the contrast below asserts the
    /// mechanism, not merely its effect.
    const NON_SCALAR_RULE: &str = "OrderedOperand";
    // Each case pairs a **scalar** body with a **non-scalar** one reached
    // through the identical enclosing call, so the only difference between the
    // two prefixes is what the body resolved to. The scalar control navigates
    // to a **numeric** property so T2 leaves the ordered comparator admissible
    // there, making the `<` contrast read the body's shape rather than a String
    // operand's own T2 verdict. Prefixes carry no trailing space; the loop
    // appends one to reach the second anchor.
    let cases: &[(&str, &str, &str)] = &[
        // Arm-A `project`: a to-many class-typed association end.
        (
            "world_1",
            "|spider::world_1::model::default::Country.all()->project([x|$x.population",
            "|spider::world_1::model::default::Country.all()\
             ->project([x|$x.fk1DefaultCountrylanguage",
        ),
        // Arm-A `project`: a to-*one* class-typed association end — class-typed
        // is enough on its own, multiplicity is not the distinguishing fact.
        (
            "world_1",
            "|spider::world_1::model::default::City.all()->project([x|$x.population",
            "|spider::world_1::model::default::City.all()->project([x|$x.fk0DefaultCountry",
        ),
        // `groupBy`'s key list, with a primitive mapped over a to-many step —
        // T7's other arm ("a body left at a to-many collection").
        (
            "car_1",
            "|spider::car_1::model::default::Continents.all()->groupBy([x|$x.contId",
            "|spider::car_1::model::default::Continents.all()\
             ->groupBy([x|$x.fk0DefaultCountries.countryName",
        ),
        // Arm-R `project(~[…])`, the relation-API spelling of the same position.
        (
            "world_1",
            "|spider::world_1::model::default::Country.all()->project(~[col: x|$x.population",
            "|spider::world_1::model::default::Country.all()\
             ->project(~[col: x|$x.fk1DefaultCountrylanguage",
        ),
    ];
    for (db_id, scalar_base, non_scalar_base) in cases {
        // "" is the member anchor (lexeme still open); " " closes the lexeme and
        // reaches the completed-term anchor T2/T6 are read at.
        for anchor in ["", " "] {
            let scalar = format!("{scalar_base}{anchor}");
            let non_scalar = format!("{non_scalar_base}{anchor}");
            let scalar_verdicts = admissible_after(db_id, &scalar, CLOSERS);
            let non_scalar_verdicts = admissible_after(db_id, &non_scalar, CLOSERS);
            // Without this the equality below could pass by both sides being
            // masked — a future L1 change could kill the guard silently.
            assert!(
                scalar_verdicts.iter().any(|kept| *kept),
                "the scalar control must keep at least one closer, or the \
                 comparison below is vacuous in {db_id}:\n  {scalar}"
            );
            for ((probe, scalar_ok), non_scalar_ok) in
                CLOSERS.iter().zip(scalar_verdicts).zip(non_scalar_verdicts)
            {
                assert_eq!(
                    scalar_ok,
                    non_scalar_ok,
                    "L2 SOUNDNESS: the projection lambda closer `{}` was decided \
                     differently on a non-scalar body in {db_id} — every such shape \
                     compiles against the pinned stack, so the two must agree:\n  \
                     scalar:     {scalar}\n  non-scalar: {non_scalar}",
                    bytes_str(probe)
                );
            }
        }
        // The anti-vacuity contrast, at the completed-term anchor where T6 is
        // read: the overlay does distinguish the two bodies, and does so through
        // T6's rule specifically.
        let scalar_term = format!("{scalar_base} ");
        let non_scalar_term = format!("{non_scalar_base} ");
        assert!(
            admissible_after(db_id, &scalar_term, &[b"<"])[0],
            "T6 must leave an ordered comparator admissible on a scalar body in \
             {db_id}:\n  {scalar_term}"
        );
        let (verdicts, kind) = probe_at(db_id, &non_scalar_term, &[b"<"]);
        assert!(
            !verdicts[0],
            "T6 must mask an ordered comparator on a non-scalar body in \
             {db_id}:\n  {non_scalar_term}"
        );
        assert_eq!(
            kind,
            Some(NON_SCALAR_RULE),
            "the non-scalar body must be recognised by T6's own rule in \
             {db_id}, so the contrast pins the mechanism:\n  {non_scalar_term}"
        );
    }
}

/// T4 narrows the method name after a `->` whose receiver the overlay has
/// already typed, on all three routes that produce one: a TDS accessor call
/// (`$row.getInteger('Cylinders')->`), a bare primitive property navigation
/// (`$x.cylinders->`), and a type-preserving `toOne()` over either
/// (`$x.cylinders->toOne()->`).
///
/// `toUpper`/`toLower`/`startsWith`/`endsWith` are String-only in the engine's
/// own signature table, so each is masked on the Integer receiver while a
/// type-agnostic step (`toOne`) stays admissible. `contains` is *not* masked:
/// it matches the generic collection overload on any receiver (see
/// `narrow::denied_string_method`'s doc comment for the live evidence).
#[test]
fn t4_masks_a_string_method_on_a_non_string_receiver() {
    assert_frozen("t4-string-method");
}

/// N3i, T4's co-tenant at the same position: a `RELATION_RECEIVER_METHODS`
/// name is dead on a scalar-primitive receiver whatever its type class, on each
/// of the four routes the overlay types one from — a completed string literal
/// (`'car_makers'->`), a receiver-only builtin's fixed Boolean
/// (`->isNotEmpty()->`) and Integer (`->count()->`) result, and a bare property
/// navigation (`$x.maker->`).
///
/// Attested live on the pinned engine on every one of them before it was
/// written: `tableToTDS(String[1])`, `restrict(Boolean[1],String[1])`,
/// `renameColumns(Integer[1],String[1])`, `restrict(String[0..1],String[1])` —
/// each answered with a candidate set every entry of which wants a relation or a
/// store.
#[test]
fn n3i_masks_a_relation_method_on_a_scalar_receiver() {
    assert_frozen("n3i-scalar-receiver-method");
}

/// N3i's soundness edge, the mirror of
/// [`t4_keeps_every_string_method_on_a_string_receiver`]: the builtins a scalar
/// primitive receiver really does admit stay admissible on one.
///
/// Streamed whole rather than probed one token at a time, because the rule's
/// clear lands on the token that *closes* a denied name: a name that survives
/// its own bytes proves nothing until its `(` is admitted too.
///
/// The list is not decorative — every call below came back **compiling** against
/// the pinned engine on the receiver it is written on, including the four names
/// whose arity, not whose receiver, is what the failing walks got wrong
/// (`groupBy`, `project`, `limit`, `sort`). Masking any of them would be N3i
/// over-reaching from an arity error into a receiver claim.
#[test]
fn n3i_keeps_a_receiver_generic_method_on_a_scalar_receiver() {
    const EXTENT: &str = "|spider::car_1::model::default::CarMakers.all()";
    // `agg` first: its arrow form is the exact construct the first revision of
    // this rule masked, and the gold corpus writes its plain-function twin 2367
    // times. `no_denied_name_is_one_the_corpus_writes_with_a_scalar_first_argument`
    // is the gate that closes the class; this pins the instance beside its
    // siblings, where the rule's own soundness edge is stated.
    for tail in [
        "->project('COUNT()'->agg(row: meta::pure::tds::TDSRow[1]|$row, \
         y: meta::pure::tds::TDSRow[*]|$y->count()))",
        "->project('car_makers'->substring(0,1))",
        "->project('car_makers'->parseFloat())",
        "->project('car_makers'->pair('_c1'))",
        "->project('car_makers'->in(['_c1']))",
        "->project('car_makers'->toOne())",
        "->project('car_makers'->count())",
        "->project('car_makers'->limit(1))",
        "->project('car_makers'->sort())",
        "->project('car_makers'->groupBy([x|$x],[],['_c1']))",
        "->project('car_makers'->project([x|$x],['_c1']))",
        "->isNotEmpty()->toString()",
        "->isNotEmpty()->toOne()",
        "->isNotEmpty()->in([true])",
        "->count()->toString()",
        "->count()->sort()",
        "->filter(x|$x.maker->in(['_c1']))",
        "->filter(x|$x.maker->toUpper()=='_c1')",
    ] {
        assert_streams_soundly_under_l2("car_1", &format!("{{{EXTENT}{tail}}}"));
    }
}

/// T4's soundness edge: on a receiver the overlay types `String` — a
/// `getString(…)` accessor or a `String` property navigation, the two shapes
/// the gold corpus actually puts a string method on — every one of those
/// methods stays admissible, and so does `contains`, which the engine's
/// generic collection overload makes legal on *any* receiver.
#[test]
fn t4_keeps_every_string_method_on_a_string_receiver() {
    let probes: &[&[u8]] = &[
        b"toLower",
        b"toUpper",
        b"startsWith",
        b"endsWith",
        b"contains",
    ];
    let accessor = "|spider::car_1::model::default::CarsData.all()\
        ->project([x|$x.horsepower], ['Horsepower'])\
        ->filter(row: meta::pure::tds::TDSRow[1]|$row.getString('Horsepower')->";
    let property = "|spider::car_1::model::default::CarsData.all()->filter(x|$x.horsepower->";
    for prefix in [accessor, property] {
        for (probe, admitted) in probes.iter().zip(admissible_after("car_1", prefix, probes)) {
            assert!(
                admitted,
                "L2 SOUNDNESS: `{}` was masked on a String receiver in car_1:\n  {prefix}",
                bytes_str(probe)
            );
        }
    }
    // `contains` is unconstrained on a *non*-String receiver too — the generic
    // collection overload compiles there (live-attested; see
    // `narrow::denied_string_method`).
    let numeric = "|spider::car_1::model::default::CarsData.all()->filter(x|$x.cylinders->";
    assert!(
        admissible_after("car_1", numeric, &[b"contains"])[0],
        "L2 SOUNDNESS: `contains` was masked on a numeric receiver in car_1:\n  {numeric}"
    );
}

/// After `project(...,['Name','Result'])` the relation columns are exactly
/// those names; a getInteger of an emitted name is admissible, of an unemitted
/// one is masked.
#[test]
fn n6_masks_an_unemitted_relation_column() {
    assert_frozen("n6-column");
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

/// The precision upgrade: on an arm-R relation-row binder, `$x.<Col>` is a
/// bare-ident column access narrowed against the emitted-column universe. The
/// real projected column `Cyl` streams (soundness); a name that no `~`-construct
/// emitted is a phantom and is masked (precision) — end-to-end through the
/// shipped grammar + scope + narrower.
#[test]
fn arm_r_groupby_map_lambda_binder_masks_a_phantom_column() {
    assert_frozen("n6-relation-column");
}

/// The RelationColumn *fused* branch end-to-end: on an arm-R relation-row binder a
/// fused `.<Col>` token — the nav dot and the column's first byte packed into one
/// BPE token — must narrow against the emitted-column universe, not stream on the
/// strength of the pre-dot anchor (where the column position is not yet active).
/// The prefix ends at `$x` (the dot is NOT a separate token), so the decision point
/// is the fused `.<char>` token itself: the real projected column `.Cyl` stays
/// admissible while a name no `~`-construct emitted (`.Zzz`) is masked — the fused
/// single-token analogue of the split-token `$x.` column pass.
#[test]
fn arm_r_groupby_map_binder_narrows_a_fused_relation_column() {
    assert_frozen("n6-relation-column-fused");
}

/// The dual: inside `project(~[Cyl: x|$x.` the binder `x` is a row of the source
/// relation, so N1 must still narrow `$x.<prop>` against CarsData — the rebinding
/// fix must not degrade this still-typed position to pass-through. A preceding
/// filter (the exact trigger of the soundness bug) must not perturb it.
#[test]
fn arm_r_project_map_lambda_binder_stays_narrowed_to_the_source() {
    assert_frozen("n1-project-map-binder");
}

/// world_1: Country is a real class; a phantom is masked at the source, and a
/// phantom property is masked after a bound var — on a schema no rule was
/// authored against (G5).
///
/// dog_kennels: a phantom property after a bound var is masked.
///
/// student_transcripts_tracking: same, on the third held-out schema.
#[test]
fn precision_generalizes_to_oos_held_out_schemas() {
    assert_frozen("oos-held-out");
}

/// What is frozen about a [`FrozenKill`].
enum Kill {
    /// A whole walk the decoder must never be able to emit, refused when its own
    /// next token — `closed_by` — is cleared from the mask.
    ///
    /// Frozen on the walk **string**, never a walk index: any rule change
    /// reshuffles the whole chained-SplitMix64 exploration stream, so an index
    /// names a different query after the very next PR while the text stays
    /// exactly the degenerate shape that was proven to fail live.
    Walk {
        walk: &'static str,
        closed_by: &'static str,
    },
    /// A single decision point, derived mechanically from a gold query by
    /// swapping **one** token: after `prefix`, `real` must still be admissible
    /// (soundness) while `phantom` is cleared (precision).
    Probe {
        prefix: &'static str,
        real: &'static str,
        phantom: &'static str,
    },
}

/// A frozen precision fixture: an input the decoder must refuse, recorded
/// together with the **mechanism** that refuses it.
///
/// **Why one table.** Four times running (issue #55 Phases 1–4) a newly landed
/// rule silently took over an earlier phase's kill: the walk still failed, the
/// fixture still passed, and the rule it was written to prove stopped being
/// exercised by anything at all. Phase 3 answered with `closed_by` (the token
/// that closes a walk) and Phase 4 with `precision_reject::assert_dies_at` (its
/// L1 twin). Both are *per-fixture*: they redden when that one walk's outcome
/// changes, and stay silent when a rule quietly loses its last fixture — which
/// is the failure that actually happened. In-diff mutation testing cannot see
/// it either, because the code that lost its coverage is untouched by the diff.
///
/// Recording the closing **rule kind** on every fixture, and checking the whole
/// table against [`ALL_RULE_KINDS`] in
/// [`every_rule_kind_has_a_frozen_walk_that_it_closes`], closes that class for
/// every rule, present and future (constitution §5 — fix the class, not the
/// instance).
struct FrozenKill {
    /// The family tag this fixture belongs to — its provenance, via
    /// [`FROZEN_FAMILIES`], and the slice the per-rule test below reads by.
    /// [`frozen`] refuses to hand back an empty slice, so a renamed or deleted
    /// family fails loudly instead of leaving a test looping over nothing.
    fixture: &'static str,
    /// The fixture database the walk or probe is replayed against.
    db: &'static str,
    /// The rule that governs the position where this fixture is refused, and
    /// whether the overlay is what actually refuses it. Both halves are
    /// re-derived from the shipped decoder on every run by
    /// [`assert_frozen_kill`] and checked against this claim.
    closer: Closer,
    /// The frozen input itself.
    kill: Kill,
}

/// Which layer refuses a [`FrozenKill`], and under which rule.
///
/// The distinction is load-bearing. `DecoderSession::active_l2_position` names
/// the rule that *governs* a position; it does not say the overlay is what
/// cleared the bit. A token the byte-PDA already refuses reads back with an L2
/// position all the same, so recording that position alone would let an
/// L1-dead fixture stand in as an L2 rule's evidence — and
/// [`every_rule_kind_has_a_frozen_walk_that_it_closes`] would then certify a
/// rule as covered by a fixture the rule does not close. Both variants are
/// verified in both directions against a schema-less session, so neither label
/// can be applied to the other's fixture.
enum Closer {
    /// The overlay is what refuses it: with the schema removed, the same token
    /// is still admitted. This is a rule's frozen evidence, and only this
    /// counts toward rule coverage.
    ///
    /// The name is the rule active *where the mask is read*, which is the
    /// position the decoder is at when the offending token is offered. For a
    /// fused `.<char>` token — byte-level BPE packs the navigation dot and the
    /// member's first byte into one token — that position is still the pre-dot
    /// anchor, so those fixtures record `RefVar` rather than the `Member` or
    /// `RelationColumn` rule whose *narrowing* rejects them. That is the honest
    /// reading of the mechanism, not a mislabel: at the pre-dot anchor the
    /// fused pass is what the mask depends on.
    ///
    /// S2's **sigil** half reads the same way. It clears a `$` that no binder
    /// could satisfy at the anchor *before* the sigil (issue #275 — read at the
    /// name instead, the rule would clear everything `AfterDollar` admits and
    /// deadlock the decoder), so a `{|$…}` fixture records the block-statement
    /// anchor's `SourceIdent`, not `RefVar`.
    L2(&'static str),
    /// The byte-PDA refuses it too, so the fixture pins a shape the decoder
    /// must never emit and names the rule that governs the position — but it is
    /// **not** evidence that the rule fires, and never counts toward coverage.
    /// Kept rather than deleted: the walk is still frozen, and demoting a
    /// fixture to this variant cannot buy a rule its coverage.
    AlsoL1(&'static str),
}

impl Closer {
    /// The rule name, whichever layer refuses the fixture.
    fn rule(&self) -> &'static str {
        match self {
            Closer::L2(rule) | Closer::AlsoL1(rule) => rule,
        }
    }

    /// Whether the offending token must still be admitted with no schema.
    fn expects_l1_to_admit(&self) -> bool {
        matches!(self, Closer::L2(_))
    }
}

/// Each fixture family's provenance: the issue #55 phase and failure bucket it
/// was cut from, so a walk's origin survives the rule that motivated it.
///
/// Held here rather than on every row, because it is a fact about the family,
/// not about each of its walks.
/// [`every_frozen_family_is_declared_exactly_once`] keeps the two in step.
const FROZEN_FAMILIES: &[(&str, &str)] = &[
    (
        "n3-classpath",
        "Phase 1 · bucket A — a fabricated `::` segment on a finished source classpath",
    ),
    (
        "s2-refvar",
        "Phase 1 · bucket C — a `$var` nothing in the stream ever bound",
    ),
    (
        "n7-bare-source",
        "Phase 2 · bucket B — a dangling bare word off a bare source; Phase 3's N3c \
         took every one of these kills over",
    ),
    (
        "n7-extent",
        "Phase 3 · bucket B — the same dangling-word payloads, re-rooted through a \
         legal `Class.all()` so no source-level rule reaches them first",
    ),
    (
        "n3f-extent-method",
        "Phase 5 · bucket D — a builtin arrowed off a `Class.all()` extent whose \
         every overload wants a receiver a `T[*]` class collection cannot be",
    ),
    (
        "n3h-extent-method-arg",
        "Phase 10 · bucket D — a literal in the first argument slot of a \
         class-extent builtin whose own signature fixes that slot's shape",
    ),
    (
        "n3i-scalar-receiver-method",
        "Phase 10 · bucket D — a relation/store builtin arrowed off a receiver the \
         overlay has typed a scalar primitive: a string literal, or a \
         receiver-only builtin's fixed Boolean/Integer result",
    ),
    (
        "n3g-receiver-only-arg",
        "Phase 6 · bucket D — an argument passed to an arrow call whose every \
         overload takes the receiver and nothing else",
    ),
    (
        "n4a-store-result",
        "Phase 6 · bucket E — an operator applied to a store method's `Table[1]` \
         result",
    ),
    (
        "n4b-logical-operand",
        "Phase 6 · bucket E — a literal operand of `&&`/`||`, which take Boolean \
         only",
    ),
    (
        "n4b-lambda-operand",
        "Phase 10 · bucket E — a lambda in the same slot, one category further \
         out than the literal",
    ),
    (
        "n4c-str-operator",
        "Phase 6 · bucket E — arithmetic whose left operand is a string literal",
    ),
    (
        "n3c-class",
        "Phase 3 · bucket R1 — a method arrowed off the bare `Class<T>[1]` metatype",
    ),
    (
        "n3c-store-all",
        "Phase 3 · bucket R2 — `.all()` on a store path, which has no extent",
    ),
    (
        "n3c-store-method",
        "Phase 3 · bucket R2 — a store arrowed into a non-store method",
    ),
    (
        "n3c-cost",
        "Phase 3 · the disclosed precision cost — walks that only ever compiled \
         because a loose builtin signature accepts the metatype",
    ),
    (
        "n3d-arg-separator",
        "Phase 4 · the store method's call shape — the separator after a completed \
         argument",
    ),
    (
        "n3d-open-arg-slot",
        "Phase 4 · the store method's call shape — an opened slot owes its argument",
    ),
    (
        "n3e-extent-operator",
        "Phase 4 · an operator applied to a `T[*]` class extent",
    ),
    (
        "n1-extent-dot",
        "Phase 4 · a phantom member after the extent dot, in either spelling",
    ),
    (
        "n3-let-prefix",
        "Phase 2 · a source that is only a prefix of the `let` keyword",
    ),
    (
        "n3-source-class",
        "M3 G2 · a phantom class, and a phantom store, at the pipeline source",
    ),
    (
        "source-method",
        "Phase 2 · a resolved source method admits nothing but its own call",
    ),
    (
        "source-method-arg",
        "M3 G2 · a phantom argument in `all()`'s own call, past the bitemporal \
         milestoning carve-out",
    ),
    ("n1-member", "M3 G2 · a phantom property after a bound var"),
    (
        "n1-fused-navdot",
        "M3 G2 · a phantom member fused with the navigation dot",
    ),
    (
        "n1-fused-nav-hop",
        "M3 G2 · a fused member on the hop after an association step reaches a class",
    ),
    (
        "n2-association",
        "M3 G2 · a phantom after an association step advances the class",
    ),
    (
        "t1-revalue",
        "M3 G3 · a literal whose kind mismatches the property's declared type",
    ),
    (
        "t2-comparator",
        "M3 G3 · an ordered comparator on a non-ordered operand",
    ),
    (
        "t3-reducer",
        "M3 G3 · a reducer whose kind mismatches the reduce lambda's element",
    ),
    (
        "t6-ordered-operand",
        "#116 T6 · an ordered comparator on a navExpr that is not a scalar \
         primitive — a collection, an extent navigation, or a class",
    ),
    (
        "t6-nonscalar-operator",
        "Phase 10 · bucket E — the logical and arithmetic operators on the same \
         non-scalar navExpr, and the fourth shape that reaches it: a \
         type-preserving call off the class extent",
    ),
    (
        "t4-string-method",
        "#116 T4 · a String-only method arrowed off a receiver the overlay has \
         typed non-String",
    ),
    (
        "n6-column",
        "M3 G2 · a TDS getter naming a column no `project` emitted",
    ),
    (
        "n6-relation-column",
        "arm-R · a bare-ident column access no `~`-construct emitted",
    ),
    (
        "n6-relation-column-fused",
        "arm-R · the same column access, fused with the navigation dot",
    ),
    (
        "n1-project-map-binder",
        "arm-R · a `project` map binder stays narrowed to the source class",
    ),
    (
        "oos-held-out",
        "G5 · phantoms masked on the held-out schemas no rule was authored against",
    ),
];

/// Every frozen precision fixture issue #55's phases produced, in one table.
///
/// The per-rule tests below read slices of it through [`frozen`];
/// [`every_rule_kind_has_a_frozen_walk_that_it_closes`] reads all of it. Adding
/// a fixture means adding a row here, not a new list somewhere else — a fixture
/// outside this table is invisible to the coverage gate, which is exactly the
/// hole the table exists to close.
static FROZEN_KILLS: &[FrozenKill] = &[
    FrozenKill {
        fixture: "n3-classpath",
        db: "world_1",
        closer: Closer::L2("SourceIdent"),
        kill: Kill::Walk {
            walk: "{|spider::world_1::Db::desc->min('_v')}",
            closed_by: "spider::world_1::Db::desc",
        },
    },
    FrozenKill {
        fixture: "n3-classpath",
        db: "world_1",
        closer: Closer::L2("SourceIdent"),
        kill: Kill::Walk {
            walk: "\n|spider::world_1::model::default::Countrylanguage::name\
             ->distinct('asia'!='GovernmentForm_T1_3')",
            closed_by: "spider::world_1::model::default::Countrylanguage::name",
        },
    },
    FrozenKill {
        fixture: "n3-classpath",
        db: "world_1",
        closer: Closer::L2("SourceIdent"),
        kill: Kill::Walk {
            walk: "{\n    |spider::world_1::model::default::Countrylanguage::pair\
             ->concatenate(c)+Integer}",
            closed_by: "spider::world_1::model::default::Countrylanguage::pair",
        },
    },
    FrozenKill {
        fixture: "n3-classpath",
        db: "world_1",
        closer: Closer::L2("SourceIdent"),
        kill: Kill::Walk {
            walk: "|spider::world_1::model::default::Countrylanguage::limit\
             ->isEmpty('GovernmentForm_T1')",
            closed_by: "spider::world_1::model::default::Countrylanguage::limit",
        },
    },
    FrozenKill {
        fixture: "n3-classpath",
        db: "world_1",
        closer: Closer::L2("SourceIdent"),
        kill: Kill::Walk {
            walk: "{     \n    |spider::world_1::Db::name::language->distinct(row!=.3000)*limit}",
            closed_by: "spider::world_1::Db::name::language",
        },
    },
    FrozenKill {
        fixture: "n3-classpath",
        db: "world_1",
        closer: Closer::L2("SourceIdent"),
        kill: Kill::Walk {
            walk: "|spider::world_1::model::default::Country::distinct::Y->min()",
            closed_by: "spider::world_1::model::default::Country::distinct::Y",
        },
    },
    FrozenKill {
        fixture: "n3-classpath",
        db: "world_1",
        closer: Closer::L2("SourceIdent"),
        kill: Kill::Walk {
            walk: "{\n    |l::filter->project(renameColumns)}",
            closed_by: "l::filter",
        },
    },
    FrozenKill {
        fixture: "n3-classpath",
        db: "world_1",
        closer: Closer::L2("SourceIdent"),
        kill: Kill::Walk {
            walk: "|spider::world_1::model::default::Country::min->limit('Capital_t1')",
            closed_by: "spider::world_1::model::default::Country::min",
        },
    },
    FrozenKill {
        fixture: "n3-classpath",
        db: "world_1",
        closer: Closer::L2("SourceIdent"),
        kill: Kill::Walk {
            walk: "|spider::world_1::model::default::Country::row1\
             ::spider::world_1::model::default::Countrylanguage::pair::groupBy\
             ->restrict('IndepYear'&&'_nn'!='_ord0'+'Percentage_T2_2'!='GNPOld_T1_1')",
            closed_by: "spider::world_1::model::default::Country::row1\
             ::spider::world_1::model::default::Countrylanguage::pair::groupBy",
        },
    },
    // S2's sigil half (issue #275): with nothing bound, the `$` itself is what is
    // illegal, so the walk is closed one token *earlier* than it used to be — at
    // the sigil rather than at the name it opens. The rule active where that mask
    // is read is the block-statement anchor's own `SourceIdent`; see `Closer::L2`.
    FrozenKill {
        fixture: "s2-refvar",
        db: "world_1",
        closer: Closer::L2("SourceIdent"),
        kill: Kill::Walk {
            walk: "{|\n        $code\n      /'IsOfficial_t2'}",
            closed_by: "$",
        },
    },
    FrozenKill {
        fixture: "s2-refvar",
        db: "world_1",
        closer: Closer::L2("SourceIdent"),
        kill: Kill::Walk {
            walk: "{\n|      $name}",
            closed_by: "$",
        },
    },
    FrozenKill {
        fixture: "n7-bare-source",
        db: "world_1",
        closer: Closer::L2("SourceIdent"),
        kill: Kill::Walk {
            walk: "|spider::world_1::model::default::Country->max(language)",
            closed_by: "->",
        },
    },
    FrozenKill {
        fixture: "n7-bare-source",
        db: "world_1",
        closer: Closer::L2("SourceIdent"),
        kill: Kill::Walk {
            walk: "|spider::world_1::model::default::Countrylanguage->pair(code    \n!='Name_T2')",
            closed_by: "->",
        },
    },
    FrozenKill {
        fixture: "n7-bare-source",
        db: "world_1",
        closer: Closer::L2("SourceIdent"),
        kill: Kill::Walk {
            walk: "|spider::world_1::model::default::Country->filter('Percentage_T2_4'<average)",
            closed_by: "->",
        },
    },
    FrozenKill {
        fixture: "n7-bare-source",
        db: "world_1",
        closer: Closer::L2("SourceIdent"),
        kill: Kill::Walk {
            walk: "|spider::world_1::model::default::Countrylanguage->between(renameColumns>'hasDutch')",
            closed_by: "->",
        },
    },
    FrozenKill {
        fixture: "n7-bare-source",
        db: "world_1",
        closer: Closer::L2("SourceIdent"),
        kill: Kill::Walk {
            walk: "{|spider::world_1::model::default::Country->between(join)<LEFT_OUTER}",
            closed_by: "->",
        },
    },
    FrozenKill {
        fixture: "n7-bare-source",
        db: "world_1",
        closer: Closer::L2("SourceIdent"),
        kill: Kill::Walk {
            walk: "|spider::world_1::model::default::Country->tableReference(restrict)",
            closed_by: "->",
        },
    },
    FrozenKill {
        fixture: "n7-bare-source",
        db: "world_1",
        closer: Closer::L2("SourceIdent"),
        kill: Kill::Walk {
            walk: "|spider::world_1::model::default::Countrylanguage\
             ->groupBy('Gelderland'||'Population_T1_1'&&asc)",
            closed_by: "->",
        },
    },
    FrozenKill {
        fixture: "n7-bare-source",
        db: "world_1",
        closer: Closer::L2("StoreMethod"),
        kill: Kill::Walk {
            walk: "{|spider::world_1::Db->concatenate('IndepYear_T1_1',desc-col=='IndepYear_country')}",
            closed_by: "concatenate",
        },
    },
    FrozenKill {
        fixture: "n7-bare-source",
        db: "world_1",
        closer: Closer::L2("SourceIdent"),
        kill: Kill::Walk {
            walk: "|spider::world_1::model::default::Countrylanguage->tableReference(pair)",
            closed_by: "->",
        },
    },
    FrozenKill {
        fixture: "n7-bare-source",
        db: "world_1",
        closer: Closer::L2("SourceIdent"),
        kill: Kill::Walk {
            walk: "|spider::world_1::model::default::Country->pair(tableReference)&&5",
            closed_by: "->",
        },
    },
    FrozenKill {
        fixture: "n7-bare-source",
        db: "world_1",
        closer: Closer::L2("SourceIdent"),
        kill: Kill::Walk {
            walk: "|spider::world_1::model::default::Country->col(between\n*'District_city')",
            closed_by: "->",
        },
    },
    FrozenKill {
        fixture: "n7-bare-source",
        db: "world_1",
        closer: Closer::L2("SourceIdent"),
        kill: Kill::Walk {
            walk: "|spider::world_1::model::default::Country->filter('SUM(SurfaceArea)'<agg/'_nn__t0anti1')",
            closed_by: "->",
        },
    },
    // N3i's own two kills, taken verbatim from the live lane on this branch (the
    // `car_1` exploration walks the phase set out to close), from the `{` their
    // leading whitespace run opens.
    FrozenKill {
        fixture: "n3i-scalar-receiver-method",
        db: "car_1",
        closer: Closer::L2("ScalarMethod"),
        kill: Kill::Walk {
            walk: "{\n\n\n     \n         \n  \n      \n      \n        \n      \n \n          |spider::car_1::model::default::CarMakers.\nall(\n)->project('car_makers'->tableToTDS()=='ContId'+'Weight_T3'>limit|'MakeId_T2'||spider::car_1::model::default::CarMakers<'Weight_T4')!='europe'}",
            closed_by: "(",
        },
    },
    FrozenKill {
        fixture: "n3i-scalar-receiver-method",
        db: "car_1",
        closer: Closer::L2("ScalarMethod"),
        kill: Kill::Walk {
            walk: "{ \n  \n      \n        |spider::car_1::model::default::CarMakers.\n      \n           all(\n  \n        )\n    ->isNotEmpty()->restrict('Horsepower_T2')=='Id_T2'&&'Accelerate_T2_2'==b||String}",
            closed_by: "(",
        },
    },
    // The two receiver routes the live walks above do not reach — a
    // receiver-only builtin's fixed `Integer[1]` and a bare `String[0..1]`
    // property navigation. Stated as walks rather than one-token probes because,
    // like N3f's, the clear lands on the token that *closes* the denied name
    // (`(`), never on the name itself.
    FrozenKill {
        fixture: "n3i-scalar-receiver-method",
        db: "car_1",
        closer: Closer::L2("ScalarMethod"),
        kill: Kill::Walk {
            walk: "{|spider::car_1::model::default::CarMakers.all()->count()->renameColumns('_c1')}",
            closed_by: "(",
        },
    },
    FrozenKill {
        fixture: "n3i-scalar-receiver-method",
        db: "car_1",
        closer: Closer::L2("ScalarMethod"),
        kill: Kill::Walk {
            walk: "{|spider::car_1::model::default::CarMakers.all()\
             ->filter(x|$x.maker->restrict('_c1'))}",
            closed_by: "(",
        },
    },
    FrozenKill {
        fixture: "n3f-extent-method",
        db: "world_1",
        closer: Closer::L2("ExtentMethod"),
        kill: Kill::Walk {
            walk: "{|spider::world_1::model::default::Countrylanguage.all()\
             ->pair('LifeExpectancy_T3_1')!='GNP_T3_1'}",
            closed_by: "(",
        },
    },
    // The four names the same relation/store evidence adds to N3f's own deny set
    // (`RELATION_RECEIVER_METHODS`), each refused live on a class extent with the
    // relation-only candidate set named back.
    FrozenKill {
        fixture: "n3f-extent-method",
        db: "world_1",
        closer: Closer::L2("ExtentMethod"),
        kill: Kill::Walk {
            walk: "{|spider::world_1::model::default::Country.all()->extend('_c1')}",
            closed_by: "(",
        },
    },
    FrozenKill {
        fixture: "n3f-extent-method",
        db: "world_1",
        closer: Closer::L2("ExtentMethod"),
        kill: Kill::Walk {
            walk: "{|spider::world_1::model::default::Country.all()->pivot('_c1')}",
            closed_by: "(",
        },
    },
    FrozenKill {
        fixture: "n3f-extent-method",
        db: "world_1",
        closer: Closer::L2("ExtentMethod"),
        kill: Kill::Walk {
            walk: "{|spider::world_1::model::default::Country.all()->asOfJoin('_c1')}",
            closed_by: "(",
        },
    },
    FrozenKill {
        fixture: "n3f-extent-method",
        db: "world_1",
        closer: Closer::L2("ExtentMethod"),
        kill: Kill::Walk {
            walk: "{|spider::world_1::model::default::Country.all()->olapGroupBy('_c1')}",
            closed_by: "(",
        },
    },
    FrozenKill {
        fixture: "n3f-extent-method",
        db: "world_1",
        closer: Closer::L2("ExtentMethod"),
        kill: Kill::Walk {
            walk: "{|spider::world_1::model::default::Countrylanguage.all()\
             ->average('LocalName_T3_1')||'dutch'==160000/'CountryCode_city'\
             ||'GNPOld_t1'::count&&'ID_T1_1'}",
            closed_by: "(",
        },
    },
    FrozenKill {
        fixture: "n3f-extent-method",
        db: "world_1",
        closer: Closer::L2("ExtentMethod"),
        kill: Kill::Walk {
            walk: "{|spider::world_1::model::default::Countrylanguage.all()->agg(.1950)}",
            closed_by: "(",
        },
    },
    FrozenKill {
        fixture: "n3f-extent-method",
        db: "world_1",
        closer: Closer::L2("ExtentMethod"),
        kill: Kill::Walk {
            walk: "{|spider::world_1::model::default::Country.all()->join('_c1')}",
            closed_by: "(",
        },
    },
    FrozenKill {
        fixture: "n3f-extent-method",
        db: "car_1",
        closer: Closer::L2("ExtentMethod"),
        kill: Kill::Walk {
            walk: "{|spider::car_1::model::default::CarMakers.all()\
             ->between(max('American Motor Company'))*'cnt'\
             ::spider::car_1::model::default::CarMakers<=col||'Model_t1'}",
            closed_by: "(",
        },
    },
    FrozenKill {
        fixture: "n3f-extent-method",
        db: "car_1",
        closer: Closer::L2("ExtentMethod"),
        kill: Kill::Walk {
            walk: "{|spider::car_1::model::default::CarMakers.all()->join('car_names')}",
            closed_by: "(",
        },
    },
    FrozenKill {
        fixture: "n3f-extent-method",
        db: "world_1",
        closer: Closer::L2("ExtentMethod"),
        kill: Kill::Walk {
            walk: "{|spider::world_1::model::default::Country.all()->renameColumns('a','b')}",
            closed_by: "(",
        },
    },
    FrozenKill {
        fixture: "n3f-extent-method",
        db: "world_1",
        closer: Closer::L2("ExtentMethod"),
        kill: Kill::Walk {
            walk: "{|spider::world_1::model::default::Country.all()->restrict('Population_T3')}",
            closed_by: "(",
        },
    },
    FrozenKill {
        fixture: "n3f-extent-method",
        db: "world_1",
        closer: Closer::L2("ExtentMethod"),
        kill: Kill::Walk {
            walk: "{|spider::world_1::model::default::Country.all()->tableReference('default','country')}",
            closed_by: "(",
        },
    },
    FrozenKill {
        fixture: "n3f-extent-method",
        db: "world_1",
        closer: Closer::L2("ExtentMethod"),
        kill: Kill::Walk {
            walk: "{|spider::world_1::model::default::Country.all()->tableToTDS()}",
            closed_by: "(",
        },
    },
    FrozenKill {
        fixture: "n3f-extent-method",
        db: "world_1",
        closer: Closer::L2("ExtentMethod"),
        kill: Kill::Walk {
            walk: "{|spider::world_1::model::default::Country.all()->endsWith('a')}",
            closed_by: "(",
        },
    },
    FrozenKill {
        fixture: "n3f-extent-method",
        db: "world_1",
        closer: Closer::L2("ExtentMethod"),
        kill: Kill::Walk {
            walk: "{|spider::world_1::model::default::Country.all()->in('a')}",
            closed_by: "(",
        },
    },
    FrozenKill {
        fixture: "n3f-extent-method",
        db: "world_1",
        closer: Closer::L2("ExtentMethod"),
        kill: Kill::Walk {
            walk: "{|spider::world_1::model::default::Country.all()->parseFloat('1')}",
            closed_by: "(",
        },
    },
    FrozenKill {
        fixture: "n3f-extent-method",
        db: "world_1",
        closer: Closer::L2("ExtentMethod"),
        kill: Kill::Walk {
            walk: "{|spider::world_1::model::default::Country.all()->startsWith('a')}",
            closed_by: "(",
        },
    },
    FrozenKill {
        fixture: "n3f-extent-method",
        db: "world_1",
        closer: Closer::L2("ExtentMethod"),
        kill: Kill::Walk {
            walk: "{|spider::world_1::model::default::Country.all()->substring(1,2)}",
            closed_by: "(",
        },
    },
    FrozenKill {
        fixture: "n3f-extent-method",
        db: "world_1",
        closer: Closer::L2("ExtentMethod"),
        kill: Kill::Walk {
            walk: "{|spider::world_1::model::default::Country.all()->sum('a')}",
            closed_by: "(",
        },
    },
    FrozenKill {
        fixture: "n3f-extent-method",
        db: "world_1",
        closer: Closer::L2("ExtentMethod"),
        kill: Kill::Walk {
            walk: "{|spider::world_1::model::default::Country.all()->toLower('a')}",
            closed_by: "(",
        },
    },
    FrozenKill {
        fixture: "n3f-extent-method",
        db: "world_1",
        closer: Closer::L2("ExtentMethod"),
        kill: Kill::Walk {
            walk: "{|spider::world_1::model::default::Country.all()->toString('a')}",
            closed_by: "(",
        },
    },
    FrozenKill {
        fixture: "n3f-extent-method",
        db: "world_1",
        closer: Closer::L2("ExtentMethod"),
        kill: Kill::Walk {
            walk: "{|spider::world_1::model::default::Country.all()->year('a')}",
            closed_by: "(",
        },
    },
    // N3h — the argument half of N3f's position. Both names are legal on a
    // `T[*]` extent and wrong only in what they are called with, so the
    // receiver-category rule cannot reach either.
    FrozenKill {
        fixture: "n3h-extent-method-arg",
        db: "car_1",
        closer: Closer::L2("ExtentMethodArg"),
        kill: Kill::Walk {
            walk: "{|spider::car_1::model::default::CarMakers.all()->groupBy('Edispl_T4')}",
            closed_by: "'Edispl_T4'",
        },
    },
    FrozenKill {
        fixture: "n3h-extent-method-arg",
        db: "world_1",
        closer: Closer::L2("ExtentMethodArg"),
        kill: Kill::Walk {
            walk: "{|spider::world_1::model::default::Countrylanguage.all()\
             ->groupBy('Percentage_t2'<'country')}",
            closed_by: "'Percentage_t2'",
        },
    },
    FrozenKill {
        fixture: "n3h-extent-method-arg",
        db: "car_1",
        closer: Closer::L2("ExtentMethodArg"),
        kill: Kill::Walk {
            walk: "{|spider::car_1::model::default::CarMakers.all()->limit('MPG_T2_2')}",
            closed_by: "'MPG_T2_2'",
        },
    },
    // The two shapes' whole difference, pinned as a contrast: the `Integer` slot
    // keeps a numeric literal that the `Function` slot clears, and both keep the
    // opener a real argument list starts with.
    FrozenKill {
        fixture: "n3h-extent-method-arg",
        db: "car_1",
        closer: Closer::L2("ExtentMethodArg"),
        kill: Kill::Probe {
            prefix: "|spider::car_1::model::default::CarMakers.all()->limit(",
            real: "3",
            phantom: "'MPG_T2_2'",
        },
    },
    FrozenKill {
        fixture: "n3h-extent-method-arg",
        db: "car_1",
        closer: Closer::L2("ExtentMethodArg"),
        kill: Kill::Probe {
            prefix: "|spider::car_1::model::default::CarMakers.all()->groupBy(",
            real: "[",
            phantom: "1",
        },
    },
    FrozenKill {
        fixture: "n7-extent",
        db: "world_1",
        closer: Closer::L2("ValueIdent"),
        kill: Kill::Walk {
            walk: "|spider::world_1::model::default::Country.all()->max(language)",
            closed_by: ")",
        },
    },
    FrozenKill {
        fixture: "n7-extent",
        db: "world_1",
        closer: Closer::L2("ValueIdent"),
        kill: Kill::Walk {
            walk: "|spider::world_1::model::default::Country.all()\
             ->filter('SUM(SurfaceArea)'<agg/'_nn__t0anti1')",
            closed_by: "/",
        },
    },
    FrozenKill {
        fixture: "n7-extent",
        db: "world_1",
        closer: Closer::L2("ValueIdent"),
        kill: Kill::Walk {
            walk: "|spider::world_1::model::default::Country.all()->limit(tableReference)&&5",
            closed_by: ")",
        },
    },
    FrozenKill {
        fixture: "n7-extent",
        db: "world_1",
        closer: Closer::L2("ValueIdent"),
        kill: Kill::Walk {
            walk: "|spider::world_1::model::default::Countrylanguage.all()\
             ->sort(code    \n!='Name_T2')",
            closed_by: "    \n",
        },
    },
    FrozenKill {
        fixture: "n7-extent",
        db: "world_1",
        closer: Closer::L2("ValueIdent"),
        kill: Kill::Walk {
            walk: "|spider::world_1::model::default::Country.all()->col(between\n*'District_city')",
            closed_by: "\n",
        },
    },
    FrozenKill {
        fixture: "n3c-class",
        db: "world_1",
        closer: Closer::L2("SourceIdent"),
        kill: Kill::Walk {
            walk: "\n        |spider::world_1::model::default::Countrylanguage->agg('Central Africa')",
            closed_by: "->",
        },
    },
    FrozenKill {
        fixture: "n3c-class",
        db: "world_1",
        closer: Closer::L2("SourceIdent"),
        kill: Kill::Walk {
            walk: "\n\n \n       \n           |spider::world_1::model::default::Country->col(between.'HeadOfState_T1'!='Brazil')",
            closed_by: "->",
        },
    },
    FrozenKill {
        fixture: "n3c-class",
        db: "world_1",
        closer: Closer::L2("SourceIdent"),
        kill: Kill::Walk {
            walk: "\n              \n    \n        \n  \n    \n        |spider::world_1::model::default::Countrylanguage->count('Beatrix'&&'AVG(LifeExpectancy)')",
            closed_by: "->",
        },
    },
    FrozenKill {
        fixture: "n3c-class",
        db: "world_1",
        closer: Closer::L2("SourceIdent"),
        kill: Kill::Walk {
            walk: "\n\n    \n|spider::world_1::model::default::Country->distinct('Angola')",
            closed_by: "->",
        },
    },
    FrozenKill {
        fixture: "n3c-class",
        db: "world_1",
        closer: Closer::L2("SourceIdent"),
        kill: Kill::Walk {
            walk: "\n    \n  \n        \n        |spider::world_1::model::default::Country->filter('Percentage_T2_4'<average.'HeadOfState_country')",
            closed_by: "->",
        },
    },
    FrozenKill {
        fixture: "n3c-class",
        db: "world_1",
        closer: Closer::L2("SourceIdent"),
        kill: Kill::Walk {
            walk: "\n|spider::world_1::model::default::Country->groupBy('CountryCode_T2_2')",
            closed_by: "->",
        },
    },
    FrozenKill {
        fixture: "n3c-class",
        db: "world_1",
        closer: Closer::L2("SourceIdent"),
        kill: Kill::Walk {
            walk: "|spider::world_1::model::default::Countrylanguage->isEmpty('_k0')",
            closed_by: "->",
        },
    },
    FrozenKill {
        fixture: "n3c-class",
        db: "world_1",
        closer: Closer::L2("SourceIdent"),
        kill: Kill::Walk {
            walk: "  |spider::world_1::model::default::Countrylanguage->join('District_city'\n  .renameColumns||'Europe')",
            closed_by: "->",
        },
    },
    FrozenKill {
        fixture: "n3c-class",
        db: "world_1",
        closer: Closer::L2("SourceIdent"),
        kill: Kill::Walk {
            walk: "|spider::world_1::model::default::Countrylanguage->max(b.'Region_T3_1')",
            closed_by: "->",
        },
    },
    FrozenKill {
        fixture: "n3c-class",
        db: "world_1",
        closer: Closer::L2("SourceIdent"),
        kill: Kill::Walk {
            walk: "\n             |spider::world_1::model::default::Country->restrict()",
            closed_by: "->",
        },
    },
    FrozenKill {
        fixture: "n3c-class",
        db: "world_1",
        closer: Closer::L2("SourceIdent"),
        kill: Kill::Walk {
            walk: "\n    \n    |spider::world_1::model::default::Country->sort('Population_T3_1')",
            closed_by: "->",
        },
    },
    FrozenKill {
        fixture: "n3c-class",
        db: "world_1",
        closer: Closer::L2("SourceIdent"),
        kill: Kill::Walk {
            walk: "\n    \n         \n  \n        \n      |spider::world_1::model::default::Countrylanguage->sum('GNPOld_T1_3'>'country')",
            closed_by: "->",
        },
    },
    FrozenKill {
        fixture: "n3c-class",
        db: "world_1",
        closer: Closer::L2("SourceIdent"),
        kill: Kill::Walk {
            walk: "\n           |spider::world_1::model::default::Countrylanguage->tableReference(pair|'Angola'\n    )",
            closed_by: "->",
        },
    },
    FrozenKill {
        fixture: "n3c-class",
        db: "car_1",
        closer: Closer::L2("SourceIdent"),
        kill: Kill::Walk {
            walk: "|spider::car_1::model::default::CarsData->col(3,'FullName_t1_1')",
            closed_by: "->",
        },
    },
    FrozenKill {
        fixture: "n3c-class",
        db: "car_1",
        closer: Closer::L2("SourceIdent"),
        kill: Kill::Walk {
            walk: "\n\n    |spider::car_1::model::default::CarsData->count('CountryName'<='Model_T1')",
            closed_by: "->",
        },
    },
    FrozenKill {
        fixture: "n3c-class",
        db: "car_1",
        closer: Closer::L2("SourceIdent"),
        kill: Kill::Walk {
            walk: "|spider::car_1::model::default::CarMakers->extend('Continent_T3')",
            closed_by: "->",
        },
    },
    FrozenKill {
        fixture: "n3c-class",
        db: "car_1",
        closer: Closer::L2("SourceIdent"),
        kill: Kill::Walk {
            walk: "  \n      \n        \n  \n        \n    \n\n \n       \n           |spider::car_1::model::default::ModelList->filter('null')",
            closed_by: "->",
        },
    },
    FrozenKill {
        fixture: "n3c-class",
        db: "car_1",
        closer: Closer::L2("SourceIdent"),
        kill: Kill::Walk {
            walk: "    |spider::car_1::model::default::ModelList->groupBy('Maker_T2_3'>|'Accelerate_T2_2'    )",
            closed_by: "->",
        },
    },
    FrozenKill {
        fixture: "n3c-class",
        db: "car_1",
        closer: Closer::L2("SourceIdent"),
        kill: Kill::Walk {
            walk: "\n        \n    \n      \n\n\n        \n  |spider::car_1::model::default::CarsData->project('CountryName')",
            closed_by: "->",
        },
    },
    FrozenKill {
        fixture: "n3c-class",
        db: "car_1",
        closer: Closer::L2("SourceIdent"),
        kill: Kill::Walk {
            walk: "|spider::car_1::model::default::ModelList->restrict(fk4DefaultCarsData.'Maker_t2_4')",
            closed_by: "->",
        },
    },
    FrozenKill {
        fixture: "n3c-class",
        db: "car_1",
        closer: Closer::L2("SourceIdent"),
        kill: Kill::Walk {
            walk: "\n        \n  |spider::car_1::model::default::CarsData->year('Horsepower_T1'\n=='cars_data'||'_c0__t0l0'!='cars_data'\n  *'Country_T1'    )",
            closed_by: "->",
        },
    },
    FrozenKill {
        fixture: "n3c-store-all",
        db: "world_1",
        closer: Closer::L2("SourceIdent"),
        kill: Kill::Walk {
            walk: "\n    {\n          \n          |\n        \n    spider::world_1::Db.\n    \n    \n      \n  all()}",
            closed_by: ".",
        },
    },
    FrozenKill {
        fixture: "n3c-store-all",
        db: "car_1",
        closer: Closer::L2("SourceIdent"),
        kill: Kill::Walk {
            walk: "          \n    \n      \n  \n        {\n  \n        \n        \n    |spider::car_1::Db.all()}",
            closed_by: ".",
        },
    },
    FrozenKill {
        fixture: "n3c-store-method",
        db: "world_1",
        closer: Closer::L2("StoreMethod"),
        kill: Kill::Walk {
            walk: "{|spider::world_1::Db->max('CountryCode_T2_2')(isEmpty:limit&&'CountryCode_t3')}",
            closed_by: "max",
        },
    },
    FrozenKill {
        fixture: "n3c-store-method",
        db: "world_1",
        closer: Closer::L2("StoreMethod"),
        kill: Kill::Walk {
            walk: "\n  \n      \n{\n      |spider::world_1::Db->String('GNP_t1'*'HeadOfState_T3_1' )!=getFloat('CountryCode_t2'  \n      )==getInteger|'AVG(GNP)'!='Continent_T1_1'}",
            closed_by: "String",
        },
    },
    FrozenKill {
        fixture: "n3c-store-method",
        db: "car_1",
        closer: Closer::L2("StoreMethod"),
        kill: Kill::Walk {
            walk: "\n    \n\n        {\n\n      |spider::car_1::Db->project('MakeId_T1'\n         =='MPG')  }",
            closed_by: "project",
        },
    },
    FrozenKill {
        fixture: "n3c-store-method",
        db: "car_1",
        closer: Closer::L2("StoreMethod"),
        kill: Kill::Walk {
            walk: "\n    {|spider::car_1::Db->exists()|year('ModelId'<='MPG_T1'.'Country'-weight('Id_T1_1','volvo'\n    )&&'Model_T2'+'car_names'    !='$)a)parseFloat<,4000}(tableToTDS)]}else",
            closed_by: "exists",
        },
    },
    FrozenKill {
        fixture: "n3c-cost",
        db: "world_1",
        closer: Closer::L2("SourceIdent"),
        kill: Kill::Walk {
            walk: "\n        |spider::world_1::model::default::Country->pair('US Territory')",
            closed_by: "->",
        },
    },
    FrozenKill {
        fixture: "n3c-cost",
        db: "world_1",
        closer: Closer::L2("SourceIdent"),
        kill: Kill::Walk {
            walk: "\n        |spider::world_1::model::default::Country->limit(1930)",
            closed_by: "->",
        },
    },
    FrozenKill {
        fixture: "n3c-cost",
        db: "car_1",
        closer: Closer::L2("SourceIdent"),
        kill: Kill::Walk {
            walk: "\n        \n    |spider::car_1::model::default::ModelList->max()",
            closed_by: "->",
        },
    },
    FrozenKill {
        fixture: "n3c-cost",
        db: "car_1",
        closer: Closer::L2("SourceIdent"),
        kill: Kill::Walk {
            walk: "\n  |spider::car_1::model::default::CarsData->concatenate(3  )",
            closed_by: "->",
        },
    },
    FrozenKill {
        fixture: "n3c-cost",
        db: "car_1",
        closer: Closer::L2("SourceIdent"),
        kill: Kill::Walk {
            walk: "\n    \n      \n      \n        \n        |\n      spider::car_1::model::default::ModelList->concatenate('CountryId_T1'+'Id_T2_3'\n  )",
            closed_by: "->",
        },
    },
    FrozenKill {
        fixture: "n3d-arg-separator",
        db: "world_1",
        closer: Closer::L2("StoreMethodArgSep"),
        kill: Kill::Walk {
            walk: "  \n      \n        \n         \n     \n        \n     \n            \n         \n    \n  \n        \n        \n            \n     \n      \n    \n    \n  \n        \n          \n    {    \n    \n      \n      \n    \n        \n   \n            |    \n        spider::world_1::Db->tableReference('Code_T1_3') }",
            closed_by: ")",
        },
    },
    FrozenKill {
        fixture: "n3d-arg-separator",
        db: "world_1",
        closer: Closer::L2("StoreMethodArgSep"),
        kill: Kill::Walk {
            walk: "  \n    \n    \n  \n    \n        \n\n        \n        {\n      | \n        \n\n    spider::world_1::Db->tableReference('Continent_T1_3'=='GovernmentForm_T3_1'>'dutch')&&'IndepYear_country'&&'_c0__t0r0'}",
            closed_by: "==",
        },
    },
    FrozenKill {
        fixture: "n3d-arg-separator",
        db: "car_1",
        closer: Closer::L2("StoreMethodArgSep"),
        kill: Kill::Walk {
            walk: "   {    \n         \n  \n\n      |spider::car_1::Db->tableReference('MakeId_T1'\n         =='MPG')  }",
            closed_by: "==",
        },
    },
    // N3d's *arity* half, which the three walks above never reach: they all die
    // at the separator after a completed argument, so `StoreMethodArg` — the
    // open slot itself — had no frozen fixture at all until
    // `every_rule_kind_has_a_frozen_walk_that_it_closes` said so. The walk is
    // the live lane's own (it is frozen for a different, later rule in
    // `precision_reject::a_juxtaposed_application_off_a_non_name_dies`, which
    // reaches it only because that lane runs L1 alone); under the schema overlay
    // an opened slot owes its argument, so the zero-argument call cannot be
    // walked at all and dies on the closer instead.
    FrozenKill {
        fixture: "n3d-open-arg-slot",
        db: "world_1",
        closer: Closer::L2("StoreMethodArg"),
        kill: Kill::Walk {
            walk: "  {|spider::world_1::Db->tableReference()(isEmpty:limit&&'CountryCode_t3')}",
            closed_by: ")",
        },
    },
    // The same rule at its other anchor: the slot a `,` opens owes an argument
    // too, so a one-argument call cannot be closed there either.
    FrozenKill {
        fixture: "n3e-extent-operator",
        db: "car_1",
        closer: Closer::L2("SourceExtent"),
        kill: Kill::Walk {
            walk: "  {\n\n  \n      \n      \n  \n        \n    \n      \n    |spider::car_1::model::default::ModelList.    \n    \n  all()&&'usa'}",
            closed_by: "&&",
        },
    },
    FrozenKill {
        fixture: "n3e-extent-operator",
        db: "car_1",
        closer: Closer::L2("SourceExtent"),
        kill: Kill::Walk {
            walk: "  {\n          \n        \n        |spider::car_1::model::default::ModelList.   all(  )&&'MPG_T3'}",
            closed_by: "&&",
        },
    },
    FrozenKill {
        fixture: "n1-extent-dot",
        db: "world_1",
        closer: Closer::L2("Member"),
        kill: Kill::Walk {
            walk: "   \n       \n      \n    \n      {      \n|\n\n          spider::world_1::model::default::Countrylanguage.\n  \n    \n  \n        all(\n    \n\n)\n    .'Capital_T1'  }",
            closed_by: "'Capital_T1'",
        },
    },
    FrozenKill {
        fixture: "n1-extent-dot",
        db: "world_1",
        closer: Closer::L2("Member"),
        kill: Kill::Walk {
            walk: "  \n  \n          \n          { \n  \n      \n        |spider::world_1::model::default::Countrylanguage.\n      \n           all(\n  \n        ).'Code';}",
            closed_by: "'Code'",
        },
    },
    FrozenKill {
        fixture: "n1-extent-dot",
        db: "car_1",
        closer: Closer::L2("Member"),
        kill: Kill::Walk {
            walk: "  \n      \n\n    \n    \n      \n      {\n    \n      \n        \n        \n  \n        \n        \n \n        \n       \n\n\n          \n        \n        |spider::car_1::model::default::ModelList.   all(  ).'_c1'}",
            closed_by: "'_c1'",
        },
    },
    FrozenKill {
        fixture: "n1-extent-dot",
        db: "car_1",
        closer: Closer::L2("Member"),
        kill: Kill::Walk {
            walk: "  \n      \n    \n  \n        \n          \n      {\n  |\n\n      spider::car_1::model::default::ModelList. all()\n    \n\n      .'Id_T2_2'}",
            closed_by: "'Id_T2_2'",
        },
    },
    FrozenKill {
        fixture: "n1-extent-dot",
        db: "world_1",
        closer: Closer::L2("Member"),
        kill: Kill::Walk {
            walk: "  \n    \n        \n\n      {|       \n      \n  \n        \n    \n     \n        spider::world_1::model::default::Country.\n        \n      \n         all(\n  \n      ).sort ||'Language_t2'=='Code2_T1_3'\n+'countrylanguage'-getInteger||spider::world_1::model::default::Countrylanguage}",
            closed_by: "sort",
        },
    },
    FrozenKill {
        fixture: "n1-extent-dot",
        db: "car_1",
        closer: Closer::L2("Member"),
        kill: Kill::Walk {
            walk: "   \n       \n      \n    \n      {      \n|\n\n          spider::car_1::model::default::CarMakers.\n  \n    \n  \n        all(\n    \n\n)\n      \n     \n        \n.col    ->tableReference('Horsepower_T1'   ,'_v__t0sc0'  )*concatenate('CountryName_T1_2').'COUNT()'\n        \n}",
            closed_by: "col",
        },
    },
    FrozenKill {
        fixture: "n3-let-prefix",
        db: "world_1",
        closer: Closer::L2("SourceIdent"),
        kill: Kill::Walk {
            walk: "{|l->pair(col>'SUM(SurfaceArea)')}",
            closed_by: "->",
        },
    },
    FrozenKill {
        fixture: "n3-source-class",
        db: "car_1",
        closer: Closer::L2("SourceIdent"),
        kill: Kill::Probe {
            prefix: "|",
            real: "spider::car_1::model::default::CarsData",
            phantom: "spider::car_1::model::default::DoesNotExist",
        },
    },
    FrozenKill {
        fixture: "n3-source-class",
        db: "car_1",
        closer: Closer::L2("SourceIdent"),
        kill: Kill::Probe {
            prefix: "|",
            real: "spider::car_1::Db",
            phantom: "spider::car_1::Nope",
        },
    },
    FrozenKill {
        fixture: "source-method",
        db: "world_1",
        closer: Closer::L2("SourceMethod"),
        kill: Kill::Probe {
            prefix: "|spider::world_1::model::default::Country.all",
            real: "(",
            phantom: "->",
        },
    },
    FrozenKill {
        fixture: "source-method",
        db: "world_1",
        closer: Closer::L2("SourceMethod"),
        kill: Kill::Probe {
            prefix: "|spider::world_1::model::default::Country.all",
            real: "(",
            phantom: " ",
        },
    },
    FrozenKill {
        fixture: "source-method",
        db: "world_1",
        closer: Closer::L2("SourceMethod"),
        kill: Kill::Probe {
            prefix: "|spider::world_1::model::default::Country.all",
            real: "(",
            phantom: ".",
        },
    },
    // A closer cannot follow a bare name at all, so the byte-PDA refuses this one
    // before S1 is consulted: kept as the frozen contrast it always was, but it
    // is not what proves S1 fires — the other four probes in this family are.
    FrozenKill {
        fixture: "source-method",
        db: "world_1",
        closer: Closer::AlsoL1("SourceMethod"),
        kill: Kill::Probe {
            prefix: "|spider::world_1::model::default::Country.all",
            real: "(",
            phantom: ")",
        },
    },
    FrozenKill {
        fixture: "source-method",
        db: "world_1",
        closer: Closer::L2("SourceMethod"),
        kill: Kill::Probe {
            prefix: "|spider::world_1::model::default::Country.all",
            real: "(",
            phantom: "x",
        },
    },
    FrozenKill {
        fixture: "source-method-arg",
        db: "car_1",
        closer: Closer::L2("SourceMethodArg"),
        kill: Kill::Probe {
            prefix: "|spider::car_1::model::default::CarsData.all(",
            real: ")",
            phantom: "'French'",
        },
    },
    FrozenKill {
        fixture: "source-method-arg",
        db: "car_1",
        closer: Closer::L2("SourceMethodArg"),
        kill: Kill::Probe {
            prefix: "|spider::car_1::model::default::CarsData.all(",
            real: ")",
            phantom: "all",
        },
    },
    FrozenKill {
        fixture: "source-method-arg",
        db: "car_1",
        closer: Closer::L2("SourceMethodArg"),
        kill: Kill::Probe {
            prefix: "|spider::car_1::model::default::CarsData.all(",
            real: "%latest",
            phantom: "'French'",
        },
    },
    FrozenKill {
        fixture: "n1-member",
        db: "car_1",
        closer: Closer::L2("Member"),
        kill: Kill::Probe {
            prefix: "|spider::car_1::model::default::CarsData.all()->filter(x|$x.",
            real: "cylinders",
            phantom: "sallary",
        },
    },
    FrozenKill {
        fixture: "n1-member",
        db: "car_1",
        closer: Closer::L2("Member"),
        kill: Kill::Probe {
            prefix: "|spider::car_1::model::default::CarsData.all()->filter(x|$x.",
            real: "horsepower",
            phantom: "maker",
        },
    },
    FrozenKill {
        fixture: "n1-fused-navdot",
        db: "concert_singer",
        closer: Closer::L2("RefVar"),
        kill: Kill::Probe {
            prefix: "|spider::concert_singer::model::default::Concert.all()->filter(c|$c",
            real: ".theme",
            phantom: ".zzz",
        },
    },
    FrozenKill {
        fixture: "n1-fused-navdot",
        db: "concert_singer",
        closer: Closer::L2("RefVar"),
        kill: Kill::Probe {
            prefix: "|spider::concert_singer::model::default::Concert.all()->filter(c|$c",
            real: ".concertName",
            phantom: ".maker",
        },
    },
    FrozenKill {
        fixture: "n1-fused-nav-hop",
        db: "concert_singer",
        closer: Closer::L2("Member"),
        kill: Kill::Probe {
            prefix: "|spider::concert_singer::model::default::SingerInConcert.all()\
             ->filter(x|$x.fk2DefaultConcert",
            real: ".theme",
            phantom: ".zzz",
        },
    },
    FrozenKill {
        fixture: "n2-association",
        db: "car_1",
        closer: Closer::L2("Member"),
        kill: Kill::Probe {
            prefix: "|spider::car_1::model::default::ModelList.all()->filter(x|$x.fk2DefaultCarMakers.",
            real: "fullName",
            phantom: "cylinders",
        },
    },
    FrozenKill {
        fixture: "t1-revalue",
        db: "car_1",
        closer: Closer::L2("ReValue"),
        kill: Kill::Probe {
            prefix: "|spider::car_1::model::default::CarsData.all()->filter(x|$x.cylinders == ",
            real: "4",
            phantom: "'four'",
        },
    },
    FrozenKill {
        fixture: "t1-revalue",
        db: "car_1",
        closer: Closer::L2("ReValue"),
        kill: Kill::Probe {
            prefix: "|spider::car_1::model::default::CarsData.all()->filter(x|$x.horsepower == ",
            real: "'150'",
            phantom: "150",
        },
    },
    // `<<` is no comparator the emitted subset spells, so L1 refuses it on its
    // own; T2's own evidence is the ordered-comparator row above, where L1 admits
    // `<` and only the operand's declared type clears it.
    FrozenKill {
        fixture: "t2-comparator",
        db: "car_1",
        closer: Closer::AlsoL1("Comparator"),
        kill: Kill::Probe {
            prefix: "|spider::car_1::model::default::CarsData.all()->filter(x|$x.cylinders ",
            real: "<",
            phantom: "<<",
        },
    },
    // An open reduce lambda owes a term, so L1 refuses its closer whatever the
    // element type; T3's own evidence is the `sum`-on-`String[*]` row above.
    FrozenKill {
        fixture: "t3-reducer",
        db: "car_1",
        closer: Closer::AlsoL1("Reducer"),
        kill: Kill::Probe {
            prefix: "|spider::car_1::model::default::CarsData.all()->groupBy([], \
         [agg('X', row: meta::pure::tds::TDSRow[1]|$row.getInteger('Cylinders'), \
         y: Integer[*]|$y->",
            real: "sum",
            phantom: ")",
        },
    },
    FrozenKill {
        fixture: "t2-comparator",
        db: "car_1",
        closer: Closer::L2("Comparator"),
        kill: Kill::Probe {
            prefix: "|spider::car_1::model::default::CarsData.all()->filter(x|$x.horsepower ",
            real: "==",
            phantom: "<",
        },
    },
    FrozenKill {
        fixture: "t6-ordered-operand",
        db: "world_1",
        closer: Closer::L2("OrderedOperand"),
        kill: Kill::Probe {
            prefix: "|spider::world_1::model::default::Country.all()\
                     ->filter(c|$c.fk1DefaultCountrylanguage ",
            real: "==",
            phantom: "<",
        },
    },
    FrozenKill {
        fixture: "t6-ordered-operand",
        db: "car_1",
        closer: Closer::L2("OrderedOperand"),
        kill: Kill::Probe {
            prefix: "|spider::car_1::model::default::ModelList.all()\
                     ->filter(x|$x.fk3DefaultCarNames.model ",
            real: "==",
            phantom: "<",
        },
    },
    // The collapse the spec names is what a to-many navExpr is *for*, and it is
    // the shape both gold anchors take (`corpus/gold_queries.jsonl` car_1
    // `$x.fk3DefaultCarNames->exists(…)`, world_1
    // `$c.fk1DefaultCountrylanguage->filter(…)->isEmpty()`): the step arrow that
    // opens it must survive the very mask that clears the comparator.
    FrozenKill {
        fixture: "t6-ordered-operand",
        db: "car_1",
        closer: Closer::L2("OrderedOperand"),
        kill: Kill::Probe {
            prefix: "|spider::car_1::model::default::ModelList.all()\
                     ->filter(x|$x.fk3DefaultCarNames ",
            real: "->",
            phantom: ">",
        },
    },
    FrozenKill {
        fixture: "t6-ordered-operand",
        db: "world_1",
        closer: Closer::L2("OrderedOperand"),
        kill: Kill::Probe {
            prefix: "|spider::world_1::model::default::Country.all().gnp ",
            real: "==",
            phantom: ">=",
        },
    },
    FrozenKill {
        fixture: "t6-ordered-operand",
        db: "world_1",
        closer: Closer::L2("OrderedOperand"),
        kill: Kill::Probe {
            prefix: "|spider::world_1::model::default::Countrylanguage.all()\
                     ->filter(c|$c.fk1DefaultCountry ",
            real: "==",
            phantom: "<=",
        },
    },
    // Phase 10 widens the same position from the ordered comparators to every
    // operator family the engine declares over scalar operands only. The walk is
    // the live `car_1` failure it closes, whose `>=` sits behind a `*` no earlier
    // rule reached — and it needs the whitespace the live walk itself has, for
    // the reason `n4c-str-operator`'s own fourth fixture records: a member name
    // is dispatched only once a later token closes it, so an operator written
    // flush against it is decided at the `Member` trie's boundary policy rather
    // than at this position.
    FrozenKill {
        fixture: "t6-nonscalar-operator",
        db: "car_1",
        closer: Closer::L2("OrderedOperand"),
        kill: Kill::Walk {
            walk: "{|spider::car_1::model::default::ModelList.all().maker *'MPG'}",
            closed_by: "*",
        },
    },
    FrozenKill {
        fixture: "t6-nonscalar-operator",
        db: "car_1",
        closer: Closer::L2("OrderedOperand"),
        kill: Kill::Probe {
            prefix: "|spider::car_1::model::default::CarMakers.all().id ",
            real: "==",
            phantom: "&&",
        },
    },
    FrozenKill {
        fixture: "t6-nonscalar-operator",
        db: "car_1",
        closer: Closer::L2("OrderedOperand"),
        kill: Kill::Probe {
            prefix: "|spider::car_1::model::default::CarMakers.all().id ",
            real: "==",
            phantom: "/",
        },
    },
    FrozenKill {
        fixture: "t6-nonscalar-operator",
        db: "world_1",
        closer: Closer::L2("OrderedOperand"),
        kill: Kill::Probe {
            prefix: "|spider::world_1::model::default::Country.all()\
                     ->filter(c|$c.fk1DefaultCountrylanguage ",
            real: "->",
            phantom: "||",
        },
    },
    // The fourth shape, and the only one that reaches this position through a
    // *call*: `toOne` collapses the extent's multiplicity and hands the class
    // straight back (`lessThan(CarMakers[1],Integer[1])`), where a member name —
    // the route the other three take — is long gone by the closing `)`.
    FrozenKill {
        fixture: "t6-nonscalar-operator",
        db: "car_1",
        closer: Closer::L2("OrderedOperand"),
        kill: Kill::Probe {
            prefix: "|spider::car_1::model::default::CarMakers.all()->toOne() ",
            real: "==",
            phantom: "<",
        },
    },
    FrozenKill {
        fixture: "t3-reducer",
        db: "car_1",
        closer: Closer::L2("Reducer"),
        kill: Kill::Probe {
            prefix: "|spider::car_1::model::default::CarsData.all()->groupBy([], \
         [agg('X', row: meta::pure::tds::TDSRow[1]|$row.getString('Horsepower'), \
         y: String[*]|$y->",
            real: "min",
            phantom: "sum",
        },
    },
    FrozenKill {
        fixture: "t4-string-method",
        db: "car_1",
        closer: Closer::L2("ScalarMethod"),
        kill: Kill::Probe {
            prefix: "|spider::car_1::model::default::CarsData.all()\
         ->project([x|$x.cylinders], ['Cylinders'])\
         ->filter(row: meta::pure::tds::TDSRow[1]|$row.getInteger('Cylinders')->",
            real: "toOne",
            phantom: "toUpper",
        },
    },
    FrozenKill {
        fixture: "t4-string-method",
        db: "car_1",
        closer: Closer::L2("ScalarMethod"),
        kill: Kill::Probe {
            prefix: "|spider::car_1::model::default::CarsData.all()->filter(x|$x.cylinders->",
            real: "toOne",
            phantom: "startsWith",
        },
    },
    FrozenKill {
        fixture: "t4-string-method",
        db: "car_1",
        closer: Closer::L2("ScalarMethod"),
        kill: Kill::Probe {
            prefix: "|spider::car_1::model::default::CarsData.all()\
         ->filter(x|$x.cylinders->toOne()->",
            real: "toString",
            phantom: "toLower",
        },
    },
    FrozenKill {
        fixture: "n6-column",
        db: "battle_death",
        closer: Closer::L2("Column"),
        kill: Kill::Probe {
            prefix: "|spider::battle_death::model::default::Battle.all()\
         ->project([x|$x.name, x|$x.result], ['Name', 'Result'])\
         ->filter(r|$r.getInteger(",
            real: "'Name'",
            phantom: "'Ghost'",
        },
    },
    FrozenKill {
        fixture: "n6-relation-column",
        db: "car_1",
        closer: Closer::L2("RelationColumn"),
        kill: Kill::Probe {
            prefix: "|spider::car_1::model::default::CarsData.all()\
         ->filter(x|$x.cylinders >= 0)\
         ->project(~[Cyl: x|$x.cylinders])\
         ->groupBy(~[Cyl], ~'Total': x|$x.",
            real: "Cyl",
            phantom: "Zzz",
        },
    },
    FrozenKill {
        fixture: "n6-relation-column-fused",
        db: "car_1",
        closer: Closer::L2("RefVar"),
        kill: Kill::Probe {
            prefix: "|spider::car_1::model::default::CarsData.all()\
         ->filter(x|$x.cylinders >= 0)\
         ->project(~[Cyl: x|$x.cylinders])\
         ->groupBy(~[Cyl], ~'Total': x|$x",
            real: ".Cyl",
            phantom: ".Zzz",
        },
    },
    FrozenKill {
        fixture: "n1-project-map-binder",
        db: "car_1",
        closer: Closer::L2("Member"),
        kill: Kill::Probe {
            prefix: "|spider::car_1::model::default::CarsData.all()\
         ->filter(x|$x.cylinders >= 0)\
         ->project(~[Cyl: x|$x.",
            real: "cylinders",
            phantom: "sallary",
        },
    },
    FrozenKill {
        fixture: "n3g-receiver-only-arg",
        db: "world_1",
        closer: Closer::L2("ReceiverOnlyArg"),
        kill: Kill::Walk {
            walk: "{|spider::world_1::model::default::Countrylanguage.all()\
             ->isEmpty('_v__t0sc0')->renameColumns('Population_T3')}",
            closed_by: "'_v__t0sc0'",
        },
    },
    FrozenKill {
        fixture: "n3g-receiver-only-arg",
        db: "world_1",
        closer: Closer::L2("ReceiverOnlyArg"),
        kill: Kill::Walk {
            walk: "{|spider::world_1::model::default::Country.all()->count('Name')}",
            closed_by: "'Name'",
        },
    },
    FrozenKill {
        fixture: "n3g-receiver-only-arg",
        db: "world_1",
        closer: Closer::L2("ReceiverOnlyArg"),
        kill: Kill::Walk {
            walk: "{|spider::world_1::model::default::Country.all()->isNotEmpty('Name')}",
            closed_by: "'Name'",
        },
    },
    FrozenKill {
        fixture: "n3g-receiver-only-arg",
        db: "world_1",
        closer: Closer::L2("ReceiverOnlyArg"),
        kill: Kill::Walk {
            walk: "{|spider::world_1::model::default::Country.all()->size('Name')}",
            closed_by: "'Name'",
        },
    },
    FrozenKill {
        fixture: "n3g-receiver-only-arg",
        db: "world_1",
        closer: Closer::L2("ReceiverOnlyArg"),
        kill: Kill::Walk {
            walk: "{|spider::world_1::model::default::Country.all()->toOne('Name')}",
            closed_by: "'Name'",
        },
    },
    FrozenKill {
        fixture: "n3g-receiver-only-arg",
        db: "car_1",
        closer: Closer::L2("ReceiverOnlyArg"),
        kill: Kill::Walk {
            walk: "{|spider::car_1::model::default::CarMakers.all()->count('Maker')}",
            closed_by: "'Maker'",
        },
    },
    FrozenKill {
        fixture: "n4a-store-result",
        db: "world_1",
        closer: Closer::L2("StoreResult"),
        kill: Kill::Walk {
            walk: "{|spider::world_1::Db->tableReference('Percentage','Name')\
             >spider::world_1::model::default::Country}",
            closed_by: ">",
        },
    },
    FrozenKill {
        fixture: "n4a-store-result",
        db: "world_1",
        closer: Closer::L2("StoreResult"),
        kill: Kill::Walk {
            walk: "{|spider::world_1::Db->tableReference('HeadOfState_T1_3','english')\
             &&'CountryCode_T1_1'}",
            closed_by: "&&",
        },
    },
    FrozenKill {
        fixture: "n4a-store-result",
        db: "world_1",
        closer: Closer::L2("StoreResult"),
        kill: Kill::Walk {
            walk: "{|spider::world_1::Db->tableReference('name','Caribbean')>'CountryCode_T2'}",
            closed_by: ">",
        },
    },
    FrozenKill {
        fixture: "n4a-store-result",
        db: "world_1",
        closer: Closer::L2("StoreResult"),
        kill: Kill::Walk {
            walk: "{|spider::world_1::Db->tableReference('name','Caribbean')-'CountryCode_T2'}",
            closed_by: "'CountryCode_T2'",
        },
    },
    FrozenKill {
        fixture: "n4a-store-result",
        db: "car_1",
        closer: Closer::L2("StoreResult"),
        kill: Kill::Walk {
            walk: "{|spider::car_1::Db->tableReference('Model_t3_5','cnt')>'Edispl_T2_2'}",
            closed_by: ">",
        },
    },
    FrozenKill {
        fixture: "n4a-store-result",
        db: "car_1",
        closer: Closer::L2("StoreResult"),
        kill: Kill::Walk {
            walk: "{|spider::car_1::Db->tableReference('Weight_t1','Id_T2_3')\
             *'MAX(Accelerate)'}",
            closed_by: "*",
        },
    },
    // One row per byte of `STORE_RESULT_DENIED_OPENERS` that the walker's own
    // failures do not already pin (`&`, `>` and `*` are covered above), so no
    // byte can be dropped from the set without a red test — the standard N3g's
    // per-name rows set in this same phase. Each was sent through the live engine
    // on this branch: `||` gives `or(Table[1],Boolean[1])`, `<` gives
    // `lessThan(Table[1],String[1])`, `+` gives `plus(Any[2])` and `/` gives
    // `divide(Table[1],Integer[1])`.
    FrozenKill {
        fixture: "n4a-store-result",
        db: "world_1",
        closer: Closer::L2("StoreResult"),
        kill: Kill::Walk {
            walk: "{|spider::world_1::Db->tableReference('default','country')||true}",
            closed_by: "||",
        },
    },
    FrozenKill {
        fixture: "n4a-store-result",
        db: "world_1",
        closer: Closer::L2("StoreResult"),
        kill: Kill::Walk {
            walk: "{|spider::world_1::Db->tableReference('default','country')<'x'}",
            closed_by: "<",
        },
    },
    FrozenKill {
        fixture: "n4a-store-result",
        db: "world_1",
        closer: Closer::L2("StoreResult"),
        kill: Kill::Walk {
            walk: "{|spider::world_1::Db->tableReference('default','country')+1}",
            closed_by: "+",
        },
    },
    FrozenKill {
        fixture: "n4a-store-result",
        db: "world_1",
        closer: Closer::L2("StoreResult"),
        kill: Kill::Walk {
            walk: "{|spider::world_1::Db->tableReference('default','country')/1}",
            closed_by: "/",
        },
    },
    FrozenKill {
        fixture: "n4b-logical-operand",
        db: "world_1",
        closer: Closer::L2("LogicalOperand"),
        kill: Kill::Walk {
            walk: "{|spider::world_1::model::default::Country.all()\
             ->filter('Percentage_T4_2'=='IndepYear_T1_1'&&'GNP_T1_3')}",
            closed_by: "'GNP_T1_3'",
        },
    },
    FrozenKill {
        fixture: "n4b-logical-operand",
        db: "car_1",
        closer: Closer::L2("LogicalOperand"),
        kill: Kill::Walk {
            walk: "{|spider::car_1::model::default::CarMakers.all()\
             ->filter(x|$x.country=='usa'||1930)}",
            closed_by: "1930",
        },
    },
    // Phase 10 — the same slot, one category further out: a `LambdaFunction` is
    // no more a `Boolean` than a string literal is. The named-binder form dies on
    // the very same byte, the bare word before it being an operand this rule
    // keeps.
    FrozenKill {
        fixture: "n4b-lambda-operand",
        db: "world_1",
        closer: Closer::L2("LogicalOperand"),
        kill: Kill::Probe {
            prefix: "|spider::world_1::model::default::Country.all()\
                     ->filter(x|$x.name == 'Aruba' && ",
            real: "true",
            phantom: "|",
        },
    },
    FrozenKill {
        fixture: "n4b-lambda-operand",
        db: "car_1",
        closer: Closer::L2("LogicalOperand"),
        kill: Kill::Probe {
            prefix: "|spider::car_1::model::default::CarMakers.all()\
                     ->filter(x|$x.country == 'usa' || ",
            real: "true",
            phantom: "|",
        },
    },
    FrozenKill {
        fixture: "n4c-str-operator",
        db: "car_1",
        closer: Closer::L2("StrOperator"),
        kill: Kill::Walk {
            walk: "{|spider::car_1::model::default::ModelList.all()\
             .fk3DefaultCarNames<='Id_T2'-'Maker_t1_1'}",
            closed_by: "'Maker_t1_1'",
        },
    },
    FrozenKill {
        fixture: "n4c-str-operator",
        db: "world_1",
        closer: Closer::L2("StrOperator"),
        kill: Kill::Walk {
            walk: "{|spider::world_1::model::default::Countrylanguage.all()\
             ->isEmpty()>'LifeExpectancy'*'Continent_t1'}",
            closed_by: "*",
        },
    },
    FrozenKill {
        fixture: "n4c-str-operator",
        db: "world_1",
        closer: Closer::L2("StrOperator"),
        kill: Kill::Walk {
            walk: "{|spider::world_1::model::default::Country.all()\
             ->filter('Percentage_T4_2'=='IndepYear_T1_1'/'COUNT(DISTINCT Language)')}",
            closed_by: "/",
        },
    },
    // The **whitespace-separated** operator, which is the only route to N4c's
    // arming half: with no gap the rule is read at the byte-PDA's pending-quote
    // state, and the `awaiting_str_operator` arm at `AfterValue` is never
    // reached. Live-attested with the space in place (`times(String[2])`).
    FrozenKill {
        fixture: "n4c-str-operator",
        db: "world_1",
        closer: Closer::L2("StrOperator"),
        kill: Kill::Walk {
            walk: "{|spider::world_1::model::default::Countrylanguage.all()\
             ->isEmpty()>'LifeExpectancy' *'Continent_t1'}",
            closed_by: "*",
        },
    },
    FrozenKill {
        fixture: "oos-held-out",
        db: "world_1",
        closer: Closer::L2("SourceIdent"),
        kill: Kill::Probe {
            prefix: "|",
            real: "spider::world_1::model::default::Country",
            phantom: "spider::world_1::model::default::Nation",
        },
    },
    FrozenKill {
        fixture: "oos-held-out",
        db: "world_1",
        closer: Closer::L2("Member"),
        kill: Kill::Probe {
            prefix: "|spider::world_1::model::default::Country.all()->filter(x|$x.",
            real: "name",
            phantom: "gdp",
        },
    },
    FrozenKill {
        fixture: "oos-held-out",
        db: "dog_kennels",
        closer: Closer::L2("Member"),
        kill: Kill::Probe {
            prefix: "|spider::dog_kennels::model::default::Professionals.all()->filter(x|$x.",
            real: "lastName",
            phantom: "salary",
        },
    },
    FrozenKill {
        fixture: "oos-held-out",
        db: "student_transcripts_tracking",
        closer: Closer::L2("Member"),
        kill: Kill::Probe {
            prefix: "|spider::student_transcripts_tracking::model::default::Transcripts.all()\
         ->filter(x|$x.",
            real: "transcriptDate",
            phantom: "nonexistent",
        },
    },
];

/// A fixture family's provenance, from [`FROZEN_FAMILIES`].
fn origin_of(fixture: &str) -> &'static str {
    FROZEN_FAMILIES
        .iter()
        .find(|(tag, _)| *tag == fixture)
        .map(|(_, origin)| *origin)
        .unwrap_or_else(|| panic!("no FROZEN_FAMILIES row declares {fixture:?}"))
}

/// The frozen fixtures tagged `fixture`. Never empty: a per-rule test that
/// silently stopped covering anything is itself the regression this file is
/// built to catch.
fn frozen(fixture: &'static str) -> Vec<&'static FrozenKill> {
    let slice: Vec<&FrozenKill> = FROZEN_KILLS
        .iter()
        .filter(|kill| kill.fixture == fixture)
        .collect();
    assert!(
        !slice.is_empty(),
        "no FROZEN_KILLS row is tagged {fixture:?} — this test would pass vacuously"
    );
    slice
}

/// Replay one frozen fixture, assert it is still refused exactly as recorded,
/// and return the rule kind it is **evidence** for — `None` when the byte-PDA
/// refuses it too, since a rule that does not do the refusing is not exercised.
fn assert_frozen_kill(kill: &FrozenKill) -> Option<&'static str> {
    let (observed, l1_admits) = match &kill.kill {
        Kill::Walk { walk, closed_by } => walk_closer(kill.db, walk, closed_by),
        Kill::Probe {
            prefix,
            real,
            phantom,
        } => probe_closer(kill.db, prefix, real.as_bytes(), phantom.as_bytes()),
    };
    let rule = kill.closer.rule();
    let observed = observed.unwrap_or_else(|| {
        panic!(
            "no L2 rule is active where this fixture is refused, so {rule} is no \
             longer exercised here — {} [{}]",
            origin_of(kill.fixture),
            kill.fixture
        )
    });
    assert_eq!(
        observed,
        rule,
        "a different rule now closes this fixture — {observed} took over {rule}'s \
         recorded kill: {} [{}]",
        origin_of(kill.fixture),
        kill.fixture
    );
    // The half `active_l2_position` alone cannot answer: it names the rule that
    // *governs* the position, not the layer that cleared the bit. Checked in
    // both directions, so neither label can be worn by the other's fixture.
    assert_eq!(
        l1_admits,
        kill.closer.expects_l1_to_admit(),
        "{}: the byte-PDA {} this fixture's own token, so it is recorded under \
         the wrong Closer variant — {} [{}]",
        rule,
        if l1_admits {
            "admits (the overlay is what refuses it: this is Closer::L2)"
        } else {
            "already refuses (so it is not this rule's evidence: this is Closer::AlsoL1)"
        },
        origin_of(kill.fixture),
        kill.fixture
    );
    kill.closer.expects_l1_to_admit().then_some(rule)
}

/// [`FROZEN_KILLS`] and [`FROZEN_FAMILIES`] name exactly the same families, each
/// declared once — so a fixture can neither lose its provenance nor keep a
/// provenance row after its last fixture is gone.
#[test]
fn every_frozen_family_is_declared_exactly_once() {
    let mut declared: Vec<&str> = FROZEN_FAMILIES.iter().map(|(tag, _)| *tag).collect();
    declared.sort_unstable();
    let deduped: BTreeSet<&str> = declared.iter().copied().collect();
    assert_eq!(
        declared.len(),
        deduped.len(),
        "FROZEN_FAMILIES declares a family tag twice"
    );
    let used: BTreeSet<&str> = FROZEN_KILLS.iter().map(|kill| kill.fixture).collect();
    assert_eq!(
        deduped, used,
        "FROZEN_FAMILIES and FROZEN_KILLS disagree about which fixture families exist"
    );
}

/// Replay every fixture in one family.
fn assert_frozen(fixture: &'static str) {
    for kill in frozen(fixture) {
        assert_frozen_kill(kill);
    }
}

/// Replay `walk`'s tokens through a schema-aware session and assert the stream
/// **cannot** be produced: at some step the walk's own next token is absent from
/// `allowed_mask`, and that token is exactly `closed_by`.
///
/// Returns the rule active at that step, and whether a **schema-less** session
/// still admits the offending token there — the second half being what separates
/// "the overlay refuses this" from "the byte-PDA already did".
fn walk_closer(db_id: &str, walk: &str, closed_by: &str) -> (Option<&'static str>, bool) {
    // The vocabulary is built from this walk's own lexemes and nothing else, which
    // is what makes a fixture a *closed* experiment — but it also means the
    // vocabulary usually cannot spell the continuation a name rule leaves legal.
    // Several of these masks are therefore empty, and legitimately so: that is the
    // documented precondition of §6.7's liveness invariant (`tests/l2_liveness.rs`
    // asserts it over a vocabulary complete over the grammar's alphabet, the
    // shipping case), not a
    // violation of it. Falling open here instead would surrender exactly the
    // `NamePoint::Partial` masking these fixtures exist to freeze.
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
            let kind = session.active_l2_position().as_ref().and_then(rule_kind);
            return (kind, l1_admits_at(&grammar, &vocab, &lex(walk)[..step], id));
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

/// Assert `real` stays admissible and `phantom` is cleared after `prefix`.
///
/// Returns the rule active at that decision point, and whether a schema-less
/// session still admits `phantom` there (see [`walk_closer`]).
fn probe_closer(
    db_id: &str,
    prefix: &str,
    real: &[u8],
    phantom: &[u8],
) -> (Option<&'static str>, bool) {
    let (verdicts, kind) = probe_at(db_id, prefix, &[real, phantom]);
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
    let extras: Vec<Vec<u8>> = [real, phantom].iter().map(|p| p.to_vec()).collect();
    let vocab = TokenVocab::build(&[prefix], &extras);
    let grammar = CompiledGrammar::compile(vocab.vocab());
    let id = vocab.id_of(phantom).expect("probe token in vocab");
    (kind, l1_admits_at(&grammar, &vocab, &lex(prefix), id))
}

/// Whether the **schema-less** recognizer — L1 alone, no overlay — still admits
/// token `id` after streaming `lead`.
///
/// L1 is a strict over-approximation of L2, so `lead` is always admissible here;
/// the answer is entirely about the one token under test.
fn l1_admits_at(grammar: &CompiledGrammar, vocab: &TokenVocab, lead: &[Vec<u8>], id: u32) -> bool {
    let mut session = DecoderSession::new(grammar);
    for token in lead {
        let tid = vocab
            .id_of(token)
            .unwrap_or_else(|| panic!("token not in vocab: {:?}", bytes_str(token)));
        session
            .accept_token(tid)
            .expect("L1 admits every token L2 already admitted");
    }
    session.allowed_mask().test(id)
}

/// Rule kinds [`ALL_RULE_KINDS`] lists that no frozen fixture currently closes.
///
/// A kind missing from *both* the recorded-closer set and this list is a real
/// fixture regression — a rule that lost its last frozen evidence, exactly the
/// four-times-repeated failure of issue #55's Phases 1–4 — not documented
/// residue. Mirrors `schema_walk_rule_coverage.rs`'s `EXPECTED_UNFIRED` and
/// `schema_walk_state_coverage.rs`'s `EXPECTED_UNREACHABLE` conventions.
///
/// **Empty, and it should stay that way.** Every rule this overlay ships is
/// currently the recorded closer of at least one frozen fixture. Writing a name
/// in here is admitting a shipped rule has no frozen evidence at all, so it
/// wants the same "no evidence, no implementation" justification `EXPECTED_UNFIRED`
/// carries — not a way to make this gate quiet.
const EXPECTED_WITHOUT_A_FROZEN_KILL: &[&str] = &[];

/// The [`ALL_RULE_KINDS`] entries `recorded` does not contain, sorted — the
/// gate's own decision, factored out so
/// [`the_frozen_kill_gate_reddens_when_a_rule_loses_its_last_fixture`] can prove
/// it actually fires rather than trusting that it would.
fn rule_kinds_without_a_frozen_kill(recorded: &BTreeSet<&'static str>) -> Vec<&'static str> {
    let mut missing: Vec<&'static str> = ALL_RULE_KINDS
        .iter()
        .filter(|kind| !recorded.contains(*kind))
        .copied()
        .collect();
    missing.sort_unstable();
    missing
}

/// Every recorded closer names a real rule ([`ALL_RULE_KINDS`]), so a typo can
/// never buy a rule its coverage.
#[test]
fn every_recorded_closer_names_a_shipped_rule() {
    for kill in FROZEN_KILLS {
        assert!(
            ALL_RULE_KINDS.contains(&kill.closer.rule()),
            "FROZEN_KILLS records a closer that is not a shipped rule kind: \
             {:?} — {} [{}]",
            kill.closer.rule(),
            origin_of(kill.fixture),
            kill.fixture
        );
    }
    for kind in EXPECTED_WITHOUT_A_FROZEN_KILL {
        assert!(
            ALL_RULE_KINDS.contains(kind),
            "EXPECTED_WITHOUT_A_FROZEN_KILL names a rule kind that does not ship: {kind:?}"
        );
    }
}

/// **The gate this file exists for.** Every shipped L2 rule is the observed
/// closer of at least one frozen fixture, or is named in
/// [`EXPECTED_WITHOUT_A_FROZEN_KILL`] with a reason.
///
/// The per-fixture `closed_by`/`at` checks Phases 3 and 4 added redden when a
/// *specific* walk's outcome changes. They cannot see the failure that actually
/// recurred four times: a new rule taking over an older rule's kills until that
/// older rule has no walk-level evidence left anywhere. This does — the closers
/// are re-derived live from the shipped decoder on every run, so a takeover
/// removes the stolen-from rule from the set the moment it happens.
///
/// Only [`Closer::L2`] fixtures count. A shape the byte-PDA refuses on its own
/// is still frozen, but it is not evidence that the rule governing that position
/// fires, and letting it stand in as evidence would be the same false green this
/// gate exists to prevent.
#[test]
fn every_rule_kind_has_a_frozen_walk_that_it_closes() {
    let observed: BTreeSet<&'static str> =
        FROZEN_KILLS.iter().filter_map(assert_frozen_kill).collect();
    let mut expected = EXPECTED_WITHOUT_A_FROZEN_KILL.to_vec();
    expected.sort_unstable();
    assert_eq!(
        rule_kinds_without_a_frozen_kill(&observed),
        expected,
        "a shipped L2 rule has no frozen fixture left that closes it — either a \
         newer rule took its kills over, or its fixtures were dropped. Add a \
         frozen fixture for it, or document it in EXPECTED_WITHOUT_A_FROZEN_KILL"
    );
}

/// The gate's own counterfactual: for **every** rule the table currently covers,
/// losing its last fixture must make
/// [`every_rule_kind_has_a_frozen_walk_that_it_closes`] red. Without this the
/// gate could pass by never reporting anything.
#[test]
fn the_frozen_kill_gate_reddens_when_a_rule_loses_its_last_fixture() {
    let observed: BTreeSet<&'static str> = FROZEN_KILLS
        .iter()
        .filter_map(|kill| {
            kill.closer
                .expects_l1_to_admit()
                .then_some(kill.closer.rule())
        })
        .collect();
    assert!(
        rule_kinds_without_a_frozen_kill(&observed).len() < ALL_RULE_KINDS.len(),
        "the table covers no rule at all, so the gate below proves nothing"
    );
    for &kind in &observed {
        let mut without = observed.clone();
        without.remove(kind);
        assert!(
            rule_kinds_without_a_frozen_kill(&without).contains(&kind),
            "the gate stays green with {kind} left uncovered"
        );
    }
    // The concrete incident, replayed: Phase 3's N3c took over all twelve of
    // Phase 2's frozen N7 walks, and N7 was left proven only by the `n7-extent`
    // family Phase 3 cut to replace them. Drop that family — exactly what a
    // fifth rule stealing those kills would do — and the gate must name
    // `ValueIdent`, which is what nothing in Phases 1–4 could do automatically.
    let without_n7: BTreeSet<&'static str> = FROZEN_KILLS
        .iter()
        .filter(|kill| kill.fixture != "n7-extent")
        .filter_map(|kill| {
            kill.closer
                .expects_l1_to_admit()
                .then_some(kill.closer.rule())
        })
        .collect();
    assert!(
        rule_kinds_without_a_frozen_kill(&without_n7).contains(&"ValueIdent"),
        "N7 lost its last frozen walk and the gate did not notice"
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
    assert_frozen("n3-classpath");
}

/// Issue #55 bucket C — a `$var` reference to a name nothing in the stream ever
/// bound. Both `world_1` walks were rejected by a live Legend engine with "Can't
/// find variable class for variable '<name>' in the graph"; S2
/// (`L2Position::RefVar`) now admits only names the tracker has actually seen
/// bound, and neither walk binds anything at all.
///
/// Frozen verbatim (issue #55 Phase 1, gate (b)); see [`Kill::Walk`] on why the string, not the
/// walk index, is the fixture.
#[test]
fn s2_masks_every_unbound_refvar_walk() {
    assert_frozen("s2-refvar");
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
/// The completion counterpart of [`walk_closer`]: a walk can be
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
    assert_frozen("source-method");
}

/// Issue #55 Phase 2 — the `let`-candidate states are inside a source
/// identifier. This `world_1` walk was produced by the schema walker and
/// rejected live with "Can't find the packageable element 'l'": `l` is a strict
/// prefix of the `let` keyword N3 admits, and N3 used to go dark the moment the
/// byte-PDA entered `LetL`, so a bare `l` read as a finished source.
#[test]
fn n3_masks_a_source_that_is_only_a_prefix_of_the_let_keyword() {
    assert_frozen("n3-let-prefix");
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
/// Frozen verbatim (gate (b)); see [`Kill::Walk`] on why the string, not the
/// walk index, is the fixture.
#[test]
fn n7_masks_every_dangling_value_identifier_walk() {
    assert_frozen("n7-bare-source");
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
/// |…::Country.all()->limit(tableReference)&&5             => …element 'tableReference'
/// |…::Countrylanguage.all()->sort(code    \n!='Name_T2')  => …element 'code'
/// |…::Country.all()->col(between\n*'District_city')        => …element 'between'
/// ```
///
/// **Two payloads were re-rooted in Phase 5, and a third in Phase 6** (issue
/// #55); the `closed_by` guard is what forced each. Phase 5's two previously
/// arrived through `->pair(…)`, which N3f now denies on a class extent, closing
/// the walk at the call's `(`. Phase 6's arrived through `->count(…)`, whose
/// argument slot N3g now admits nothing but its closer — so the dangling word
/// was cleared one token *before* N7's own decision point, and the recorded
/// closer read back as `tableReference` instead of `)`. Each is one rule taking
/// over another's kill, the sixth such occurrence in the series and the second
/// the gate caught on its first run.
///
/// The carriers now in use — `max`, `filter`, `limit`, `sort`, `col` — are
/// governed by neither N3f nor N3g, so N7 is still the rule under test. Every
/// replacement was sent through the live engine on this branch and rejected on
/// the dangling word exactly as its predecessor was
/// (`->limit(tableReference)&&5` → "Can't find the packageable element
/// 'tableReference'").
#[test]
fn n7_masks_a_dangling_value_identifier_behind_a_real_extent() {
    assert_frozen("n7-extent");
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
/// Frozen verbatim (gate (b)); see [`Kill::Walk`] on why the string, not the
/// walk index, is the fixture.
#[test]
fn n3c_masks_every_arrow_off_a_bare_class_source_walk() {
    assert_frozen("n3c-class");
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
    assert_frozen("n3c-store-all");
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
    assert_frozen("n3c-store-method");
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
    assert_frozen("n3c-cost");
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

/// Issue #55 Phase 4 — N3d, the store method's own call shape. Every store-method
/// parameter is a `String[1]` and the call takes exactly two of them (the
/// engine's own signature, quoted back in its rejection:
/// `tableReference(Database[1],String[1],String[1]):Table[1]`; the 5034-query gold
/// corpus agrees, with all 8455 of its store-method calls passing exactly two
/// single-quoted strings). The argument slot therefore admits only whitespace and
/// a string's opening quote — not the call's own closer, which is the arity half
/// — and the separator that follows a completed argument admits only the `,` the
/// call still owes or, once it owes none, its `)`.
///
/// Frozen verbatim from the live lane, each with the token that closes it
/// (issue #55 Phase 3's fixture rule): a later rule stealing one of these kills
/// reddens the fixture instead of silently passing.
///
/// The rule has two anchors and therefore two families. Every walk the live lane
/// produced dies at the **separator** after a completed argument; the *arity*
/// half — an opened slot, before or after a comma, that owes its argument — had
/// no frozen fixture at all until
/// [`every_rule_kind_has_a_frozen_walk_that_it_closes`] pointed that out on its
/// first run.
#[test]
fn n3d_masks_every_store_method_call_with_the_wrong_argument_shape() {
    assert_frozen("n3d-arg-separator");
    assert_frozen("n3d-open-arg-slot");
}

/// N3d's soundness counterfactual: the two-string call the rule is built around
/// streams end to end, and so does the arm-A envelope that follows it — so the
/// rule is masking a wrong argument shape and not the position itself.
#[test]
fn n3d_still_admits_the_two_string_store_method_call() {
    assert_streams_soundly_under_l2(
        "world_1",
        "|spider::world_1::Db->tableReference('default','country')->tableToTDS()->limit(5)",
    );
}

/// Issue #55 Phase 4 — N3e, the class-extent continuation. `Class.all()` produces
/// a `T[*]` extent, so every binary operator the vocabulary offers mismatches it
/// by construction (live: "Can't find a match for function
/// 'and(ModelList[*],String[1])'"). The corpus split behind the rule is cited
/// once, in `docs/spec/schema.md` §6.6.
#[test]
fn n3e_masks_every_operator_applied_to_a_class_extent() {
    assert_frozen("n3e-extent-operator");
}

/// N3e's soundness counterfactual: the three continuations the corpus does
/// exercise all still stream — the step arrow, a property navigation over the
/// extent, and (for the arrow) the whitespace that may precede it.
#[test]
fn n3e_still_admits_the_step_arrow_and_the_extent_property_dot() {
    assert_streams_soundly_under_l2(
        "world_1",
        "|spider::world_1::model::default::Country.all() ->filter(x|$x.name == 'Aruba')",
    );
    assert_streams_soundly_under_l2(
        "world_1",
        "|spider::world_1::model::default::Country.all().name",
    );
}

/// Issue #296 — N3e's third continuation, `nothing at all`, spelled the three
/// ways the grammar spells the *end of a term*. `pipeline = source , { "->" step }`
/// (`docs/spec/grammar.md` §5.2) permits **zero** steps, so a closed `Class.all()`
/// is already a whole pipeline; where that pipeline sits inside a frame, the
/// token that ends it is the frame's own closer or separator, not end-of-stream.
///
/// The top-level `|Class.all()` form ended correctly all along (the EOS bit), so
/// masking `}`, `;` and `)` made the recognizer disagree with itself about the
/// same construct depending only on what enclosed it.
#[test]
fn n3e_admits_the_zero_step_pipeline_its_grammar_defines() {
    // §5.1 `blockQuery = "{|" { letBinding ";" } pipeline "}"` — the block's
    // closer ends a zero-step pipeline.
    assert_streams_soundly_under_l2(
        "world_1",
        "{|spider::world_1::model::default::Country.all()}",
    );
    // §5.1 `letBinding = "let" ident "=" pipeline` — the `;` that ends a binding
    // whose pipeline has zero steps.
    assert_streams_soundly_under_l2(
        "world_1",
        "{|let c = spider::world_1::model::default::Country.all(); $c}",
    );
    // A binding and a final zero-step pipeline, the latter separated from the
    // block's closer by the whitespace N3e's arming must survive to reach it.
    assert_streams_soundly_under_l2(
        "world_1",
        "{|let c = spider::world_1::model::default::Country.all(); \
         spider::world_1::model::default::Countrylanguage.all() }",
    );
}

/// Issue #55 Phase 5 — N3f, the extent's **receiver category**. N3e admits the
/// step arrow off a closed `Class.all()`; this decides what that arrow may open.
///
/// The first five walks are the criterion database's own live bucket-D failures
/// (whitespace normalised — the kill lands on the call's `(`, which no
/// whitespace run can move; each normalised string was re-sent through the
/// engine on this branch and rejected identically to the walker's original):
///
/// ```text
/// {|…::Countrylanguage.all()->pair('LifeExpectancy_T3_1')!=…}  pair(Countrylanguage[*],String[1])
/// {|…::Countrylanguage.all()->average('LocalName_T3_1')||…}    average(Countrylanguage[*],String[1])
/// {|…::Countrylanguage.all()->agg(.1950)}                      agg(Countrylanguage[*],Float[1])
/// {|…::Country.all()->join('_c1')}                             join(Country[*],String[1])
/// {|…::CarMakers.all()->between(max('American Motor…'))*…}     between(CarMakers[*],String[1])
/// {|…::CarMakers.all()->join('car_names')}                     join(CarMakers[*],String[1])
/// ```
///
/// The rest carry one walk per remaining [`EXTENT_INCOMPATIBLE_METHODS`] entry,
/// so no name in the set can be dropped without a red test. Each was rejected by
/// the live engine on this branch with the whole candidate overload set printed
/// back, and in no candidate does the receiver parameter admit a `T[*]` class
/// extent — `renameColumns(TabularDataSet[1],Pair<String, String>[*])`,
/// `restrict(TabularDataSet[1],String[*])`,
/// `tableReference(Database[1],String[1],String[1])`, `tableToTDS(Table[1])`,
/// `endsWith(String[…],String[1])`, `in(Any[1]|Any[0..1],Any[*])`,
/// `parseFloat(String[1])`, `startsWith(String[…],String[1])`,
/// `substring(String[1],…)`, `sum(Float|Integer|Number[*])`,
/// `toLower(String[1])`, `toString(Any[1])`, `year(Date[1]|Date[0..1])`.
#[test]
fn n3f_masks_every_extent_method_whose_receiver_category_a_class_extent_cannot_be() {
    assert_frozen("n3f-extent-method");
}

/// N3f's soundness counterfactuals, and the reason the rule is a *deny* set.
///
/// The first block is the permissiveness evidence itself: `take`, `contains` and
/// `init` all compile on a class extent live on this branch (`init` appears in no
/// corpus at all, alongside `at`, `drop`, `slice`, `add`, `tail`, `first`,
/// `last`, `removeDuplicates`, `reverse` and `fold`), so an allow-list built from
/// corpus method names would have masked eleven legal builtins. They must stream.
///
/// `init` is also the prefix trap: `in` **is** denied and is a strict byte prefix
/// of it. The rule clears a denied name at the token that *closes* its lexeme, so
/// `in` stays walkable as a live prefix — the same discipline N3c's close policy
/// needs for `Country` ⊂ `Countrylanguage`.
///
/// The last block is the scope counterfactual: the denial belongs to the *class
/// extent*, so the identical name reached anywhere else still streams — a store
/// path's own `->tableReference` (N3c's permit set) and a `restrict` applied to a
/// real TDS, which is the receiver its every overload actually asks for.
#[test]
fn n3f_still_admits_what_a_class_extent_really_accepts() {
    for query in [
        "|spider::world_1::model::default::Country.all()->take(1)",
        "|spider::world_1::model::default::Country.all()->contains('x')",
        "|spider::world_1::model::default::Country.all()->init()",
        "|spider::world_1::model::default::Country.all()->count()",
        "|spider::world_1::model::default::Country.all()->filter(x|$x.name == 'Aruba')",
    ] {
        assert_streams_soundly_under_l2("world_1", query);
    }
    assert_streams_soundly_under_l2(
        "world_1",
        "|spider::world_1::Db->tableReference('default','country')->tableToTDS()\
         ->restrict('name')",
    );
}

/// Issue #55 Phase 6 — N3g, the **arity** half of bucket D. N3f decides which
/// names a class extent's arrow may open; this decides how long the argument list
/// of one of them may be.
///
/// The first walk is the criterion database's own live bucket-D arity failure
/// (whitespace normalised — the kill lands on the argument literal, which no
/// whitespace run can move; the normalised string was re-sent through the engine
/// on this branch and rejected identically to the walker's original). The rest
/// carry one walk per [`RECEIVER_ONLY_METHODS`] entry plus a generalization-arm
/// row, so no name in the set can be dropped without a red test:
///
/// ```text
/// {|…::Countrylanguage.all()->isEmpty('_v__t0sc0')->…}  isEmpty(Countrylanguage[*],String[1])
/// {|…::Country.all()->count('Name')}                    count(Country[*],String[1])
/// {|…::Country.all()->isNotEmpty('Name')}               isNotEmpty(Country[*],String[1])
/// {|…::Country.all()->size('Name')}                     size(Country[*],String[1])
/// {|…::Country.all()->toOne('Name')}                    toOne(Country[*],String[1])
/// {|…::CarMakers.all()->count('Maker')}                 count(CarMakers[*],String[1])
/// ```
///
/// Each was rejected by the live engine on this branch with the whole candidate
/// overload set printed back, and every candidate has arity one — the receiver.
#[test]
fn n3g_masks_an_argument_to_a_receiver_only_arrow_call() {
    assert_frozen("n3g-receiver-only-arg");
}

/// N3g's soundness counterfactuals, in both of the directions the rule could go
/// wrong in.
///
/// **The niladic call itself must stream** — the rule clears the slot's openers,
/// not its closer, so `->isEmpty()` is exactly as walkable as it was. Without
/// this the rule could pass its fixtures by masking the whole call.
///
/// **The plain-function form must stream**, which is why the rule is stated of
/// the *arrow* call alone. `count(Any[*])` spends its one parameter on the
/// receiver either way, so in `|isEmpty($x.name)` the argument *is* that
/// parameter — live `OK(Boolean)`, against `->isEmpty('x')`'s rejection. A rule
/// keyed on the name without the call shape would mask a legal query.
///
/// **A name outside the set keeps its arguments**: `filter`, `sort` and `take`
/// all take one, and `sort` is the near miss the set deliberately excludes
/// (1048 corpus calls, every one with a comparator argument).
#[test]
fn n3g_still_admits_the_niladic_call_and_the_plain_function_form() {
    for query in [
        "|spider::world_1::model::default::Country.all()->isEmpty()",
        "|spider::world_1::model::default::Country.all()->count()",
        "|spider::world_1::model::default::Country.all()->filter(x|isEmpty($x.name))",
        "|spider::world_1::model::default::Country.all()->filter(x|$x.name->isEmpty())",
        "|spider::world_1::model::default::Country.all()->take(1)",
        "|spider::world_1::model::default::Country.all()->filter(x|$x.name == 'Aruba')",
    ] {
        assert_streams_soundly_under_l2("world_1", query);
    }
}

/// Issue #55 Phase 6 — N4a, the store arm's dual of N3e. N3e stops an operator
/// being applied to a `Class.all()` extent; this stops one being applied to the
/// `Table[1]` a store method's call returns.
///
/// The criterion and generalization databases' own live bucket-E failures
/// (whitespace normalised; each normalised string re-sent through the engine on
/// this branch and rejected identically), plus the split-arrow probe:
///
/// ```text
/// {|…::Db->tableReference('Percentage','Name')>…::Country}   greaterThan(Table[1],Class<Country>[1])
/// {|…::Db->tableReference('HeadOfState_T1_3','english')&&…}  and(Table[1],String[1])
/// {|…::Db->tableReference('name','Caribbean')>'CountryCode…} greaterThan(Table[1],String[1])
/// {|…::Db->tableReference('name','Caribbean')-'CountryCode…} minus(Any[2])
/// {|…::Db->tableReference('Model_t3_5','cnt')>'Edispl_T2_2'} greaterThan(Table[1],String[1])
/// {|…::Db->tableReference('Weight_t1','Id_T2_3')*'MAX(Acc…'} times(Any[2])
/// ```
///
/// The fourth is the reassembly guard, and it is the one that lands on a token
/// other than the operator: the `-` streams, because it may still become the
/// `->` of `->tableToTDS()`, and the walk dies on the operand behind it.
///
/// The last four carry one walk per [`STORE_RESULT_DENIED_OPENERS`] byte the
/// walker's own failures leave unpinned (`|`, `<`, `+`, `/`), so — exactly as
/// N3g's per-name rows do for its set — no byte can be dropped from the deny set
/// without a red test. Each was live-attested on this branch alongside the rest.
#[test]
fn n4a_masks_every_operator_applied_to_a_store_methods_table_result() {
    assert_frozen("n4a-store-result");
}

/// N4a's soundness counterfactuals, and the reason the rule is subtractive
/// rather than the permit set N3e gets.
///
/// A bare `|…::Db->tableReference('T','S')` compiles live and returns `Table`, so
/// the *closers* stay; `equal(Any[1],Any[1])` is a real overload, so `==`/`!=`
/// stay; the corpus's own 8455 store calls all continue `->tableToTDS()`, so the
/// step arrow stays; and `.name` resolves on the metamodel `Table`, so the
/// navigation dot stays. Every one of these was sent through the running engine
/// on this branch before the deny set was written down. Without them the rule
/// could pass its fixtures by clearing everything after a store call.
#[test]
fn n4a_still_admits_what_a_table_result_really_accepts() {
    for query in [
        "|spider::world_1::Db->tableReference('default','country')",
        "|spider::world_1::Db->tableReference('default','country')->tableToTDS()",
        "|spider::world_1::Db->tableReference('default','country') == 'x'",
        "|spider::world_1::Db->tableReference('default','country') != 'x'",
    ] {
        assert_streams_soundly_under_l2("world_1", query);
    }
}

/// Issue #55 Phase 6 — N4b, the logical operator's operand. `and`/`or` have
/// Boolean-only overloads, so a string, numeric or date literal in the slot one
/// opens can never match:
///
/// ```text
/// {|…::Country.all()->filter('Percentage_T4_2'=='IndepYear_T1_1'&&'GNP_T1_3')}
///     => and(Boolean[1],String[1])
/// {|…::CarMakers.all()->filter(x|$x.country=='usa'||1930)}
///     => or(Boolean[1],Integer[1])
/// ```
#[test]
fn n4b_masks_a_literal_operand_of_a_logical_operator() {
    assert_frozen("n4b-logical-operand");
}

/// Issue #55 Phase 10 extends N4b one category out from the literal: a `|` in
/// the same slot opens a lambda, and a `LambdaFunction` is no more a `Boolean`
/// than a string is.
///
/// ```text
/// {|…::Db->tableReference('english','GNP_t1')!='Population_T3'|||…::Db}
///     => or(Boolean[1],LambdaFunction<{->Database[1]}>[1])
/// {|true&&|true}  => and(Boolean[1],LambdaFunction<{->Boolean[1]}>[1])
/// {|true&&x|true} => Can't find variable class for variable 'x' in the graph
/// ```
///
/// The named-binder form dies on the very same byte: `x` is a bare word this
/// rule keeps, and the pipe that would make it a binder arrives at the same
/// position under the same stamped rule.
#[test]
fn n4b_masks_a_lambda_operand_of_a_logical_operator() {
    assert_frozen("n4b-lambda-operand");
}

/// N4b's soundness counterfactual: the rule masks *literals* of a mismatched
/// kind and nothing else, so every shape that can actually be Boolean stays —
/// a nested comparison, a `$var` navExpr, and a `true`/`false` keyword, which
/// [`keeps_operand`]'s predicate keeps because they are identifiers rather than
/// literals. `('a'=='b')&&(1<2)` and `true&&true` both compile live.
#[test]
fn n4b_still_admits_every_operand_that_can_be_boolean() {
    for query in [
        "|spider::world_1::model::default::Country.all()\
         ->filter(x|$x.name == 'Aruba' && $x.code == 'ABW')",
        "|spider::world_1::model::default::Country.all()\
         ->filter(x|$x.name == 'Aruba' && true)",
        "|spider::world_1::model::default::Country.all()\
         ->filter(x|$x.name == 'Aruba' || ($x.population > 1))",
    ] {
        assert_streams_soundly_under_l2("world_1", query);
    }
}

/// Issue #55 Phase 6 — N4c, the logical operand's mirror image: the operator
/// half, read from the completed literal on its left. `minus`, `times` and
/// `divide` have no `String` overload, so none of them can take a string literal
/// as a left operand:
///
/// ```text
/// {|…::ModelList.all().fk3DefaultCarNames<='Id_T2'-'Maker_t1_1'}      minus(String[2])
/// {|…::Countrylanguage.all()->isEmpty()>'LifeExpectancy'*'Continent…'} times(String[2])
/// {|…::Country.all()->filter('Percentage_T4_2'=='IndepYear…'/'COUNT…')} divide(String[1],String[1])
/// ```
///
/// The first is the reassembly guard again, and lands on the operand rather than
/// the operator: a string literal is arrowed 32309 times across the three
/// corpora, so the `-` must stream as a possible `->` and die on what follows.
///
/// The fourth is the rule's **arming** half, and it needs the space to exist at
/// all. A string literal is dispatched only once a later token closes it, so an
/// operator written flush against the closing quote is decided at the byte-PDA's
/// pending-quote state, inside `position`; only a token that *closes* the literal
/// first — whitespace here — reaches the `awaiting_str_operator` arm at
/// `AfterValue`. Without this row that arm has no fixture, and replacing its
/// guard with `false` is a mutant every other N4c walk survives.
#[test]
fn n4c_masks_arithmetic_whose_left_operand_is_a_string_literal() {
    assert_frozen("n4c-str-operator");
}

/// N4c's soundness counterfactual, and the reason its deny set is only two bytes
/// wide.
///
/// `+` is string concatenation (`plus(String[*])`, live `OK(String)`). The
/// ordered comparators have a real `greaterThan(String[1],String[1])` overload,
/// which is why they survive **this** rule — T2 independently narrows them where
/// the left operand is a *resolved navExpr* rather than a literal, and N4c
/// neither extends nor relaxes that.
/// `&&`/`||` follow a string literal all through the gold corpus while taking the
/// enclosing *comparison* as their operand — a comparison binds tighter than a
/// conjunction — which is why the canonical `filter(x|$x.a == 'p' && $x.b == 'q')`
/// must and does stream. And the step arrow off a literal is the corpus's own
/// `'FacID'->pair('FacID_T1')` shape.
#[test]
fn n4c_still_admits_concatenation_comparison_and_the_step_arrow() {
    for query in [
        "|spider::world_1::model::default::Country.all()\
         ->filter(x|$x.name == 'Aru' + 'ba')",
        "|spider::world_1::model::default::Country.all()\
         ->filter(x|'Aru' > 'ba')",
        "|spider::world_1::model::default::Country.all()\
         ->filter(x|$x.name == 'Aruba' && $x.code == 'ABW')",
        "|spider::world_1::model::default::Country.all()\
         ->filter(x|$x.name == 'Aruba'->toUpper())",
    ] {
        assert_streams_soundly_under_l2("world_1", query);
    }
}

/// N3f's completion half: a stream may not *end* on a denied whole name either.
///
/// The mask clears the token that closes a denied name; EOS is that same closure
/// by another route, so the overlay clears the EOS bit at a deny-trie terminal
/// and leaves it alone everywhere else. This is also the half of the rule that
/// is pinned on a closer *other* than a call's `(`, and it covers both receiver
/// categories: `sum`/`pair` are primitive-scalar entries, `restrict` a
/// relation one.
///
/// L1 admits end-of-stream after any bare arrow-method name (the byte-PDA has no
/// mandatory-call rule for one), and the engine rejects every such stream — at
/// the *parser*, live: `|…::Country.all()->restrict` → "no viable alternative at
/// input '->restrict}'". So the counterfactual below asserts only what it can:
/// a name this rule does not deny keeps whatever completion L1 gives it, which
/// is what makes the three denials above the rule's own doing and not L1's.
#[test]
fn n3f_forbids_a_stream_ending_on_a_denied_extent_method_name() {
    let extent = "|spider::world_1::model::default::Country.all()";
    assert!(
        walk_may_end("world_1", &format!("{extent}->su")),
        "a strict prefix of a denied name is an open lexeme, not a denial"
    );
    for denied in ["sum", "pair", "restrict"] {
        assert!(
            !walk_may_end("world_1", &format!("{extent}->{denied}")),
            "a stream may not end on the denied name {denied:?}"
        );
    }
    assert!(
        walk_may_end("world_1", &format!("{extent}->count")),
        "a name the rule does not deny keeps whatever completion L1 gives it"
    );
}

/// N3i's completion half, the twin of
/// [`n3f_forbids_a_stream_ending_on_a_denied_extent_method_name`] one receiver
/// category over: a stream may not *end* on a denied whole name at a scalar
/// receiver either.
///
/// The same argument and the same mechanism — EOS is a name's closure by another
/// route, so `admits_eos` clears it at a `SCALAR_DENY` terminal — and it needs
/// its own fixture because the two rules read *different* tries: the extent's
/// carries `sum`, `pair` and `agg`, this one must not, since all three compile on
/// a scalar receiver.
///
/// Only the two **receiver-only-call** routes appear here, and their absence
/// elsewhere is a fact about the grammar rather than an omission: the other two
/// receivers a `ScalarMethod` arms on — a string literal and a property
/// navigation — are reachable only *inside* a call or a lambda, so an
/// L1-accepting stream that reaches one still owes a `)` and can never end at
/// this position at all. The two counterfactuals are what make the denial this
/// rule's doing and not L1's: a strict prefix is an open lexeme, and a name this
/// rule does not deny keeps whatever completion L1 gives it.
#[test]
fn n3i_forbids_a_stream_ending_on_a_denied_scalar_method_name() {
    const EXTENT: &str = "|spider::car_1::model::default::CarMakers.all()";
    for receiver in [
        format!("{EXTENT}->isNotEmpty()"),
        format!("{EXTENT}->count()"),
    ] {
        assert!(
            walk_may_end("car_1", &format!("{receiver}->res")),
            "a strict prefix of a denied name is an open lexeme, not a denial"
        );
        for denied in ["restrict", "tableToTDS", "renameColumns"] {
            assert!(
                !walk_may_end("car_1", &format!("{receiver}->{denied}")),
                "a stream may not end on the denied name {denied:?} after {receiver}"
            );
        }
        for kept in ["count", "sum", "pair", "agg"] {
            assert!(
                walk_may_end("car_1", &format!("{receiver}->{kept}")),
                "{kept:?} is legal on a scalar receiver and must keep whatever \
                 completion L1 gives it after {receiver}"
            );
        }
    }
}

/// N3f under a vocabulary that splits the step connector. Phase 4 found N3c had
/// never fired in the live lane at all, because classification read a token's own
/// bytes and a `-`/`>` split meant no token's bytes were ever `->`. N3f arms on
/// the same arrow event, so it inherits that hazard and has to pin it the same
/// way — with an explicit token run, since a lexed walk can never reproduce the
/// split.
#[test]
fn n3f_holds_when_the_step_arrow_is_split_across_tokens() {
    const EXTENT: &[&str] = &[
        "|",
        "spider::world_1::model::default::Country",
        ".",
        "all",
        "(",
        ")",
    ];
    fn run<'a>(tail: &[&'a str]) -> Vec<&'a str> {
        EXTENT.iter().chain(tail).copied().collect()
    }
    // Whole arrow and split arrow alike close the denied name at its call `(`…
    assert_token_run_is_masked("world_1", &run(&["->", "pair", "("]));
    assert_token_run_is_masked("world_1", &run(&["-", ">", "pair", "("]));
    // …including when BPE packs the name's tail and the `(` into one token.
    assert_token_run_is_masked("world_1", &run(&["-", ">", "pa", "ir("]));
    // …and a name that merely *starts* with a denied one still streams.
    assert_token_run_streams("world_1", &run(&["-", ">", "in", "it", "("]));
    assert_token_run_streams("world_1", &run(&["->", "count", "("]));
}

/// Issue #55 Phase 4 — N1 over the extent dot. A `.` straight off `Class.all()`
/// navigates the extent's own class, and Pure spells a member either bare or
/// quoted; both name the same set, so a phantom in either spelling is cleared.
/// Before this the position had no `dot_base` at all and was wholly unnarrowed.
#[test]
fn n1_masks_a_phantom_member_after_the_extent_dot_in_either_spelling() {
    assert_frozen("n1-extent-dot");
}

/// N1's counterfactual at the same position: a *real* member streams in both
/// spellings, so the rule is narrowing the name set and not the position.
#[test]
fn n1_still_admits_a_real_member_after_the_extent_dot() {
    assert_streams_soundly_under_l2(
        "world_1",
        "|spider::world_1::model::default::Country.all().name",
    );
    assert_streams_soundly_under_l2(
        "world_1",
        "|spider::world_1::model::default::Country.all().'name'",
    );
}

/// Drive an explicit token run through a schema-aware session for `db_id`,
/// asserting every token but the last is admitted and the last one is masked.
///
/// Unlike [`Kill::Walk`], which is lexed and so always offers a
/// whole `->`, this pins behaviour under a vocabulary that splits the step
/// connector into `-` and `>` — the split that let a store path be arrowed into
/// an arbitrary method past N3c (live:
/// `{|spider::car_1::Db->min('default'…)}` → "Can't find a match for function
/// 'min(Database[1],…)'"), because no token's bytes were ever `->` and the scope
/// machine's arrow event never fired.
fn assert_token_run_is_masked(db_id: &str, tokens: &[&str]) {
    let owned: Vec<Vec<u8>> = tokens.iter().map(|t| t.as_bytes().to_vec()).collect();
    let vocab = TokenVocab::build(&[], &owned);
    let grammar = CompiledGrammar::compile(vocab.vocab());
    let schema = load_schema(db_id);
    let mut session =
        DecoderSession::with_schema(&grammar, schema).expect("grammar is fixed-engine");
    let (last, lead) = tokens.split_last().expect("a non-empty token run");
    for (step, token) in lead.iter().enumerate() {
        let id = vocab.id_of(token.as_bytes()).expect("token in vocab");
        assert!(
            session.allowed_mask().test(id),
            "the run was closed at step {step} ({token:?}), before the token under test"
        );
        session
            .accept_token(id)
            .unwrap_or_else(|err| panic!("L1 rejected an L2-admitted token {token:?}: {err}"));
    }
    let id = vocab.id_of(last.as_bytes()).expect("token in vocab");
    assert!(
        !session.allowed_mask().test(id),
        "PRECISION GAP: {last:?} is still admitted after {lead:?}"
    );
}

/// Issue #55 Phase 4 — N3c holds when the step connector arrives split. The scope
/// machine classifies a token from the automaton state it opened at, so the lone
/// `>` that lands on a just-consumed `-` is the step arrow it completes, and the
/// store-method narrowing that arrow arms fires exactly as it does for a whole
/// `->`.
#[test]
fn n3c_holds_when_the_step_arrow_arrives_as_two_tokens() {
    for method in ["min", "isEmpty", "count", "restrict", "tableToTDS"] {
        assert_token_run_is_masked("car_1", &["{", "|", "spider::car_1::Db", "-", ">", method]);
    }
    // The counterfactual: the one store method there *is* still streams through
    // the same split arrow, so the rule is narrowing the name and not the arrow.
    let tokens = [
        "{",
        "|",
        "spider::car_1::Db",
        "-",
        ">",
        "tableReference",
        "(",
    ];
    let owned: Vec<Vec<u8>> = tokens.iter().map(|t| t.as_bytes().to_vec()).collect();
    let vocab = TokenVocab::build(&[], &owned);
    let grammar = CompiledGrammar::compile(vocab.vocab());
    let mut session = DecoderSession::with_schema(&grammar, load_schema("car_1"))
        .expect("grammar is fixed-engine");
    for token in tokens {
        let id = vocab.id_of(token.as_bytes()).expect("token in vocab");
        assert!(
            session.allowed_mask().test(id),
            "the store method's own name must stay admitted through a split arrow ({token:?})"
        );
        session.accept_token(id).expect("L1 admits the token");
    }
}

/// Drive an explicit token run through a schema-aware session for `db_id`,
/// asserting every token is admitted and accepted — the positive twin of
/// [`assert_token_run_is_masked`], for the shapes a split vocabulary must keep.
fn assert_token_run_streams(db_id: &str, tokens: &[&str]) {
    let owned: Vec<Vec<u8>> = tokens.iter().map(|t| t.as_bytes().to_vec()).collect();
    let vocab = TokenVocab::build(&[], &owned);
    let grammar = CompiledGrammar::compile(vocab.vocab());
    let schema = load_schema(db_id);
    let mut session =
        DecoderSession::with_schema(&grammar, schema).expect("grammar is fixed-engine");
    for (step, token) in tokens.iter().enumerate() {
        let id = vocab.id_of(token.as_bytes()).expect("token in vocab");
        assert!(
            session.allowed_mask().test(id),
            "L2 masked a legal token at step {step} ({token:?}) in {tokens:?}"
        );
        session
            .accept_token(id)
            .unwrap_or_else(|err| panic!("L1 rejected {token:?} at step {step}: {err}"));
    }
}

/// N3e admits the step arrow the *one* way it is an arrow, whether the
/// vocabulary offers it whole or split — and nothing else that opens with a `-`.
///
/// The split half is what makes the rule hold under byte-level BPE: a lone `-`
/// stays admissible (a vocabulary may only be able to spell the connector that
/// way), and the very next token is then narrowed to the `>` that completes it,
/// so an arithmetic minus cannot be reassembled a byte at a time — live-attested,
/// `{|…::Countrylanguage.all() -'HeadOfState_T1_3'}` is rejected with "Collection
/// element must have a multiplicity [1]".
#[test]
fn n3e_admits_the_step_arrow_split_or_whole_and_no_other_dash() {
    const EXTENT: &[&str] = &[
        "|",
        "spider::world_1::model::default::Country",
        ".",
        "all",
        "(",
        ")",
    ];
    fn run<'a>(tail: &[&'a str]) -> Vec<&'a str> {
        EXTENT.iter().chain(tail).copied().collect()
    }
    // Whole and split, both admitted through to the step's own method name.
    assert_token_run_streams("world_1", &run(&["->", "filter"]));
    assert_token_run_streams("world_1", &run(&["-", ">", "filter"]));
    // The `-` is committed once emitted: only the `>` may follow it.
    assert_token_run_is_masked("world_1", &run(&["-", "'HeadOfState_T1_3'"]));
    assert_token_run_is_masked("world_1", &run(&["-", "3"]));
    // …and a longer `-`-led token that is not the arrow never opens at all.
    assert_token_run_is_masked("world_1", &run(&["-'HeadOfState_T1_3'"]));
    assert_token_run_is_masked("world_1", &run(&["-3"]));
    // The scope counterfactual: the narrowing belongs to the *extent*, so an
    // arithmetic minus reached anywhere else — a dash mid-predicate, where N7
    // governs — still streams. Without this the `-` half would silently apply to
    // every dash in the stream.
    assert_streams_soundly_under_l2(
        "car_1",
        "|spider::car_1::model::default::CarsData.all()->filter(x|$x.horsepower - 3 > 0)",
    );
}
