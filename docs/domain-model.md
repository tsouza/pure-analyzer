# Domain Model

The evolving statement of what the `pure-analyzer` umbrella contains and what
has actually landed. The repository holds two independent products:
`pure-analyzer`, an early-scaffold static analyzer, and
`pure-analyzer-purecard`, a constrained decoder with M0–M5 code artifacts
implemented and documented end-to-end proof obligations still open. The analyzer
[design document](design/pure-analyzer-design.md) remains the target source for
its intended grammar, milestoning algorithm, and subcommand contracts; it is
not evidence that those capabilities are implemented. PureCARD's shipped
contract lives in its [nested product documentation](../crates/pure-analyzer-purecard/docs/).

This document is **EVOLVABLE**. It is the elaboration of the domain section of
[`../constitution.md`](../constitution.md): the constitution states the
non-negotiable domain *rules*; this file describes the *entities, workflows, and
invariants* those rules govern. When the two disagree, the constitution wins.
For target analyzer behavior, an unexplained disagreement with the design doc
is a design-governance bug. For present-tense implementation status, this file
and the code are authoritative.

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

### Umbrella repository

**What it is.** A Cargo workspace and governance boundary containing two
independent sibling products plus shared repository infrastructure.

**Invariants.** Analyzer crates and PureCARD have zero Cargo dependency edges
in either direction, including normal, development, build, optional, and renamed
dependencies. `xtask`, `just`, and root CI may orchestrate either product but
are not product layers. Co-location alone grants no shared parser, corpus,
runtime architecture, product ownership, or release authority.

**Relationships.** Contains the `pure-analyzer` and PureCARD product entities.
Any parser or corpus integration between them requires a future spec and ADR.

**Introduced by.**
[`specs/migrate-purecard-structural-move.md`](../specs/migrate-purecard-structural-move.md)
· [`specs/reconcile-purecard-governance.md`](../specs/reconcile-purecard-governance.md)
· [ADR-0004](decisions/0004-purecard-independent-workspace-product.md) ·
[PureCARD ADR-0009](../crates/pure-analyzer-purecard/docs/decisions/0009-monorepo-placement.md).

### pure-analyzer product

**What it is.** A planned deterministic, standalone static-analysis toolchain
for Legend Pure's modern `Relation<>` dialect.

**Invariants.** Its processing order is `lexer → syntax → parser → model
→ resolve → analysis → libpure → cli`. Cargo edges point toward
prerequisites: resolver may depend on model; model must not depend on resolver.
Diagnostics is a shared analyzer leaf. Runtime analysis remains mechanical:
no LLM, network, clock, or runtime Legend engine.

**Relationships.** Lexer and diagnostics currently have substantive
implementations. Syntax, parser, model, resolve, analysis, and `libpure` are
mostly version-reporting stubs; CLI subcommands return `not implemented yet`.
The design document describes the intended later product.

**Introduced by.** Repository bootstrap ·
[ADR-0003](decisions/0003-analysis-engine-crate-dag.md).

### PureCARD product

**What it is.** A constrained decoder for Legend Pure whose fixed PDA and
implemented partial schema overlay mask the next tokens those constraints
reject during language-model decoding.

**Invariants.** PureCARD constrains output to membership in its hand-written
emitted-subset PDA and, when given a schema, narrows tokens only at the positions
covered by its implemented N/T rules. Those constraints are not a general Pure
syntax or schema-validity guarantee, and accepted output is not yet guaranteed
to compile. It does not claim semantic faithfulness or feature completeness;
the [documented end-to-end proof obligations](../crates/pure-analyzer-purecard/docs/spec/overview.md#10-milestone-implementation-status-m0m5)
remain. The migrated Cargo package remains
unpublished (`publish = false`); Python wheels built by CI are verification
artifacts only.

**Relationships.** PureCARD is a sibling product, not an analyzer front end or
a node in ADR-0003's crate DAG. It owns its decoder implementation, nested docs,
gold corpus, specialized tests, fuzz targets, and Python boundary.

**Introduced by.**
[`specs/migrate-purecard-structural-move.md`](../specs/migrate-purecard-structural-move.md)
· [ADR-0004](decisions/0004-purecard-independent-workspace-product.md).

### Diagnostic

**What it is.** The implemented analyzer output model intended for every pass:
a `code` (`PUR<nnnn>`), `severity`, `message`, a primary + secondary set of
file/byte-range `Label`s, an optional structured `Fix` (span + replacement
edits, never a rendered string), an optional `eq`/`diff` `Verdict`, and an
optional `ReasonCode` explaining an `Indecisive` verdict or a downgrade under
model under-resolution. See design doc §6.1.

**Invariants.** `Diagnostic` carries no renderer-specific state — no ANSI
codes, no LSP types — so future CLI and LSP front ends can render identical
findings from the same value. `code` is a closed `DiagCode` enum and `reason` is
a closed `ReasonCode` enum: every identifier, reason bucket, and explanatory
blurb is registered at compile time rather than constructed by a pass. The
diagnostic remains serialization-only because findings flow one way, from
passes to renderers; boundary-facing code and reason parsers accept only exact
registered identifiers.

**Relationships.** Analyzer-only. It is intended to be produced by every crate
from `pure-analyzer-parser` upward (parser syntax errors,
`pure-analyzer-analysis`'s validate/lint passes, later `pure-analyzer-eq`'s
verdicts). Today the diagnostic model itself is implemented while most planned
producers remain scaffolds. `Label.file`/`.span` use `FileId`/`TextRange` from
this crate and `text-size` respectively, the span representation intended for
the lexer/syntax/parser layers.

**Introduced by.** Repository bootstrap; closed registries are implemented in
`crates/pure-analyzer-diagnostics/`.

## Workflows

There is intentionally no cross-product runtime workflow. Analyzer execution
workflows remain target designs until their layers land. PureCARD's implemented
decode/session workflows live in its
[`docs/spec/`](../crates/pure-analyzer-purecard/docs/spec/). A root change may
orchestrate both products for validation, but orchestration does not create a
runtime relationship.

## Cross-cutting invariants

- **Independent products.** Analyzer and PureCARD runtime code never depend on
  one another. Shared repository automation may invoke both without becoming a
  product dependency or transferring ownership of product assets.

- **Analyzer mechanical determinism (design doc §1.3).** No LLM, no network, no clock,
  no randomness in output. Identical `(inputs, model, config, flags)` with
  `--jobs 1` must produce byte-identical output and exit code. This governs
  every analyzer pass, not just a specific subcommand — a `HashMap` in a render
  or model path is a bug, not a style nit.
- **`eq` soundness is sacred (design doc §1.3, §5.3).** `eq`/`diff` must never
  wrongly commit `EQUIVALENT` or `NOT_EQUIVALENT`; "don't know" always maps to
  `Indecisive`, never a guess.

## Glossary

None yet — define domain terms here so specs and code share one vocabulary.
