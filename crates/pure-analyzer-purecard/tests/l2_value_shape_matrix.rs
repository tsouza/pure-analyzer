//! The **value-shape matrix**: a systematic, per-rule sweep proving that every
//! narrowing rule which admits an open-ended literal class (a milestoning date,
//! a `$`-bound variable, a string, a number, a member identifier) admits *every*
//! grammar-legal shape of that class, byte-by-byte, not only the 2-3 witnesses
//! its own introducing PR happened to pick (`docs/spec/schema.md` §6.7, third
//! invariant).
//!
//! **The gap this closes.** `L2 ⊆ L1` (G4, `docs/spec/schema.md` §6.5) says the
//! overlay may never *widen* L1; the completed-term half of §6.7 says a rule
//! that permits a class by its *opening* byte may not clear every way of
//! *ending* the term it opened. Neither statement covers issue #391's shape: a
//! rule that permits a lexeme class (`Lexeme::Date`) by classifying a
//! **candidate token's whole bytes** is only correct when it is re-armed once
//! per fresh value slot. `L2Position::SourceMethodArg`'s arity narrowing
//! (issue #384/#387) re-arms on every byte a milestoned `all()` call's date
//! argument occupies, so a single-byte vocabulary (the adversarial,
//! byte-granular case every real BPE tokenizer's alphabet coverage implies —
//! `docs/spec/schema.md` §6.7) re-classifies the *second* digit of
//! `%2026-01-15` as a bare `Lexeme::Number` candidate rather than a date-literal
//! continuation, and masks it. `%latest` and a short `$var` never hit this: both
//! are two bytes past their opener before the byte-PDA even reaches a
//! re-armable state. The rule's own unit test
//! (`fill_source_method_arg`'s `source_method_arg_keeps_the_closer_and_milestone_dates_but_masks_a_phantom_argument`)
//! built its date witness as a *single whole-token* candidate, so it could not
//! see this either — a single-shot `narrow_into` call is definitionally immune
//! to a bug that only exists in re-arming across steps of one open value.
//!
//! **What this lane does differently.** For every L2 position that admits an
//! open-ended literal class, it drives a real, multi-step [`DecoderSession`]
//! through a *curated, comprehensive* matrix of that class's shapes
//! (`tests/support/value_shapes.rs`, built from `docs/spec/grammar.md` §5.4's
//! own `literal`/`dateLit`/`strlit`/`number`/`refVar`/`ident` productions —
//! every digit-count, separator, quoting and length shape those productions
//! admit, not only the shapes a human happened to think of) over the
//! single-byte vocabulary `tests/l2_liveness.rs` already established as the
//! adversarial case, and asserts — via the shared
//! [`byte_walk::drive`](byte_walk::drive) primitive — that every byte of every
//! witness stays L2-admissible from the position's own opener through to a
//! syntactically complete query. `drive` cross-checks each witness against a
//! schema-free L1 session in lockstep, so a witness this suite gets wrong (not
//! actually grammar-legal) fails loudly as an L1 rejection rather than
//! silently passing as a false-negative L2 finding.
//!
//! **Scope.** Applied to every L2 position whose narrowing rule classifies an
//! *open-ended* literal shape — a milestoning date/`$`-variable
//! (`SourceMethodArg`/`SourceMethodArgSep`, `PropertyMethodArg`/
//! `PropertyMethodArgSep`), a store-method string argument (`StoreMethodArg`),
//! an extent-method's fixed-shape first argument (`ExtentMethodArg`), a
//! comparison operand (`ReValue`), and — the identifier-length axis the same
//! failure pattern names by name — a member navigation (`Member`) and a bound
//! `$`-variable reference (`RefVar`). The remaining narrowing positions
//! (`SourceIdent`, `SourceMethod`, `StoreMethod`, `ExtentMethod`, `Column`,
//! `RelationColumn`, `ScalarMethod`, `Reducer`, `ValueIdent`, the pure-operator
//! positions `Comparator`/`OrderedOperand`/`StrOperator`/`LogicalOperand`) key
//! on a **finite, schema- or vocabulary-fixed name/operator set** rather than an
//! open-ended literal grammar — there is no length/shape axis for a matrix to
//! walk, and `tests/l2_precision.rs`'s frozen-kill probes already hold each of
//! them to a real-vs-phantom witness pulled from the gold corpus. Extending this
//! lane's *approach* to a relation-column schema (`Column`/`RelationColumn`
//! need emitted-column data this suite's fixture does not carry) is future
//! work, not a gap silently left uncovered — `docs/spec/schema.md` §6.8 records
//! the scope call.
//!
//! **Sibling lane.** `tests/l2_value_shape_properties.rs` complements this
//! fixed matrix with a `proptest`-generated one, over the same grammar
//! productions and the same `drive` primitive, for the two shape families most
//! worth exploring beyond a curated list (a date literal's digit/separator
//! layout; an identifier's length/alphabet) — scoped, per its own doc comment,
//! to positions this fixed matrix already found sound.
//!
//! **Fixture.** The existing `tests/fixtures/schemas/milestoning.json`
//! (`l2_precision.rs`'s own `SourceMethodArgSep`/`PropertyMethodArgSep`
//! witnesses already load it), extended here with one new navigable property
//! (`Plain.proc`, filling the one milestoning arity the fixture did not yet
//! reach) and a scalar property of every comparable type class
//! (`sVal`/`fVal`/`dVal`, plus length-varied `String` members) — additive
//! changes only, so `l2_precision.rs`'s existing named-path probes are
//! unaffected. Reusing it rather than adding a second near-duplicate schema
//! keeps one fixture as the source of truth for "every milestoning arity",
//! matching constitution §4's DRY rule. Not one of
//! [`FIXTURE_DBS`](fixture_dbs::FIXTURE_DBS): it carries no gold-corpus query
//! (deliberately — the gold corpus predates the `temporal` field entirely).
#![forbid(unsafe_code)]

#[path = "support/byte_walk.rs"]
mod byte_walk;
#[path = "support/l2.rs"]
mod l2;
#[path = "support/lex.rs"]
mod lex;
#[path = "support/value_shapes.rs"]
mod value_shapes;

use byte_walk::{byte_vocab, drive};
use l2::load_schema;
use purecard::{CompiledGrammar, Schema};
use value_shapes::{
    BOOLEAN_WITNESSES, DATE_WITNESSES, DOLLAR_WITNESSES, IDENT_MEMBER_WITNESSES, INTEGER_WITNESSES,
    MILESTONE_WITNESS, NUMBER_WITNESSES, STRING_WITNESSES,
};

/// The db id of `tests/fixtures/schemas/milestoning.json`.
const DB: &str = "milestoning";

/// A fresh single-byte-vocabulary [`CompiledGrammar`] and the value-shapes
/// [`Schema`] fixture — built per test rather than shared, matching
/// `l2_liveness.rs`'s convention, since a `CompiledGrammar` deliberately caches
/// per-state masks and is not meant to be driven by more than one logical walk
/// family at a time.
fn fixture() -> (CompiledGrammar, Schema) {
    let (vocab, _eos) = byte_vocab();
    (CompiledGrammar::compile(vocab), load_schema(DB))
}

/// Drive `prefix`, then each of `witnesses` in turn, then `suffix`, through a
/// fresh [`fixture`] — the matrix sweep every test below is built from. A
/// witness that trips a masked byte panics inside [`drive`] with the exact
/// witness text and byte offset, so a regression here is locatable without
/// re-deriving it from a bisection (mirroring `spec_equivalence.rs`'s own
/// design intent for this suite's sibling invariant).
fn sweep(prefix: &str, witnesses: &[&str], suffix: &str) {
    let (grammar, schema) = fixture();
    for witness in witnesses {
        let text = format!("{prefix}{witness}{suffix}");
        drive(&grammar, &schema, &text);
    }
}

// ---------------------------------------------------------------------------
// SourceMethodArg / SourceMethodArgSep (S1, issue #384/#387/#391) — the
// pipeline source's own `all(...)` milestoning call.
//
// **Known failure (issue #391, tracked, not a new gap this sweep discovered
// silently).** Four tests below are `#[ignore = "..."]`-marked, each citing
// #391: `fill_source_method_arg` (`src/schema/narrow.rs`) reclassifies every
// candidate token from a fresh, whole-token `classify` read each time
// `L2Position::SourceMethodArg`/`PropertyMethodArg` is re-armed, which happens
// once per byte a byte-granular vocabulary spends inside an already-open date
// literal — so a single digit survives (`%1`, `%latest`'s own first byte) but a
// second digit does not (`%20`, and #391's own reported `%2026-01-15`). The
// *sibling* rule `PropertyMethodArg` (S3, issue #386) is marked the same way
// below, for the identical reason, and is **not** a second, separately-filed
// defect: its
// own doc comment states it "shares `fill_source_method_arg`'s fill" with
// `SourceMethodArg` verbatim, so the two positions run the exact same function
// on the exact same bug — one fix (#391) resolves both, and filing a second
// issue for the second call site of the same function would just fork one
// defect into two trackers. Un-ignore all four once #391 lands; the matrix
// itself needs no changes to become the regression pin.
// ---------------------------------------------------------------------------

#[test]
fn source_method_arg_unannotated_admits_every_date_and_dollar_shape() {
    // `Plain` carries no `temporal` field: the pre-#384 pass-through, where a
    // date/`$`-variable argument is legal at any count. Unaffected by #391,
    // which is specific to a `required`-arity call — this test is the sweep's
    // own proof that the harness is not vacuously green.
    sweep("|t::milestoning::Plain.all(", DATE_WITNESSES, ")");
    sweep("|t::milestoning::Plain.all(", &[MILESTONE_WITNESS], ")");
    sweep("|t::milestoning::Plain.all(", DOLLAR_WITNESSES, ")");
}

#[test]
#[ignore = "issue #391: fill_source_method_arg re-classifies a date literal's \
            second byte onward as a fresh candidate once SourceMethodArg is \
            re-armed for a required-arity class, masking it"]
fn source_method_arg_business_temporal_admits_every_date_and_dollar_shape_as_its_one_argument() {
    // `Biz` is business-temporal (arity 1) — issue #391's exact class: every
    // date shape must survive as the call's sole argument, not just `%latest`.
    sweep("|t::milestoning::Biz.all(", DATE_WITNESSES, ")");
    sweep("|t::milestoning::Biz.all(", &[MILESTONE_WITNESS], ")");
    sweep("|t::milestoning::Biz.all(", DOLLAR_WITNESSES, ")");
}

#[test]
#[ignore = "issue #391: fill_source_method_arg re-classifies a date literal's \
            second byte onward as a fresh candidate once SourceMethodArg is \
            re-armed for a required-arity class, masking it"]
fn source_method_arg_processing_temporal_admits_every_date_and_dollar_shape_as_its_one_argument() {
    sweep("|t::milestoning::Proc.all(", DATE_WITNESSES, ")");
    sweep("|t::milestoning::Proc.all(", &[MILESTONE_WITNESS], ")");
    sweep("|t::milestoning::Proc.all(", DOLLAR_WITNESSES, ")");
}

#[test]
#[ignore = "issue #391: fill_source_method_arg re-classifies a date literal's \
            second byte onward as a fresh candidate once SourceMethodArg is \
            re-armed for a required-arity class, masking it"]
fn source_method_arg_bitemporal_admits_every_date_and_dollar_shape_in_either_argument_slot() {
    // `Bi` is bitemporal (arity 2): every shape is walked once as the *first*
    // comma-separated argument (exercising `SourceMethodArgSep { remaining:
    // true }`'s own comma decision) and once as the *second* (exercising
    // `SourceMethodArgSep { remaining: false }`'s closer decision), each paired
    // with a fixed `%latest` in the other slot.
    for witness in DATE_WITNESSES
        .iter()
        .copied()
        .chain([MILESTONE_WITNESS])
        .chain(DOLLAR_WITNESSES.iter().copied())
    {
        sweep(
            "|t::milestoning::Bi.all(",
            &[witness],
            &format!(", {MILESTONE_WITNESS})"),
        );
        sweep(
            &format!("|t::milestoning::Bi.all({MILESTONE_WITNESS}, "),
            &[witness],
            ")",
        );
    }
}

// ---------------------------------------------------------------------------
// PropertyMethodArg / PropertyMethodArgSep (S3, issue #386) — a milestoned
// property navigation's own call, one position past S1. Shares
// `fill_source_method_arg` with `SourceMethodArg` above verbatim, so it
// inherits issue #391's exact defect at the identical byte offset — see the
// block comment above `SourceMethodArg`'s own tests for why this is one bug
// tracked once, not two.
// ---------------------------------------------------------------------------

#[test]
#[ignore = "issue #391 (shared fill_source_method_arg — see the SourceMethodArg \
            block comment above)"]
fn property_method_arg_business_temporal_admits_every_date_and_dollar_shape() {
    let prefix = "|t::milestoning::Plain.all()->filter(y|$y.biz(";
    sweep(prefix, DATE_WITNESSES, ")");
    sweep(prefix, &[MILESTONE_WITNESS], ")");
    sweep(prefix, DOLLAR_WITNESSES, ")");
}

#[test]
#[ignore = "issue #391 (shared fill_source_method_arg — see the SourceMethodArg \
            block comment above)"]
fn property_method_arg_processing_temporal_admits_every_date_and_dollar_shape() {
    let prefix = "|t::milestoning::Plain.all()->filter(y|$y.proc(";
    sweep(prefix, DATE_WITNESSES, ")");
    sweep(prefix, &[MILESTONE_WITNESS], ")");
    sweep(prefix, DOLLAR_WITNESSES, ")");
}

#[test]
#[ignore = "issue #391 (shared fill_source_method_arg — see the SourceMethodArg \
            block comment above)"]
fn property_method_arg_bitemporal_admits_every_date_and_dollar_shape_in_either_argument_slot() {
    let prefix = "|t::milestoning::Plain.all()->filter(y|$y.bi(";
    for witness in DATE_WITNESSES
        .iter()
        .copied()
        .chain([MILESTONE_WITNESS])
        .chain(DOLLAR_WITNESSES.iter().copied())
    {
        sweep(prefix, &[witness], &format!(", {MILESTONE_WITNESS})"));
        sweep(&format!("{prefix}{MILESTONE_WITNESS}, "), &[witness], ")");
    }
}

// ---------------------------------------------------------------------------
// StoreMethodArg / StoreMethodArgSep (N3d) — a store method's own call, whose
// every parameter is a `String[1]`.
// ---------------------------------------------------------------------------

#[test]
fn store_method_arg_admits_every_string_shape_in_either_argument_slot() {
    sweep(
        "|t::milestoning::Db->tableReference(",
        STRING_WITNESSES,
        ", 'fixed')",
    );
    sweep(
        "|t::milestoning::Db->tableReference('fixed', ",
        STRING_WITNESSES,
        ")",
    );
}

// ---------------------------------------------------------------------------
// ExtentMethodArg (N3h) — a class-extent builtin's own fixed-shape first
// argument (`->limit(Integer)`).
// ---------------------------------------------------------------------------

#[test]
fn extent_method_arg_limit_admits_every_integer_shape() {
    sweep(
        "|t::milestoning::Plain.all()->limit(",
        INTEGER_WITNESSES,
        ")",
    );
}

// ---------------------------------------------------------------------------
// ReValue (T1) — a comparison's operand, one matrix per type class.
// ---------------------------------------------------------------------------

#[test]
fn revalue_string_admits_every_string_shape() {
    sweep(
        "|t::milestoning::Plain.all()->filter(x|$x.sVal == ",
        STRING_WITNESSES,
        ")",
    );
}

#[test]
fn revalue_numeric_admits_every_number_shape() {
    sweep(
        "|t::milestoning::Plain.all()->filter(x|$x.a == ",
        NUMBER_WITNESSES,
        ")",
    );
}

#[test]
fn revalue_temporal_admits_every_date_and_milestone_shape() {
    let prefix = "|t::milestoning::Plain.all()->filter(x|$x.dVal == ";
    sweep(prefix, DATE_WITNESSES, ")");
    sweep(prefix, &[MILESTONE_WITNESS], ")");
}

#[test]
fn revalue_boolean_admits_every_boolean_shape() {
    sweep(
        "|t::milestoning::Plain.all()->filter(x|$x.fVal == ",
        BOOLEAN_WITNESSES,
        ")",
    );
}

// ---------------------------------------------------------------------------
// Member (N1) and RefVar (S2) — the identifier-length axis.
// ---------------------------------------------------------------------------

#[test]
fn member_navigation_admits_every_identifier_length() {
    sweep(
        "|t::milestoning::Plain.all()->filter(x|$x.",
        IDENT_MEMBER_WITNESSES,
        ")",
    );
}

#[test]
fn refvar_admits_every_bound_variable_name_length() {
    // The witness must appear in *both* the lambda's own binder and the
    // reference to it — `sweep`'s single-slot substitution cannot express
    // that, so this walks the matrix directly.
    let (grammar, schema) = fixture();
    for dollar_witness in DOLLAR_WITNESSES {
        let name = dollar_witness
            .strip_prefix('$')
            .expect("DOLLAR_WITNESSES entries are all `$`-prefixed");
        let text = format!("|t::milestoning::Plain.all()->filter({name}|${name})");
        drive(&grammar, &schema, &text);
    }
}
