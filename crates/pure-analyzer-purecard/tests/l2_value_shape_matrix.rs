//! The **value-shape matrix**: a systematic, per-rule sweep proving that every
//! narrowing rule which admits an open-ended literal class (a milestoning date,
//! a `$`-bound variable, a string, a number, a member identifier) admits *every*
//! grammar-legal shape of that class, byte-by-byte, not only the 2-3 witnesses
//! its own introducing PR happened to pick (`docs/spec/schema.md` §6.8, the
//! third L2 invariant).
//!
//! **The gap this closes.** `L2 ⊆ L1` (G4, `docs/spec/schema.md` §6.5) says the
//! overlay may never *widen* L1; the completed-term half of §6.7 says a rule
//! that permits a class by its *opening* byte may not clear every way of
//! *ending* the term it opened. Neither statement covered issue #391's shape
//! (fixed by PR #393, merged the same day): `ScopeTracker::source_method_arg_sep`/
//! `property_method_arg_sep` fire — correctly — at every byte-PDA state a
//! milestoning date/variable argument can be *mid-lexeme* at (`InDateLit`,
//! `InDateTime`, `InDateFrac`, `InMilestoneLit`, `InMemberIdent`), to decide the
//! `,`-vs-`)` separator right after a *completed* argument. Before #393,
//! `fill_source_method_arg_sep` applied that arity-gated separator set to
//! *every* byte reached at those states, continuation bytes included — so a
//! bare-year date literal's second digit (`%2026-01-15`'s own `0`) was treated
//! as if it had to close or extend the argument list, rather than merely
//! continue the still-open lexeme underneath it. `%latest` and a single-char
//! `$d` never hit this: `%latest` folds every byte straight through
//! `InMilestoneLit` to a value-terminal state in one step, and `$d` never
//! revisits `InMemberIdent` a second time — the exact two shapes #387's own
//! tests covered, and the exact two shapes too short to expose a bug that only
//! exists in re-visiting an open lexeme's state a second time.
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
// SourceMethodArg / SourceMethodArgSep (S1, issue #384/#387; the regression
// PR #393 fixed was issue #391) — the pipeline source's own `all(...)`
// milestoning call.
//
// **Formerly a known failure (issue #391), now the fix's own acceptance
// check.** Before #393, `fill_source_method_arg_sep` gated a byte at
// `InDateLit`/`InDateTime`/`InMemberIdent` by the arity decision alone,
// treating a date literal's *own continuation byte* as if it had to be the
// owed separator — so a single digit survived (`%1`, `%latest`'s own first
// byte) but a second digit did not (`%20`, and #391's own reported
// `%2026-01-15`). #393 fixed it by reading the byte-PDA's own transition
// table at that state, so a continuation byte defers to L1 regardless of the
// arity gate. The four tests below (plus `PropertyMethodArg`'s three, its
// shared-code sibling one section down) were `#[ignore = "..."]`-marked while
// #391 was still open; now that #393 has merged, they run for real and are
// this fix's own acceptance check — the exact shape #387's own tests never
// covered (a real multi-digit date, not just `%latest`/`$d`).
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
fn source_method_arg_business_temporal_admits_every_date_and_dollar_shape_as_its_one_argument() {
    // `Biz` is business-temporal (arity 1) — issue #391's exact class: every
    // date shape must survive as the call's sole argument, not just `%latest`.
    sweep("|t::milestoning::Biz.all(", DATE_WITNESSES, ")");
    sweep("|t::milestoning::Biz.all(", &[MILESTONE_WITNESS], ")");
    sweep("|t::milestoning::Biz.all(", DOLLAR_WITNESSES, ")");
}

#[test]
fn source_method_arg_processing_temporal_admits_every_date_and_dollar_shape_as_its_one_argument() {
    sweep("|t::milestoning::Proc.all(", DATE_WITNESSES, ")");
    sweep("|t::milestoning::Proc.all(", &[MILESTONE_WITNESS], ")");
    sweep("|t::milestoning::Proc.all(", DOLLAR_WITNESSES, ")");
}

#[test]
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
// property navigation's own call, one position past S1. Shared
// `fill_source_method_arg_sep` with `SourceMethodArg` above verbatim, so #393
// fixed both at once — see the block comment above `SourceMethodArg`'s own
// tests for why this was one bug tracked once, not two.
// ---------------------------------------------------------------------------

#[test]
fn property_method_arg_business_temporal_admits_every_date_and_dollar_shape() {
    let prefix = "|t::milestoning::Plain.all()->filter(y|$y.biz(";
    sweep(prefix, DATE_WITNESSES, ")");
    sweep(prefix, &[MILESTONE_WITNESS], ")");
    sweep(prefix, DOLLAR_WITNESSES, ")");
}

#[test]
fn property_method_arg_processing_temporal_admits_every_date_and_dollar_shape() {
    let prefix = "|t::milestoning::Plain.all()->filter(y|$y.proc(";
    sweep(prefix, DATE_WITNESSES, ")");
    sweep(prefix, &[MILESTONE_WITNESS], ")");
    sweep(prefix, DOLLAR_WITNESSES, ")");
}

#[test]
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
