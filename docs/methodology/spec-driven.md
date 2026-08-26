# Methodology: Spec-Driven Development

The repository's two products remain domain-agnostic with respect to customer
data, but their product contracts are no longer empty. New behavior still enters
deliberately and is written down before code. That entry happens in three root
governance locations, with PureCARD's nested product specs supplying additional
decoder-specific detail.

## Where the "what" lives

1. **[`constitution.md`](../../constitution.md)** — the non-negotiable domain
   rules. Stable, high-level, PROTECTED where it needs to be. The invariants that
   hold across *every* feature.
2. **`specs/<name>.md`** — the per-feature spec. The concrete "what" for one
   change: the behavior, the acceptance criteria, the edge cases.
3. **[`domain-model.md`](../domain-model.md)** — the evolving elaboration of the
   domain: entities, workflows, invariants, vocabulary. Grows one feature at a
   time.

The analyzer target design lives in
[`docs/design/pure-analyzer-design.md`](../design/pure-analyzer-design.md), while
PureCARD owns its nested [`docs/spec/`](../../crates/pure-analyzer-purecard/docs/spec/).
Neither document makes the products one architecture.

Code is downstream of all three. The generator implements *to* them; the reviewer
checks *against* them.

## The `/spec` flow

A feature moves through three phases, driven by `/spec` and scaffolded by
`just spec <name>`:

### plan

Turn the spec into an approach *before* writing code. Read the constitution and
the current domain model, identify the entities and invariants involved, decide
which product or shared-infrastructure surface owns each piece, and name the
tests that will prove it. Analyzer work follows the processing pipeline `lexer
→ syntax → parser → model → resolve → analysis → libpure → cli`
(see ADR-0003); Cargo edges point toward prerequisites, so resolve may depend on
model and the reverse is forbidden. PureCARD is not a node in that DAG. Any
proposal to share a parser or corpus across products must include a new ADR.
Surface open questions here, not mid-implementation.

### implement

Write the code and tests against the plan, in a dedicated worktree, obeying the
constitution. The domain model and lessons are updated *in the same change* when
the work teaches us something new about the "what."

### verify

Run the change through `just ci` (the fast local gate — see
[testing.md](testing.md); the heavier coverage/mutation/audit gates run as
separate CI jobs), then the reviewer. The **reviewer checks the diff against the spec**: does the code
do what the spec said, no less and no more? Scope creep, missing acceptance
criteria, and unrequested behavior are all review findings. A change that passes
its tests but drifts from its spec does not merge.

## Writing a spec

A good spec is short and testable. It should state:

- **Goal** — what capability this adds, in one or two sentences.
- **Behavior** — the observable contract: inputs, outputs, and the CLI/LSP
  surface touched (subcommand, flags, exit code; LSP method, once v0.2 lands).
- **Acceptance criteria** — a checklist the reviewer and the tests can both
  evaluate. If a criterion can't be turned into a test, sharpen it until it can.
- **Invariants touched** — which domain-model invariants this relies on or
  introduces (and thus what must be enforced in the engine crates' types).
- **Out of scope** — what this change deliberately does *not* do, so "not done" is
  distinguished from "not intended."
- **Open questions** — anything needing a human or a decision (possibly an ADR).

No filler. A spec that restates the obvious wastes the reviewer's attention, which
is the scarcest thing in the loop.

## Why spec-first

Four payoffs, each central to the methodology:

- **The reviewer gets an oracle.** "Does the diff match the spec?" is a far
  sharper question than "is this good?" — it turns review from taste into
  verification.
- **Scope stays honest.** With "out of scope" written down, the fold-vs-branch
  call on pre-existing issues has a reference point, and scope creep is visible.
- **The domain accumulates.** Specs are the mechanism by which
  `domain-model.md` and the constitution's domain section grow — deliberately,
  reviewably, one feature at a time — instead of the "what" living only in the
  agent's head for one session.
- **Product ownership stays reviewable.** A spec names whether work belongs to
  the analyzer, PureCARD, or shared repository infrastructure, so co-location
  cannot silently turn into runtime coupling or shared corpus ownership.
