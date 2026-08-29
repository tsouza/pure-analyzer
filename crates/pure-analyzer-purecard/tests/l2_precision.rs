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
    FrozenKill {
        fixture: "s2-refvar",
        db: "world_1",
        closer: Closer::L2("RefVar"),
        kill: Kill::Walk {
            walk: "{|\n        $code\n      /'IsOfficial_t2'}",
            closed_by: "code",
        },
    },
    FrozenKill {
        fixture: "s2-refvar",
        db: "world_1",
        closer: Closer::L2("RefVar"),
        kill: Kill::Walk {
            walk: "{\n|      $name}",
            closed_by: "name",
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
            walk: "|spider::world_1::model::default::Country.all()->count(tableReference)&&5",
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
/// |…::Country.all()->count(tableReference)&&5             => …element 'tableReference'
/// |…::Countrylanguage.all()->sort(code    !='Name_T2')    => …element 'code'
/// |…::Country.all()->col(between\n*'District_city')        => …element 'between'
/// ```
///
/// **Two payloads were re-rooted again in Phase 5** (issue #55), and the
/// `closed_by` guard is what forced it: both previously arrived through
/// `->pair(…)`, and N3f now denies `pair` on a class extent, closing the walk at
/// the call's `(` — one rule taking over another's kill for the fifth time in
/// the series. They now arrive through `count` and `sort`, which N3f leaves
/// alone, so N7 is still the rule under test. Both replacements were sent
/// through the live engine on this branch and rejected on the dangling word
/// exactly as their predecessors were.
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
/// 'and(ModelList[*],String[1])'"). Across the 5034 gold queries a closed
/// `.all()` is followed by `->` 438 times, by a `.` property 37 times and by
/// end-of-query 25 times — and by nothing else.
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

/// N3f's completion half: a stream may not *end* on a denied whole name either.
///
/// The mask clears the token that closes a denied name; EOS is that same closure
/// by another route, so the overlay clears the EOS bit at a deny-trie terminal
/// and leaves it alone everywhere else. The four cases are the whole mechanism —
/// a strict prefix of a denied name is still an open lexeme and may yet grow into
/// something legal, a whole denied name may not end a query, and a name the rule
/// does not deny keeps whatever completion L1 gives it.
#[test]
fn n3f_forbids_a_stream_ending_on_a_denied_extent_method_name() {
    let extent = "|spider::world_1::model::default::Country.all()";
    assert!(
        walk_may_end("world_1", &format!("{extent}->su")),
        "a strict prefix of a denied name is an open lexeme, not a denial"
    );
    assert!(!walk_may_end("world_1", &format!("{extent}->sum")));
    assert!(!walk_may_end("world_1", &format!("{extent}->pair")));
    assert!(
        walk_may_end("world_1", &format!("{extent}->count")),
        "a name the rule does not deny keeps whatever completion L1 gives it"
    );
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
