# PureCARD Spec — Overview

_[Spec index](README.md) · [domain model](../domain-model.md)_

This file covers the interface and guarantee boundary (§1), scope and non-goals
(§2), the build milestones (§10), risks (§11), the roadmap (§12), and prior art
(Appendix B). The grammar, schema, architecture, and testing rules live in the
other [`docs/spec/`](README.md) files — see the [index](README.md) to route.

## 1. What PureCARD is — the interface and the guarantee boundary

### 1.1 What PICARD is (background the reader will not have)

**PICARD** (Scholak, Schucher, Bahdanau — _"Parsing Incrementally for Constrained Auto-Regressive Decoding from Language Models,"_ EMNLP 2021) is the original constrained decoder for text-to-SQL. Its central idea: an autoregressive language model, at each decode step, proposes a probability distribution over its whole vocabulary; PICARD sits **between the model's logits and the sampler** and rejects any next-token that would put the partial output on a path with no valid completion. The model's weights are **frozen** — PICARD is **inference-only** and **model-agnostic**: it does not fine-tune, it does not know the model's internals, it only reads the tokens generated so far and returns a decision about which next-tokens are still admissible.

The conceptual interface is a per-step logits transform:

```
mask(grammar_state, schema, logits) -> logits'
```

where `logits'` sets the logit of every inadmissible token to −∞ (so the sampler can never pick it), leaving admissible logits untouched. Output is valid **by construction**.

PICARD defines **three tiers** of checking, applied incrementally as text is generated:

1. **Lexical** — the emitted tokens form valid lexemes of the target language.
2. **Grammatical (syntactic)** — the partial output parses as the target grammar. _Schema-independent._
3. **Schema-consistency** — the identifiers and types resolve against _this specific database's_ schema: no phantom tables/columns, no type mismatches. _Per-database, context-sensitive._

A hard problem PICARD solves is **BPE↔target-token misalignment**: the language model's subword (BPE) tokens do not align with the target language's lexical tokens — a single BPE token can straddle a keyword boundary, and a target keyword can span several BPE tokens. PICARD handles this by **incremental parsing**: it feeds generated text through the parser piece by piece and checks reachability of a valid parse. PureCARD solves the same problem more simply, at the **byte level** (§4.4): it treats every model token as an opaque raw byte string and asks only whether feeding those bytes advances a byte-level automaton to a non-dead state, which sidesteps subword-boundary alignment entirely.

### 1.2 PureCARD, mechanically

PureCARD is a **logits mask generator** driven by an incremental recognizer for a restricted subset of **Legend Pure** (the functional query/modeling language of the FINOS Legend platform). At every decode step the model proposes a distribution over its vocabulary (~150k tokens); PureCARD, given the tokens generated so far, returns a boolean bitmask over the vocabulary marking the tokens that keep the partial output on a path to a valid Pure query. The Python inference loop applies the mask (sets disallowed logits to −∞) before sampling. Output is valid by construction.

Two constraint levels are both in scope; a third is explicitly out of scope:

| Level                      | Guarantees                                                                                  | In scope                    |
| -------------------------- | ------------------------------------------------------------------------------------------- | --------------------------- |
| **L1 — syntactic**         | the output parses as (emitted-subset) Pure                                                  | ✅ core                      |
| **L2 — schema-consistent** | identifiers/types resolve against _this_ model — no phantom classes/props, no type mismatch | ✅ overlay                   |
| L3 — faithful              | the query answers the question that was asked                                               | ❌ impossible at decode time |

### 1.3 The guarantee boundary (the single most important scoping fact)

PureCARD guarantees **validity** (L1: the query parses) and **schema-consistency** (L2: the query _compiles against this model_ — every identifier resolves and every operation type-checks). It does **NOT** and **CANNOT** guarantee **faithfulness** — that the query _means what was asked_.

The three levels form a strict containment hierarchy:

```
                 faithful  ⊂  schema-consistent  ⊂  syntactic
        (answers the Q)      (compiles on model)     (parses)
        L3 — out of scope    L2 — in scope           L1 — in scope
```

Read the containment right-to-left: every faithful query is schema-consistent, and every schema-consistent query is syntactic — but not vice versa. PureCARD moves the output from "arbitrary text" into the _syntactic_ set (L1) and, with a schema, into the _schema-consistent_ set (L2). It **cannot** move it into the _faithful_ set.

Why faithfulness is structurally unreachable at decode time: the mask sees the schema and the partial output string, but **never the question's intent**. Consider a database with a `Singer` class. Both

```pure
Singer.all()->filter(x|$x.country == 'France')
Singer.all()->filter(x|$x.name    == 'France')
```

are perfectly schema-consistent — `country` and `name` are both real String properties. L2 narrows `$x.` to the real member set `{singerId, name, country, songName, songReleaseYear, age, isMale}`; **every** member is a legal next-token, and L2 has no basis to prefer `country` over `name`. Only the model's own probability mass — shaped by training and by the in-context question — picks the faithful column. L2 only guarantees the model cannot pick a _non-existent_ or _mistyped_ column.

**False-confidence risk to state prominently.** Because L2 output _always compiles_, a downstream reader may over-trust it. A query can be 100% schema-consistent and 100% wrong (wrong column, wrong join, wrong aggregate). L2 narrows the _error surface_ from {syntax errors ∪ phantom-reference errors ∪ type errors ∪ wrong-answer errors} down to {wrong-answer errors} — it does **not** shrink the wrong-answer class at all, and may enlarge it at the margin (see the over-constraint caveat in §11). Any evaluation must keep measuring execution-equivalence (faithfulness) with the constraint ON; a rise in "compiles but wrong" under L2 is the signal to watch.

---

## 2. Scope and non-goals

**In scope:** a single Rust crate that compiles the emitted-Pure grammar into a byte-level pushdown automaton, computes per-step logits masks efficiently, optionally narrows those masks with a schema-consistency overlay, and exposes the whole thing to Python over a thin PyO3 boundary — plus the oracle-driven test harness that proves it correct.

**Non-goals (keep the component small):**

- **Not** a full Pure parser/compiler. Only the _emitted subset_ the trained model actually produces (class-anchored relation pipelines) needs to be recognized, and only far enough to mask next-tokens.
- **Not** faithfulness, ranking, or repair. It prunes invalid branches; it does not choose the right valid one.
- **Not** the training pipeline, the Python inference stack, tokenizer training, or general Rust project scaffolding. Only the decoder crate and its PyO3 boundary.
- **Not** trajectory constraint. The model emits full agentic trajectories (tool calls, reasoning, then the final query); PureCARD constrains **only the final-query span** — the Python loop activates it when that span begins (integration assumption, §9).
- **Not** full Pure syntax. The grammar is a deliberate over-approximation of validity in a few places (§5.6); the Legend compiler oracle (§8) catches escapes and drives tightening. Do not gold-plate — keep it minimal to stay fast and sound.
- **Not** runtime data values. L2 never constrains literal _values_ (only their _types_), because any type-valid literal compiles.

---

## 10. Build milestones (M0–M5)

- **M0 — skeleton + oracle harness.** Crate, `Vocab` ingestion, byte-PDA infrastructure, and the §8.1 soundness harness wired to the gold corpus + the §8.2 differential compile test against the live Legend engine. Test-first: the corpus and compiler are the spec.
  - _Done when:_ the harness can replay a gold query through a stub decoder and can POST a query to `/pure/v1/compilation/lambdaReturnType` and read the result.

- **M1 — L1 grammar.** Implement §5; pass **100% gold-corpus soundness** and **100% constrained-walk compile rate**. No perf work yet.
  - _Done when:_ every gold query replays with zero masked-gold-token failures, and random accepting PDA walks all compile.

- **M2 — performance.** Context-independent per-state mask cache (§4); hit the per-token latency target (≤ a few hundred µs/token); benchmark.
  - _Done when:_ mask generation is off the critical path against the model's forward pass, with a benchmark to prove it.

- **M3 — L2 schema overlay.** Scope/type tracker + schema-narrowed terminals (§6); pass schema soundness on the gold corpus with real schemas + **zero phantom/type-mismatch** under L2.
  - _Done when:_ §8.1 (L2 mode) and §8.3 both green on a held-out schema set.

- **M4 — PyO3 boundary.** §9 bindings + a reference Python harness driving a real small model to produce compilable Pure end-to-end under constraint.
  - _Done when:_ a Python loop generates constrained Pure from a real model and it compiles.

- **M5 — hardening.** Tokenizer self-check (round-trip a sample of gold queries through tokenize→bytes→decoder at startup), incomplete-generation handling, error recovery, fuzzing, final benchmarks.

**Definition of done:** L1 shippable after M2 (guaranteed-syntactic single-shot Pure); L2 shippable after M3 (guaranteed schema-consistent); the CI gate in §8.7 green on a held-out schema set.

---

## 11. Risks and open questions

- **Grammar drift.** The emitted subset co-evolves with the trained model; a query shape the model emits but the grammar rejects is a soundness failure. _Mitigation:_ the gold-corpus soundness test (§8.1) runs against the _current_ model's outputs and fails loudly on drift; treat the grammar spec as versioned alongside model checkpoints.
- **`map`'s argument grammar (L1-internal gap).** `map` (6 gold, §5.7) is currently listed as "treat as `fn`", but it takes a lambda argument that `fnArgs` (`valueExpr { "," valueExpr }`) cannot accept — an unresolved L1-internal completeness gap. Whether `map`'s argument production should admit a lambda (vs a value) is left open pending a corpus decision; the production is deliberately not guessed here.
- **L2 context-dependent set size.** If schema narrowing touches too many token positions, the runtime (non-cached) fraction grows and perf degrades. _Mitigation:_ narrow only at identifier/type positions; cache per-(state, class-scope) identifier masks (§4.5).
- **Tokenizer exactness.** Any mismatch between the host's byte representation of tokens and the model's actual tokenization breaks soundness invisibly. _Mitigation:_ a startup self-check that round-trips a sample of gold queries through tokenize→bytes→decoder (M5).
- **Possible redundancy.** The agentic schema-exploration path (the model calls `legend_describe_class` / `legend_get_associations` _before_ writing the query) may already suppress name hallucination enough that L2's marginal value is small; L1 (cheap) is the safe first target, L2 is gated on measured post-training schema-reference error. Build L1 fully; build L2 only when the measurement justifies it — but this doc specs both so the agent can proceed straight to L2 if the trigger is already met.
- **Over-constraint vs faithfulness.** Masking can force a valid-but-wrong token the model would not otherwise pick: if the model was about to emit a phantom name that (after repair) it would have corrected toward the _faithful_ name, hard-masking may instead push it to a _different real-but-wrong_ name. L2 trades "compiles never" for "compiles always, sometimes wrongly." This is inherent to constrained decoding and out of scope to solve here; flag it so host-side evaluation watches for faithfulness regressions when the constraint is enabled.
- **False confidence (restated from §1.3).** Because L2 output always compiles, downstream readers may over-trust it. Keep measuring execution-equivalence with the constraint ON; a rise in "compiles but wrong" under L2 is the signal to watch.

---

## 12. Roadmap position and build triggers

PureCARD is an **inference-time serving optimization**, not an urgent-blocking dependency. It exists to deliver _guaranteed-valid Pure in a single forward pass_ at serving time, removing the compile-repair round-trip. Its place in the roadmap:

**Build gate.** PureCARD is gated on the conjunction of:

1. a trained model exists (that emits Pure), AND
2. single-shot serving is committed (as opposed to compile-and-repair loops), AND
3. measured schema-reference errors are still material after the cheap L1 version.

**Build order.**

- **Build L1 first.** It is cheap, schema-independent, and delivers the syntactic guarantee with no schema plumbing. Ship after M2.
- **Escalate to L2 only if** name-hallucination (phantom-identifier / type-mismatch) errors _specifically_ dominate the residual error after L1 + agentic schema-exploration. Measure before over-building — L2 may be partly redundant with the model's own agentic schema-exploration, so its marginal value must be demonstrated, not assumed. This spec exists so that, _if_ the trigger fires, the L2 rules are ready to implement — not as a mandate to build L2 unconditionally.

**One-line placement.** PureCARD is the durable Rust serving kernel that turns a trained Pure-emitting model's final-query span from "probably valid" into "valid by construction (L1), and — when the measurement justifies it — schema-consistent by construction (L2)," while never claiming to make it _faithful_.

---

## Appendix B — Prior art / references (for the implementer)

- **PICARD** (Scholak, Schucher, Bahdanau, _"Parsing Incrementally for Constrained Auto-Regressive Decoding from Language Models,"_ EMNLP 2021) — the original SQL constrained decoder; incremental parsing + lexical/grammatical/schema-consistency tiers. PureCARD is its Pure analogue.
- **xgrammar** — Rust-cored grammar-constrained decoding; the context-independent/context-dependent token-mask partition and per-state caching (§4) follow its approach.
- **llama.cpp GBNF** and **Outlines** — grammar/regex-constrained decoding designs; useful references for byte-level automaton masking (§4.4).
- **Legend / Pure** — the FINOS Legend platform; the compile oracle is engine 4.113.0 at `http://localhost:6300/api`, endpoint `/pure/v1/compilation/lambdaReturnType` (§8.2).
