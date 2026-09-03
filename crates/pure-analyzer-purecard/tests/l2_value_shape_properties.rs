//! Property-based complement to `tests/l2_value_shape_matrix.rs`
//! (`docs/spec/schema.md` §6.8): the fixed matrix catches every shape category
//! a human enumerated (a leap-day date, a doubled-quote string, a scientific
//! number); this lane generates shapes nobody enumerated at all, over the same
//! grammar productions (`docs/spec/grammar.md` §5.4's `dateLit`/`ident`) and the
//! same [`byte_walk::drive`] primitive, so a counterexample here is exactly as
//! locatable (witness text, byte offset) as a fixed-matrix failure.
//!
//! Covers both the ordinary comparison-operand position (`Plain`'s
//! `ReValue(Temporal)`) and the milestoning-arity position issue #391 broke
//! (`Biz`'s business-temporal `SourceMethodArg`, PR #393) — now that #393 has
//! landed, randomized exploration of the exact position the regression lived
//! in is a genuine, standing regression guard against it recurring, not a
//! rediscovery of an already-tracked gap.
//!
//! Uses `proptest`'s fixed, committed config with default source-parallel
//! failure persistence, matching `tests/mask_properties.rs`'s own convention
//! (§8.5, G2): a counterexample's seed is written to
//! `tests/proptest-regressions/` and committed with its fix. No property here
//! has failed, so that directory holds no seed from this file.
#![forbid(unsafe_code)]

#[path = "support/byte_walk.rs"]
mod byte_walk;
#[path = "support/l2.rs"]
mod l2;
#[path = "support/lex.rs"]
mod lex;

use byte_walk::{byte_vocab, drive};
use l2::load_schema;
use proptest::prelude::*;
use purecard::CompiledGrammar;

/// The db id of `tests/fixtures/schemas/milestoning.json`, shared with
/// `l2_value_shape_matrix.rs`.
const DB: &str = "milestoning";

/// The deterministic proptest case count for this lane (constitution §4 — no
/// magic constants; gate configuration, tunable up only), matching
/// `tests/mask_properties.rs`'s own pin.
const PROPTEST_CASES: u32 = 256;

/// Every `dateLit` shape the byte-PDA's own state machine admits
/// (`src/grammar/pda.rs`'s `step_in_date_lit`/`date_field`/`step_in_date_time`/
/// `step_in_date_frac`), not `docs/spec/grammar.md` §5.4's own looser EBNF gloss
/// (`dateLit = "%" digit { dateChar | "." }`) taken literally: the executable
/// state machine is *stricter* than that prose — a `-`/`T`/`:` separator owes at
/// least one digit immediately after it (`date_field`'s own doc comment: "the
/// field it opens owes at least one digit, so the literal can neither end nor
/// branch here"), so `%0-` alone is a dead state at L1 itself, confirmed live
/// against this generator during this suite's own development (not an L2
/// finding — `pda.rs` is the ground truth here, and the EBNF gloss is a
/// simplification worth tightening in a follow-up doc pass, out of scope for
/// this soundness sweep). The regex mirrors the real shape: a digit run, zero or
/// more `-`/`T`-separated digit runs (the date half), optionally handed to a
/// `:`-separated time half with the same repetition, optionally closed by one
/// `.`-separated fractional run.
fn date_literal_strategy() -> impl Strategy<Value = String> {
    r"%[0-9]+((-|T)[0-9]+)*(:[0-9]+((-|:)[0-9]+)*(\.[0-9]+)?)?"
}

/// `ident = alpha { alnum | "_" }` (`docs/spec/grammar.md` §5.4) — a letter,
/// then 0-24 further letters/digits/underscores.
fn ident_strategy() -> impl Strategy<Value = String> {
    "[a-zA-Z][a-zA-Z0-9_]{0,24}"
}

proptest! {
    #![proptest_config(ProptestConfig { cases: PROPTEST_CASES, ..ProptestConfig::default() })]

    /// Every generated `dateLit` streams to completion as the pipeline
    /// source's own unannotated milestoning argument (`Plain` carries no
    /// `temporal` field — the one `SourceMethodArg` variant issue #391 does
    /// not affect).
    #[test]
    fn every_generated_date_literal_streams_unannotated(date in date_literal_strategy()) {
        let (vocab, _eos) = byte_vocab();
        let grammar = CompiledGrammar::compile(vocab);
        let schema = load_schema(DB);
        let text = format!("|t::milestoning::Plain.all({date})");
        drive(&grammar, &schema, &text);
    }

    /// Every generated `dateLit` streams to completion as a T1 comparison
    /// operand against a `StrictDate`-typed member.
    #[test]
    fn every_generated_date_literal_streams_as_a_comparison_operand(date in date_literal_strategy()) {
        let (vocab, _eos) = byte_vocab();
        let grammar = CompiledGrammar::compile(vocab);
        let schema = load_schema(DB);
        let text = format!("|t::milestoning::Plain.all()->filter(x|$x.dVal == {date})");
        drive(&grammar, &schema, &text);
    }

    /// Every generated `dateLit` streams to completion as a business-temporal
    /// class's own sole milestoning argument — the exact position and shape
    /// axis issue #391 broke (a bare-year date's second digit onward masked),
    /// fixed by PR #393. A standing regression guard: randomized exploration
    /// of this position recurring in CI is what catches a future re-break of
    /// the same fix that a fixed matrix entry alone would not, if the
    /// regression were narrower than any single curated witness.
    #[test]
    fn every_generated_date_literal_streams_as_a_single_temporal_argument(date in date_literal_strategy()) {
        let (vocab, _eos) = byte_vocab();
        let grammar = CompiledGrammar::compile(vocab);
        let schema = load_schema(DB);
        let text = format!("|t::milestoning::Biz.all({date})");
        drive(&grammar, &schema, &text);
    }

    /// Every generated `ident` streams to completion as a lambda's own bound
    /// `$`-variable reference, at every length the production admits.
    #[test]
    fn every_generated_identifier_streams_as_a_bound_refvar(name in ident_strategy()) {
        let (vocab, _eos) = byte_vocab();
        let grammar = CompiledGrammar::compile(vocab);
        let schema = load_schema(DB);
        let text = format!("|t::milestoning::Plain.all()->filter({name}|${name})");
        drive(&grammar, &schema, &text);
    }
}
