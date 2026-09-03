//! A curated, comprehensive matrix of L1-legal **value literal shapes**, keyed
//! by the lexeme class an L2 narrowing rule admits — the fixed complement to
//! `tests/l2_value_shape_matrix.rs`'s sweep (`docs/spec/schema.md` §6.7's
//! third invariant, issue #391).
//!
//! The recurring failure pattern across issues #367/#377/#385/#391: a new L2
//! position gets a narrowing rule, the rule's own unit tests cover the 2-3
//! witness shapes the author thought of (`%latest`, a short `$var`, one short
//! date), but a *different* L1-legal shape at the same lexeme class — a
//! longer date literal, a date-time, a longer identifier, a string with a
//! doubled quote — never gets exercised. Issue #391 shipped exactly that way:
//! `fill_source_method_arg`'s own unit test built a single-token
//! `b"%2018-01-01"` candidate, never a *byte-granular* walk through one, so the
//! per-byte reclassification bug (`classify` reading each single-byte
//! candidate as a fresh token rather than a date-literal continuation) had no
//! test that could see it.
//!
//! Every witness here is cited against its own grammar production
//! (`docs/spec/grammar.md` §5.4's `literal`/`dateLit`/`strlit`/`number`/`refVar`
//! productions) — this module invents no shape the grammar does not itself
//! admit, and the sweep that drives these witnesses
//! ([`byte_walk::drive`](super::byte_walk::drive)) independently reconfirms
//! each one is L1-legal (via the plain, schema-free session it drives in
//! lockstep) before treating an L2 divergence as a finding.

/// `dateLit = "%" digit { dateChar | "." } ; dateChar = digit | "-" | "T" | ":"`
/// (`docs/spec/grammar.md` §5.4). Deliberately spans every digit-count and
/// separator shape the production admits, not only the calendar-realistic
/// ones — the byte-PDA has no notion of "valid calendar date", only the
/// character class, and a rule keyed on `classify`'s whole-token read is
/// exactly as vulnerable to a malformed-but-grammatical date as to a
/// well-formed long one.
pub const DATE_WITNESSES: &[&str] = &[
    "%1", // the shortest legal dateLit: one digit, no dateChar at all.
    "%20",
    "%2020",
    "%2020-01-01", // issue #391's own reported shape, one digit short.
    "%2026-01-15", // issue #391's exact reported literal.
    "%2024-02-29", // a leap day — still just digits/`-` to the byte-PDA.
    "%9999-12-31",
    "%2020-01-01T00:00:00",
    "%2020-01-01T23:59:59",
    "%2020-01-01T00:00:00.000",
    "%2020-01-01T00:00:00.000000",
    "%2020:01:01", // `dateChar` admits `:` anywhere the production allows it,
                   // not only inside a time-of-day — an adversarial but
                   // grammar-legal ordering.
];

/// `milestoneLit = "%latest"` (`docs/spec/grammar.md` §5.4), the engine's one
/// symbolic milestoning literal. Classified `Lexeme::Date` (`src/schema/scope.rs`)
/// alongside every [`DATE_WITNESSES`] entry, so a rule keyed on that
/// classification must treat the two alike.
pub const MILESTONE_WITNESS: &str = "%latest";

/// `refVar = "$" ident ; ident = alpha { alnum | "_" }` (`docs/spec/grammar.md`
/// §5.4), spanning the identifier-length axis the failure pattern names by
/// name ("a different-length identifier"): a single letter, a short camelCase
/// name, and a long snake/camel mix.
pub const DOLLAR_WITNESSES: &[&str] = &[
    "$x",
    "$asOf",
    "$businessDate",
    "$a_long_bound_variable_name_for_length_coverage",
];

/// `strlit = "'" { schar | "''" } "'"` (`docs/spec/grammar.md` §5.4): empty,
/// short, long, containing whitespace/digits, and the doubled-quote escape
/// (`docs/spec/grammar.md` §5.5) that a byte-granular walk must re-open a
/// literal through rather than treat as the literal's close.
pub const STRING_WITNESSES: &[&str] = &[
    "''",
    "'a'",
    "'T'",
    "'O''Brien'",
    "'with spaces and 123 numbers'",
    "'a fairly long single-quoted string literal for length coverage'",
];

/// `number = [ "-" ] ( digit { digit } [ frac ] | frac ) ; frac = "." digit
/// { digit } [ exp ] ; exp = ( "e" | "E" ) [ "+" | "-" ] digit { digit }`
/// (`docs/spec/grammar.md` §5.4) — every shape the production admits: bare
/// int, multi-digit int, leading-dot float, signed leading-dot float,
/// ordinary float, and scientific notation (only ever after a fractional
/// part, never a bare `1e3`, per the production's own comment).
pub const NUMBER_WITNESSES: &[&str] = &[
    "0",
    "7",
    "42",
    "123456789",
    "-5",
    ".5",
    "-.5",
    "3.14",
    "-3.14",
    "1.5e3",
    "1.5e-3",
    "-1.5E+10",
];

/// The [`NUMBER_WITNESSES`] subset an `Integer`-fixed slot (`ExtentArg::Integer`
/// in `src/schema/scope.rs`, e.g. `->limit(3)`'s own argument) can legally take
/// — no fractional/scientific shape, since `int = digit { digit }`
/// (`docs/spec/grammar.md` §5.4) is its own, narrower production.
pub const INTEGER_WITNESSES: &[&str] = &["0", "7", "42", "123456789"];

/// `boollit = "true" | "false"` (`docs/spec/grammar.md` §5.4) — the whole
/// production, so no matrix expansion is possible or needed.
pub const BOOLEAN_WITNESSES: &[&str] = &["true", "false"];

/// `ident = alpha { alnum | "_" }` (`docs/spec/grammar.md` §5.4), real member
/// names committed on the `l2_value_shapes` schema fixture
/// (`tests/fixtures/schemas/l2_value_shapes.json`), spanning the same
/// identifier-length axis [`DOLLAR_WITNESSES`] spans for a `$`-bound name —
/// this one for a plain member/property navigation.
pub const IDENT_MEMBER_WITNESSES: &[&str] = &[
    "a",
    "someMediumProp",
    "snake_case_prop_name",
    "aVeryLongPropertyNameForLengthCoverageTesting",
];
