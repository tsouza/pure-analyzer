# Domain Model

The durable domain model of the `pure-analyzer` umbrella. The repository holds
two independent products: `pure-analyzer`, a static analyzer, and
`pure-analyzer-purecard`, a constrained decoder. PureCARD's product contract
lives in its [nested documentation](../crates/pure-analyzer-purecard/docs/).

This document is **EVOLVABLE**. It is the elaboration of the domain section of
[`../constitution.md`](../constitution.md): the constitution states the
non-negotiable domain *rules*; this file describes the *entities, workflows, and
invariants* those rules govern. When the two disagree, the constitution wins.
The code, tests, and contract documentation are authoritative for present
behavior. GitHub Issues and PRs carry mutable work state.

## How to use this file

- When a feature introduces a new domain concept, add or update its entry here as
  part of the same PR that adds the code.
- Keep it a **model**, not a changelog. Describe the current truth; history lives
  in git and in [`decisions/`](decisions/).
- No filler. If an entry says nothing a reader couldn't infer from the type names,
  delete it.
- Cross-link an ADR or product contract when it clarifies the present contract.

## Template for a new entry

Copy this block per concept.

```markdown
### <Concept name>

**What it is.** One or two sentences. The thing, not the implementation.

**Invariants.** The rules that must always hold. These are candidates for
enforcement in `domain` types (make illegal states unrepresentable) and in tests.

**Relationships.** How it connects to other concepts in this model.

**Authority.** Source crate · ADR <nnnn> (if any).
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
Any parser or corpus integration between them requires a future issue and ADR.

**Authority.** [ADR-0004](decisions/0004-purecard-independent-workspace-product.md) ·
[PureCARD ADR-0009](../crates/pure-analyzer-purecard/docs/decisions/0009-monorepo-placement.md).

### pure-analyzer product

**What it is.** A deterministic, standalone static-analysis toolchain for
Legend Pure's modern `Relation<>` dialect.

**Invariants.** Its processing order is `lexer → syntax → parser → model
→ resolve → analysis → libpure → front ends (CLI, LSP)`. Cargo edges point toward
prerequisites: resolver may depend on model; model must not depend on resolver.
Diagnostics is a shared analyzer leaf. Runtime analysis remains mechanical:
no LLM, network, clock, or runtime Legend engine.

**Relationships.** The analyzer pipeline is composed from the crate layers
listed above; `libpure` is its facade and the CLI and LSP are transport-specific front-end boundaries.

**Authority.** Repository bootstrap ·
[ADR-0003](decisions/0003-analysis-engine-crate-dag.md).

### PureCARD product

**What it is.** A constrained decoder for Legend Pure whose fixed PDA and
implemented partial schema overlay mask the next tokens those constraints
reject during language-model decoding.

**Invariants.** PureCARD constrains output to membership in its hand-written
emitted-subset PDA and, when given a schema, narrows tokens only at the positions
covered by its N/T rules. These constraints are not a general Pure syntax or
schema-validity guarantee, and they do not establish semantic faithfulness. The
migrated Cargo package remains
unpublished (`publish = false`); Python wheels built by CI are verification
artifacts only.

**Relationships.** PureCARD is a sibling product, not an analyzer front end or
a node in ADR-0003's crate DAG. It owns its decoder implementation, nested docs,
gold corpus, specialized tests, fuzz targets, and Python boundary.

**Authority.** [ADR-0004](decisions/0004-purecard-independent-workspace-product.md).

### Diagnostic

**What it is.** The analyzer output model used by every pass:
a `code` (`PUR<nnnn>`), `severity`, `message`, a primary + secondary set of
file/byte-range `Label`s, an optional structured `Fix` (span + replacement
edits, never a rendered string), an optional `eq`/`diff` `Verdict`, and an
optional `ReasonCode` explaining an `Indecisive` verdict or a downgrade under
model under-resolution.

**Invariants.** `Diagnostic` carries no renderer-specific state — no ANSI
codes or protocol types. `code` is a closed `DiagCode` enum and `reason` is a
closed `ReasonCode` enum: every identifier, reason bucket, and explanatory
blurb is registered at compile time rather than constructed by a pass. The
diagnostic remains serialization-only because findings flow one way, from passes
to renderers; boundary-facing code and reason parsers accept only exact
registered identifiers.

**Relationships.** Analyzer-only. It is produced by analyzer passes and carries
parser, analysis, and verdict diagnostics without renderer-specific state.
`Label.file`/`.span` use `FileId`/`TextRange` from this crate and `text-size`
respectively.

**Authority.** `crates/pure-analyzer-diagnostics/`.

## Workflows

There is intentionally no cross-product runtime workflow. PureCARD's
decode/session workflows live in its
[`docs/spec/`](../crates/pure-analyzer-purecard/docs/spec/). Root automation may
validate both products, but orchestration does not create a runtime relationship.

## Cross-cutting invariants

- **Independent products.** Analyzer and PureCARD runtime code never depend on
  one another. Shared repository automation may invoke both without becoming a
  product dependency or transferring ownership of product assets.

- **Analyzer mechanical determinism.** No LLM, no network, no clock,
  no randomness in output. Identical `(inputs, model, config, flags)` with
  `--jobs 1` must produce byte-identical output and exit code. This governs
  every analyzer pass, not just a specific subcommand — a `HashMap` in a render
  or model path is a bug, not a style nit.
- **`eq` soundness is sacred.** `eq`/`diff` must never
  wrongly commit `EQUIVALENT` or `NOT_EQUIVALENT`; "don't know" always maps to
  `Indecisive`, never a guess.

## Glossary

None yet — define domain terms here when they clarify the code's vocabulary.
