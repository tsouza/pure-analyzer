# Spec: lexer

- Status: complete
- Created: 2026-07-22
- Owner: agent (autonomous v0.1 build-out)

## Problem

`pure-analyzer-lexer` began as a hollow `version()`-only stub. Every crate
above it in the DAG (`syntax`, `parser`, and transitively everything else)
needed a real token stream before work could start there. This feature was the
first concrete v0.1 milestone (design doc §9): the token layer.

## Goals

- [x] A `logos`-derived tokenizer producing every token class design doc §4.1
      specifies: the date family (longest-match-first: `DATE_TIME`,
      `STRICT_DATE`, `LATEST_DATE`, bare `PERCENT`), symbols, the five M3
      keywords (`all let allVersions allVersionsInRange toBytes`), assignment
      (`=`), the four
      literal classes (`IDENT INTEGER STRING BOOLEAN`), the island raw tokens
      from §2.3 (`#>{`, `#{`, `#/...#`, bare `#`, `{`, `}`, `}#`), and trivia
      (whitespace, `//` line comments, `/* */` block comments) kept as real
      tokens, never skipped — required for `fmt`'s losslessness later.
- [x] Total coverage: every byte of input is accounted for by some token,
      including unrecognized bytes (an explicit `ERROR` kind) — no gaps, no
      panics, on arbitrary input (constitution §1: no `panic!`/`unwrap` outside
      tests; design doc §4.2's "total parsing" invariant applies at this layer
      too even though the parser itself is a separate future feature).
- [x] Public surface: `pub enum SyntaxKind` (`#[repr(u16)]`, the literal type
      name design doc §4.1 specifies as the lexer's output) and
      `pub fn lex(text: &str) -> Vec<(SyntaxKind, TextRange)>`, using
      `text-size::TextRange` (see `docs/dependencies/text-size.md`) so spans
      are byte-identical to what every later layer (`Diagnostic`, the CST)
      uses — no conversion at any boundary.

## Non-goals

- **No `pure-analyzer-syntax` integration.** `SyntaxKind` here covers
  **terminal (token) kinds only**. `ALLOWED_INTERNAL_DEPS` in `xtask` fixes
  `pure-analyzer-lexer -> []` (zero internal deps) and
  `pure-analyzer-syntax -> [pure-analyzer-lexer]` — the lexer cannot depend on
  syntax's (future) richer CST-kind enum, and this feature doesn't reach into
  what that enum looks like. `pure-analyzer-syntax` is a separate future spec.
- **No island balancing.** §2.3 is explicit: `logos` cannot cleanly implement
  a nesting `{`/`}` depth stack, so island tokens are lexed as flat raw
  tokens (`#>{`, `#{`, `#/...#`, `#`, `{`, `}`, `}#`) with balancing left to
  the parser. This crate does not validate island structure at all.
- **No float/decimal literal.** §4.1's literal list is exactly `IDENT INTEGER
  STRING BOOLEAN`; §5.3 hints numeric-literal richness exists elsewhere, but
  inventing a `FLOAT`/`DECIMAL` kind not named at this layer is scope creep.
  `INTEGER` is `[0-9]+`; a leading `-` is a separate arithmetic-minus token,
  disambiguated from unary negation at the parser layer, not here.
- **No exhaustive arithmetic/comparison operator set from the doc** — §4.1
  says only "arithmetic" without an operator list. This spec adds the
  standard set (`+ - * /`, `< <= > >=`) as a documented interpretation, to be
  corrected against the real grammar once the differential corpus (§8) exists.
  Flagged explicitly rather than silently assumed.
- **Single `=` is an assignment token, not an equality operator.** §4.2 defines
  `LetStmt = 'let' Ident '=' Expr`; `ASSIGN` represents that terminal while
  `EQ` continues to represent `==`. Logos longest-match behavior keeps `==`
  whole.
- **`SEMICOLON` isn't in §4.1's symbol list either, but is added anyway** — it
  is unambiguously required: §4.2's own grammar defines
  `CodeBlock = Stmt (';' Stmt)*` for multi-statement lambda bodies, and the
  design doc's own §1.1 worked example ends its query with `;`. Omitting it
  would make that worked example fail to lex cleanly. Same
  documented-interpretation caveat as the arithmetic set above.
- **String-escaping choice is a documented assumption, not verified against
  the real engine here**: single-quoted strings (`'...'`) with `''` as the
  escape for a literal quote — Pascal/SQL-style, not backslash-escaping. This
  matches `purecard`'s own engine-differentially-tested grammar notes (its
  ADR-0004: "ident/classpath/strlit (`''`-doubling) lexis"), which is the
  strongest evidence available without running the real Legend engine
  ourselves. Worth a differential-corpus check later (see the
  `project_purecard_relationship` decision — no code sharing yet, but this one
  fact is corroborating evidence, not adopted code).

## Design

Touches only `crates/pure-analyzer-lexer`. New dependencies: `logos` (with the
`forbid_unsafe` feature — its default codegen can emit `unsafe`, which would
make `#![forbid(unsafe_code)]` fail to compile in this crate) and `text-size`
(vetted: `docs/dependencies/text-size.md`; already a direct dependency of
`pure-analyzer-diagnostics`, and the future syntax-tree dependency resolves
the same span crate).

`SyntaxKind` is a flat `#[derive(logos::Logos)]` enum, `#[repr(u16)]`,
`Clone + Copy + PartialEq + Eq + Hash + Debug`. `lex()` drives `SyntaxKind::lexer(text)`
and folds every `Ok`/`Err` result into a `(SyntaxKind, TextRange)` pair — `Err`
results map to `SyntaxKind::ERROR`, so the output covers 100% of the input with
no gaps, ever.

## API / contract impact

New public API surface in a not-yet-published crate (`publish = false`) — no
semver concern yet. `cargo public-api`/`cargo semver-checks` baselines aren't
enabled yet either (see README's "optional gates" note).

## Testing plan

Unit tests in `crates/pure-analyzer-lexer/src/lib.rs` (this layer has no
integration/e2e surface — it's a pure function). Written first, confirmed
failing before implementation, per `start-feature`. Coverage:

- Each token class from §4.1, including the island raw tokens and the
  longest-match ordering within the date family (`%latest` inside a longer
  `%YYYY-MM-DD` shouldn't misfire, and vice versa).
- Assignment/equality separation, including `let x = 1`, `= ==`, and the
  longest-match boundary `===` → `[EQ, ASSIGN]`.
- Trivia (whitespace, both comment forms) round-trips as real tokens whose
  concatenated spans cover the whole input — the losslessness property `fmt`
  will depend on later.
- Total-coverage property: for a handful of fixture strings (including
  garbage bytes), the emitted spans are contiguous and sum to the input length
  with no panics.
- The worked query from design doc §1.1 (`#>{db::testDB.personTable}#->join(...)`)
  lexes end-to-end without an unexpected `ERROR` token.

## Risks & rollout

Pure addition with no implemented downstream consumer yet — zero runtime blast
radius. The documented non-goals (arithmetic operator set, string
escaping) are the main risk: both are best-effort interpretations pending the
real differential corpus, called out explicitly rather than silently assumed,
so a future correction is a clean, expected diff rather than a surprise.
