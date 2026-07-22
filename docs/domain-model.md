# Domain Model

The evolving statement of **what pure-analyzer is and does**. Unlike a fresh
fork of the starter kit, this project did not start domain-empty: it
instantiates [`docs/design/pure-analyzer-design.md`](design/pure-analyzer-design.md),
a complete, implementation-ready specification for a mechanical static-analysis
toolchain for Legend Pure. That document is the authoritative source for
background, grammar, the milestoning-arity algorithm, and subcommand
contracts — this file elaborates only the entities/workflows/invariants that
have actually landed in code, one feature at a time, each addition arriving
through a reviewer-approved PR.

This document is **EVOLVABLE**. It is the elaboration of the domain section of
[`../constitution.md`](../constitution.md): the constitution states the
non-negotiable domain *rules*; this file describes the *entities, workflows, and
invariants* those rules govern. When the two disagree, the constitution wins.
When either disagrees with the design doc on a point the design doc has already
settled, treat that as a bug to fix, not a license to diverge.

## How to use this file

- When a feature introduces a new domain concept, add or update its entry here as
  part of the same PR that adds the code.
- Keep it a **model**, not a changelog. Describe the current truth; history lives
  in git and in [`decisions/`](decisions/).
- No filler. If an entry says nothing a reader couldn't infer from the type names,
  delete it.
- Cross-link the spec (`specs/<name>.md`) that introduced each concept.

## Template for a new entry

Copy this block per concept.

```markdown
### <Concept name>

**What it is.** One or two sentences. The thing, not the implementation.

**Invariants.** The rules that must always hold. These are candidates for
enforcement in `domain` types (make illegal states unrepresentable) and in tests.

**Relationships.** How it connects to other concepts in this model.

**Introduced by.** `specs/<name>.md` · ADR <nnnn> (if any).
```

---

## Entities

### Diagnostic

**What it is.** The single output shape every pass produces: a `code`
(`PUR<nnnn>`), `severity`, `message`, a primary + secondary set of
file/byte-range `Label`s, an optional structured `Fix` (span + replacement
edits, never a rendered string), an optional `eq`/`diff` `Verdict`, and an
optional `ReasonCode` explaining an `Indecisive` verdict or a downgrade under
model under-resolution. See design doc §6.1.

**Invariants.** `Diagnostic` carries no renderer-specific state — no ANSI
codes, no LSP types — so the CLI and (in v0.2) the LSP render identical
findings from the same value. `code` and `ReasonCode::id`/`blurb` are
`&'static str`: every code is a compile-time constant a pass references, never
one it constructs at runtime, which is also why `Diagnostic` and `ReasonCode`
are `Serialize`-only (a `&'static str` field cannot soundly round-trip through
`Deserialize`) — findings flow one way, from passes to renderers.

**Relationships.** Produced by every crate from `pure-analyzer-parser` upward
(parser syntax errors, `pure-analyzer-analysis`'s validate/lint passes, later
`pure-analyzer-eq`'s verdicts). `Label.file`/`.span` use `FileId`/`TextRange`
from this crate and `text-size` respectively, the same span representation the
lexer/syntax/parser layers use, so a diagnostic's span is directly comparable
to a CST node's range with no conversion.

**Introduced by.** Repository bootstrap (no `specs/` entry — this is the
verbatim design doc §6.1 shape, not a design decision made during
implementation). `crates/pure-analyzer-diagnostics/`.

## Workflows

None yet.

## Cross-cutting invariants

- **Mechanical determinism (design doc §1.3).** No LLM, no network, no clock,
  no randomness in output. Identical `(inputs, model, config, flags)` with
  `--jobs 1` must produce byte-identical output and exit code. This governs
  every pass, not just a specific subcommand — a `HashMap` in a render or
  model path is a bug, not a style nit.
- **`eq` soundness is sacred (design doc §1.3, §5.3).** `eq`/`diff` must never
  wrongly commit `EQUIVALENT` or `NOT_EQUIVALENT`; "don't know" always maps to
  `Indecisive`, never a guess.

## Glossary

None yet — define domain terms here so specs and code share one vocabulary.
