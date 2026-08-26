# Spec: lexer-assignment-token

- Status: complete
- Created: 2026-08-26
- Owner: agent (analyzer syntax prerequisite)

## Problem

The target grammar defines `LetStmt = 'let' Ident '=' Expr`, but the analyzer
lexer has no token for a single `=`. It currently emits `ERROR` for the
assignment byte even though the adjacent equality operator `==` is supported.
The syntax layer therefore cannot exhaustively map the grammar's terminal set
without preserving a known lower-layer defect.

## Goals

- [x] Add a terminal-only `ASSIGN` kind for one `=` while retaining `EQ` for
      `==`.
- [x] Preserve longest-match behavior: `==` is one `EQ`, and `===` is `EQ`
      followed by `ASSIGN`.
- [x] Lex a representative `let x = 1` statement without an `ERROR` token and
      retain total, byte-accurate span coverage.
- [x] Bring the original lexer spec's completion state and dependency wording
      in line with the implementation that already shipped.

## Non-goals

- No syntax-tree, parser, `let`-statement, or assignment-validity behavior.
- No change to equality, comparison, arithmetic, or error recovery semantics.
- No dependency changes and no analyzer/PureCARD sharing.

## Design

This change touches only the analyzer lexer contract, implementation, tests,
and the design/spec prose that enumerates its terminals. `ASSIGN` is a fixed
literal token, and Logos maximal munch selects `EQ` for the two-byte spelling.
The lexer remains the bottom analyzer layer and gains no internal dependency.

## API / contract impact

`pure_analyzer_lexer::SyntaxKind` gains the public `ASSIGN` variant at the enum
tail, preserving every existing `repr(u16)` discriminant. Adding a variant is
source-breaking for an exhaustive downstream match, but the crate is
unpublished and no implemented downstream crate consumes its variant set yet.
The future syntax layer must map the enum exhaustively so later terminal drift
becomes a compile-time failure.

## Testing plan

Write failing unit assertions before adding the variant: a `let` assignment,
the separated `= ==` spellings, the ambiguous `===` longest-match case, and
the exhaustive symbol fixture. Run `just test-unit`, `just ci`, `just review`,
and the relevant full pre-PR gates.

## Risks & rollout

The lexer behavior is additive and localized; the public enum addition is a
deliberate pre-stability API change. The only ordering risk is accidentally
splitting `==`; the longest-match tests pin that boundary. Reverting the single
variant/token rule restores the previous behavior without data migration.
