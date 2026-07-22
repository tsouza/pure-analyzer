# Methodology: Spec-Driven Development

The kit ships domain-agnostic. Nothing about *what* the system does is assumed, so
the "what" has to enter deliberately and be written down before code is written.
That entry happens in exactly three places, and spec-driven development is how they
connect to a change.

## Where the "what" lives

1. **[`constitution.md`](../../constitution.md)** — the non-negotiable domain
   rules. Stable, high-level, PROTECTED where it needs to be. The invariants that
   hold across *every* feature.
2. **`specs/<name>.md`** — the per-feature spec. The concrete "what" for one
   change: the behavior, the acceptance criteria, the edge cases.
3. **[`domain-model.md`](../domain-model.md)** — the evolving elaboration of the
   domain: entities, workflows, invariants, vocabulary. Grows one feature at a
   time.

Code is downstream of all three. The generator implements *to* them; the reviewer
checks *against* them.

## The `/spec` flow

A feature moves through three phases, driven by `/spec` and scaffolded by
`just spec <name>`:

### plan

Turn the spec into an approach *before* writing code. Read the constitution and
the current domain model, identify the entities and invariants involved, decide
which modules the change touches, and name the tests that will prove it. Surface
open questions here, not mid-implementation.

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
- **Behavior** — the observable contract: inputs, outputs, and the HTTP/gRPC
  surface if any.
- **Acceptance criteria** — a checklist the reviewer and the tests can both
  evaluate. If a criterion can't be turned into a test, sharpen it until it can.
- **Invariants touched** — which domain-model invariants this relies on or
  introduces (and thus what must be enforced in `domain` types).
- **Out of scope** — what this change deliberately does *not* do, so "not done" is
  distinguished from "not intended."
- **Open questions** — anything needing a human or a decision (possibly an ADR).

No filler. A spec that restates the obvious wastes the reviewer's attention, which
is the scarcest thing in the loop.

## Why spec-first

Three payoffs, each central to the kit:

- **The reviewer gets an oracle.** "Does the diff match the spec?" is a far
  sharper question than "is this good?" — it turns review from taste into
  verification.
- **Scope stays honest.** With "out of scope" written down, the fold-vs-branch
  call on pre-existing issues has a reference point, and scope creep is visible.
- **The domain accumulates.** Specs are the mechanism by which
  `domain-model.md` and the constitution's domain section grow — deliberately,
  reviewably, one feature at a time — instead of the "what" living only in the
  agent's head for one session.
