# Domain Model

The evolving statement of **what PureCARD is and does**. It is the elaboration of
the domain section of [`../constitution.md`](../constitution.md): the constitution
states the non-negotiable domain *rules*; this file describes the *entities,
workflows, and invariants* those rules govern. When the two disagree, the
constitution wins.

The authoritative, self-contained specification is [the full spec](spec/README.md);
this file is the navigable model over it. This document is **EVOLVABLE** and grows
one reviewer-approved PR at a time.

## How to use this file

- When a feature introduces a new domain concept, add or update its entry here as
  part of the same PR that adds the code.
- Keep it a **model**, not a changelog. Describe the current truth; history lives
  in git and in [`decisions/`](decisions/).
- No filler. If an entry says nothing a reader couldn't infer from the type names,
  delete it.
- Cross-link the feature spec (`specs/<name>.md`, from `just spec`) and the
  `docs/spec/` section that introduced each concept.

---

## Entities

### GuaranteeLevel

**What it is.** The three nested guarantees a constrained decoder can offer:
`Syntactic` (L1), `SchemaConsistent` (L2), and `Faithful` (L3). PureCARD enforces
up to L2 and refuses L3. Implemented in `src/lib.rs`.

**Invariants.** The levels form a strict containment hierarchy —
`Faithful ⊂ SchemaConsistent ⊂ Syntactic`. A stronger guarantee implies every
weaker one. `Faithful` is never enforceable at decode time (the mask never sees
the question's intent).

**Relationships.** Every other entity exists to move a model's output up this
hierarchy: the grammar/PDA delivers L1; the schema overlay delivers L2.

**Introduced by.** [`spec/overview.md`](spec/overview.md) §1. *(The entities below
are shipped: all milestones M0–M5 are merged, so each entry describes the decoder
as built — `src/` is authoritative where a detail is load-bearing.)*

### Vocab

**What it is.** The model vocabulary as raw byte strings per token id, indexed
directly by token id — there is **no** trie (`src/vocab.rs`: `bytes(id)` is a
direct table index; per-state acceptance is resolved by probing the byte-level PDA
on first visit to a state, not by a trie walk). A token is admissible iff feeding
its raw bytes advances the byte-level automaton to a non-dead state. This avoids a
trie traversal, but it does **not** eliminate host tokenizer/vocabulary alignment
risk: the host must still supply each token's exact bytes (the §11 tokenizer-exactness
concern), and neither the byte-level replay nor the M5 self-check proves token-id
soundness — that is exercised only live in the M4 e2e lane.

**Introduced by.** [`spec/architecture.md`](spec/architecture.md) §4.1, §4.4, §9.1.

### CompiledGrammar

**What it is.** The L1 context-free skeleton of the *emitted subset* of Pure
(class-anchored relation pipelines), compiled into a pushdown automaton with
per-state context-independent mask caches.

**Invariants.** The grammar is derived from, and testable against, the gold
corpus: any production a gold query violates is wrong and must be relaxed; any
construct the corpus lacks stays out until a gold query adds it (oracle-driven,
never speculative).

**Introduced by.** [`spec/architecture.md`](spec/architecture.md) §3, §4 and [`spec/grammar.md`](spec/grammar.md) §5.

### Schema

**What it is.** The per-database structure the L2 overlay consults: classes →
`{property → (type, multiplicity)}`, plus association navigabilities and enums.
Built **host-side** (never by the decoder) from PMCD/MCP and handed to the session
at init as JSON.

**Invariants.** `PropType` is a three-way split (`Primitive` | `ClassRef` |
`EnumRef`) — load-bearing, because it decides whether `.` after a property
continues navigation, terminates a value, or narrows an enum. Association
navigability is directional (§6.2.3): getting the direction backwards is a
soundness bug.

**Introduced by.** [`spec/schema.md`](spec/schema.md) §6.2.

### Scope

**What it is.** The typed-scope state threaded through the parse: `ClassScope`
(a row of a class) or `RelationScope` (a TDS/relation with named columns). The top
of the scope stack determines which identifiers L2 admits.

**Shipped (M3).** `Schema::from_json` + `DecoderSession::with_schema` deliver L2 as
`src/schema/{model, scope, narrow}`. The scope machine (`ScopeTracker`) advances in
lockstep with `accept_token`, and `allowed_mask` intersects the L1 mask with the
schema-legal set. The shipped rules are N3 (source is a real class **or** the store
`db_path`, per `Schema::is_source`), N1/N2 (member/nav after `.`), **N5**
(association-direction narrowing — it ships folded into N1, since
`Schema::member_names`/`resolve` admit only the navigable, correct-direction
association ends, so a wrong-direction navigation is already masked by N1's member
set), N6 (relation-column strings), and T1 (comparison operand type-class — only
its string/numeric levers; Boolean/Temporal operand narrowing passes through);
deferred are T2/T3/T4/T6/T7 and the inert N4/T5 (see the module docs and
`specs/m3-schema-overlay.md`). Soundness holds on all 269
fixture-backed gold queries; the load-bearing narrowing surface is the 13 arm-C
queries (arm-A exercises only N6 + a table-exists check).

**Introduced by.** [`spec/schema.md`](spec/schema.md) §6.4.

### DecoderSession

**What it is.** The per-generation object holding PDA state, stack, and scope over
a `CompiledGrammar`. Its surface: `allowed_mask()`, `accept_token()`,
`is_complete()`, `reset()`. The `#[cfg(feature = "python")]` PyO3 bindings wrap
exactly this.

**Invariants (M5).** `is_complete()` is true iff **every frame is closed AND the
last token is fully lexed at a value boundary** — derived from `step`, so it
covers every value-terminal state (a trailing top-level identifier now completes),
while a bare `|X` source deliberately does not. `accept_token` distinguishes an
**out-of-range id** (`UnknownToken` — a host-contract violation) from an in-range,
mask-respecting reject (`InadmissibleToken`), so a host can tell its own bug from
routine masking; both leave the session untouched (§8.5 rollback).

**Introduced by.** [`spec/architecture.md`](spec/architecture.md) §9.

### Tokenizer self-check

**What it is.** An opt-in, side-effect-free proof that a host-supplied `Vocab`
can *express* grammar-legal queries — the guard against the invisible-soundness
risk (overview §11) that a host's token → bytes disagreeing with the model's real
tokenization silently breaks masking. `self_check`/`self_check_smoke` drive
canonical queries *through tokens* (longest-match segmentation, then per-segment
mask + accept, then `is_complete`), failing loud via a distinct `SelfCheckError`
(never a `DecodeError`). Implemented in `src/selfcheck.rs`; no corpus is compiled
into the core (the ~4-query `SMOKE` set is inline), and the full 5034-query
round-trip is `tests/selfcheck_corpus.rs`.

**Invariants.** A grammar-legal query the host vocab cannot segment, that
dead-ends, or that never completes proves host-vocab vs model-tokenizer drift.
The check is pure (const `&[u8]` samples, byte-slice matching, no I/O) and adds
zero core dependencies.

**Introduced by.** [`spec/overview.md`](spec/overview.md) §11; M5
(`specs/m5-hardening.md`).

## Workflows

### Compile a grammar (once per model + grammar)

`CompiledGrammar::compile(vocab)` (or the stub `from_spec(spec, vocab)`) binds the
vocab and **sizes** the lazy per-state mask cache — it probes no token up front;
each state's partition is built on first visit (`cached(state)`).
[`spec/architecture.md`](spec/architecture.md) §4.5, §9.1.

### Constrain one generation (per decode step)

`mask = cache[state] ∩ runtime-stack-check ∩ schema-narrow(scope)`; the host
`&`-masks the logits, samples, and calls `accept_token`; stop when `is_complete()`
and EOS is sampled. Constraint applies only to the **final-query span** of a
trajectory. [`spec/architecture.md`](spec/architecture.md) §3.3, §4.3, §9.3.

## Cross-cutting invariants

- **Soundness (the killer property).** The mask must never forbid a token that a
  verified gold query actually emits. Replaying the 5,034-query gold corpus and
  asserting every next token is in `allowed_mask()` is the always-on gate
  ([`spec/testing.md`](spec/testing.md) §8.1) — and it runs offline, with no Legend engine.
- **L2 only narrows, never widens.** The schema overlay intersects L1's terminal
  set; it can only remove admissible tokens, never add them ([`spec/architecture.md`](spec/architecture.md) §3.1).
- **Completeness.** Constrained generations must compile against the real Legend
  engine; a compile failure is a grammar/overlay gap to tighten oracle-driven
  ([`spec/testing.md`](spec/testing.md) §8.2).

## Glossary

- **L1 / L2 / L3** — syntactic / schema-consistent / faithful; see
  [`GuaranteeLevel`](#guaranteelevel).
- **Emitted subset** — the fragment of Pure the trained model actually produces
  (class-anchored relation pipelines); the grammar recognizes only this.
- **PDA** — the byte-level pushdown automaton implementing L1.
- **PMCD** — PureModelContextData, the Legend model structure the host turns into
  a `Schema`.
- **Soundness / completeness** — never mask a valid continuation / never lead the
  model into a dead end. Both are mechanically testable against the corpus and the
  engine oracle.
- **Faithfulness** — the query means what was asked. Out of scope; PureCARD never
  claims it.
