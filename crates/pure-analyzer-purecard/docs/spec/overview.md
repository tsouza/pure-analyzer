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

PureCARD is a **logits mask generator** driven by an incremental recognizer
for a restricted subset of **Legend Pure** (the functional query/modeling
language of the FINOS Legend platform). At every decode step the model proposes
a distribution over its vocabulary (~150k tokens); PureCARD, given the tokens
generated so far, returns a boolean bitmask marking tokens that keep the partial
output inside its hand-written emitted-subset recognizer. The Python inference
loop is designed to apply the mask (sets disallowed logits to −∞) before
sampling. That loop and live Legend compilation are not yet exercised end to
end, so the current implementation does not guarantee compiler-valid output in
every case.

Two constraint levels are product targets; a third is explicitly out of scope:

| Level                      | Target boundary                                                                             | Current implementation                                        |
| -------------------------- | ------------------------------------------------------------------------------------------- | ------------------------------------------------------------- |
| **L1 — syntactic**         | output parses as emitted-subset Pure                                                        | hand-written emitted-subset recognizer; live proof pending    |
| **L2 — schema-consistent** | identifiers/types resolve against _this_ model — no phantom classes/props, no type mismatch | partial overlay at selected positions; not full type-checking |
| L3 — faithful              | query answers the question that was asked                                                   | out of scope and impossible to guarantee at decode time       |

### 1.3 The target guarantee boundary and current evidence

The product target is **validity** (L1: the query parses) and
**schema-consistency** (L2: the query compiles against this model). The current
L1 code preserves reachability in the hand-written emitted-subset recognizer,
and the current L2 code narrows selected class/property positions against a
schema fixture. Neither constitutes a full Pure compiler or end-to-end proof:
the obligations listed in §10 remain open, and accepted output is not yet
guaranteed to compile in every case. PureCARD does **NOT** and **CANNOT**
guarantee **faithfulness** — that the query means what was asked.

The three levels form a strict containment hierarchy:

```
                 faithful  ⊂  schema-consistent  ⊂  syntactic
        (answers the Q)      (compiles on model)     (parses)
        L3 — out of scope    L2 — in scope           L1 — in scope
```

Read the conceptual containment right-to-left: every faithful query is
schema-consistent, and every schema-consistent query is syntactic — but not vice
versa. PureCARD is intended to move output from "arbitrary text" into the
emitted-subset syntactic set (L1) and, with a complete schema overlay, into the
schema-consistent set (L2). Today it enforces the L1 recognizer and a partial L2
overlay; it **cannot** move output into the _faithful_ set.

Why faithfulness is structurally unreachable at decode time: the mask sees the schema and the partial output string, but **never the question's intent**. Consider a database with a `Singer` class. Both

```pure
Singer.all()->filter(x|$x.country == 'France')
Singer.all()->filter(x|$x.name    == 'France')
```

are perfectly schema-consistent — `country` and `name` are both real String
properties. At a covered property position, the L2 overlay narrows `$x.` to the
real member set `{singerId, name, country, songName, songReleaseYear, age,
isMale}`; **every** member remains a legal next-token, and L2 has no basis to
prefer `country` over `name`. Only the model's own probability mass — shaped by
training and by the in-context question — picks the faithful column. The
implemented overlay can block a non-existent member at those covered positions;
it does not yet guarantee full name resolution or type-checking across the
query.

**False-confidence risk to state prominently.** Even if the complete L2 target
is eventually proven, a compiling query can still be 100% wrong (wrong column,
wrong join, wrong aggregate). A complete L2 constraint would narrow the _error
surface_ from {syntax errors ∪ phantom-reference errors ∪ type errors ∪
wrong-answer errors} down to {wrong-answer errors}; it would not shrink the
wrong-answer class and may enlarge it at the margin (see the over-constraint
caveat in §11). The current partial overlay does not establish that full
compilation boundary. Evaluation must keep measuring execution-equivalence
(faithfulness) with the constraint ON.

---

## 2. Scope and non-goals

**In scope:** the independent `pure-analyzer-purecard` Cargo package, whose
`purecard` Rust library selects the emitted-Pure byte-level pushdown automaton,
computes per-step logits masks efficiently, optionally narrows those masks with
the implemented schema-consistency overlay, and exposes the surface to Python
over a thin PyO3 boundary — plus the oracle-driven test harness that measures it.

**Non-goals (keep the component small):**

- **Not** a full Pure parser/compiler. Only the _emitted subset_ the trained model actually produces (class-anchored relation pipelines) needs to be recognized, and only far enough to mask next-tokens.
- **Not** faithfulness, ranking, or repair. It prunes branches rejected by the
  fixed PDA and the implemented schema rules; it does not choose the right
  surviving branch.
- **Not** the training pipeline, the Python inference stack, tokenizer training, or general Rust project scaffolding. Only the decoder crate and its PyO3 boundary.
- **Not** trajectory constraint. The model emits full agentic trajectories (tool calls, reasoning, then the final query); PureCARD constrains **only the final-query span** — the Python loop activates it when that span begins (integration assumption, §9).
- **Not** full Pure syntax. The grammar is a deliberate over-approximation of
  validity in a few places (§5.6); the Legend compiler oracle (§8) is intended
  to catch escapes and drive tightening. Do not gold-plate — keep it minimal
  and use the outstanding live proof to establish soundness.
- **Not** runtime data values. L2 never constrains literal _values_ (only their _types_), because any type-valid literal compiles.
- **Not** an analyzer subsystem. PureCARD and Pure Analyzer are sibling products
  with no dependency edges in either direction; only root governance and
  orchestration are shared ([ADR-0009](../decisions/0009-monorepo-placement.md)).

---

## 10. Milestone implementation status (M0–M5)

The milestone labels now describe implemented code slices, not a claim that every
original end-to-end acceptance criterion is green:

- **M0 — oracle/corpus harness implemented.** The committed corpus loader,
  byte-level replay, Legend client boundary, and classified live response path
  exist.
- **M1 — emitted-subset L1 PDA implemented.** The hand-written PDA admits the
  frozen gold and modern-dialect seed corpora. Hermetic accepting walks exercise
  recognizer liveness. The earlier `map` (6 gold records) grammar gap is resolved
  in the fixed PDA. Live validation that 100% of those walks compile remains
  open.
- **M2 — performance layer implemented.** Lazy per-state masks, cache-equivalence
  tests, and criterion benchmarks exist.
- **M3 — schema-overlay subset implemented.** The shipped N/T subset narrows L1
  against committed schema fixtures. Full schema-constrained accepting-walk
  generation and live zero-error validation remain open.
- **M4 — PyO3 boundary implemented.** The feature-gated module and maturin wheel
  build exist. Wheels are verification artifacts (`publish = false`), and no
  real-model Python inference-to-Legend test is implemented.
- **M5 — hardening implemented.** Tokenizer self-check, EOS/finalization and
  error hardening, fuzz targets, and final benchmark coverage exist.

The real-Qwen oracle (`tests/qwen_soundness.rs`) additionally tokenizes with the
actual pinned Qwen tokenizer and replays real token IDs on-demand and on a
schedule. It proves real-tokenizer token-ID replay, not real-model inference or
live Legend compilation.

The remaining proof obligations are:

1. validate a 100% constrained-walk compile rate against live Legend;
2. lower a supplied grammar spec into the PDA (`from_spec` currently selects the
   fixed hand-written machine);
3. generate accepting walks under the supplied schema; and
4. drive real-model Python inference through constraint and live Legend
   compilation.

Until those land, PureCARD is not described as feature-complete and the original
milestone definitions are not treated as proven end to end.

---

## 11. Risks and open questions

- **Grammar drift.** The emitted subset co-evolves with the trained model; a query shape the model emits but the grammar rejects is a soundness failure. _Mitigation:_ the gold-corpus soundness test (§8.1) runs against the _current_ model's outputs and fails loudly on drift; treat the grammar spec as versioned alongside model checkpoints.
- **Grammar-spec lowering.** `CompiledGrammar::from_spec` accepts a spec string
  for API compatibility but currently selects the fixed hand-written PDA. A
  real spec-to-PDA lowering pipeline remains outstanding.
- **Live completeness evidence.** The Legend lane reaches the pinned engine and
  classifies responses, but placeholder protocol fixtures mean it does not yet
  prove a 100% constrained-walk compile rate.
- **Schema-aware generation.** L2 can narrow token masks during replay, but the
  accepting-walk generator does not yet construct walks under a schema.
- **Real-model integration.** The PyO3 surface and wheel build are implemented;
  a Python harness that masks real-model logits and compiles the resulting query
  against live Legend is not.
- **L2 context-dependent set size.** If schema narrowing touches too many token positions, the runtime (non-cached) fraction grows and perf degrades. _Mitigation:_ narrow only at identifier/type positions; cache per-(state, class-scope) identifier masks (§4.5).
- **Tokenizer exactness.** Any mismatch between the host's byte representation of tokens and the model's actual tokenization breaks soundness invisibly. _Mitigation:_ the M5 startup self-check plus scheduled/on-demand real token-ID replay with the pinned Qwen tokenizer. The latter still does not exercise model inference.
- **Possible redundancy.** The agentic schema-exploration path may already
  suppress name hallucination enough that L2's marginal value is small. Prove
  L1 end to end; extend and prove L2 only when measured post-training
  schema-reference errors justify it.
- **Over-constraint vs faithfulness.** Masking can force a valid-but-wrong token
  the model would not otherwise pick. A complete L2 implementation would trade
  compile failures for compiling-but-sometimes-wrong output; the current subset
  does not establish that compile guarantee. Host-side evaluation must watch
  for faithfulness regressions when the constraint is enabled.
- **False confidence (restated from §1.3).** Neither the current partial L2
  overlay nor a future full L2 guarantee establishes faithfulness. Keep
  measuring execution-equivalence with the constraint ON; a rise in "compiles
  but wrong" is the signal to watch.

---

## 12. Roadmap and repository position

PureCARD remains an inference-time serving component, not a dependency of the
Pure Analyzer engine. It is colocated so both products can share root toolchain,
CI, constitution, and methodology while keeping independent internals and
release posture.

The follow-on order is evidence-driven:

1. implement grammar-spec lowering rather than silently ignoring the spec input;
2. extend the walker to generate under a supplied schema;
3. use those outputs to establish the 100% live Legend compile target; and
4. add the real-model Python inference → constrained query → live Legend path.

L2's value should continue to be measured against residual schema-reference
errors and faithfulness with constraints enabled. A compiling query can still be
wrong. Any proposal to share analyzer parser code, model types, or corpora must
first revise the product boundary through a new ADR.

---

## Appendix B — Prior art / references (for the implementer)

- **PICARD** (Scholak, Schucher, Bahdanau, _"Parsing Incrementally for Constrained Auto-Regressive Decoding from Language Models,"_ EMNLP 2021) — the original SQL constrained decoder; incremental parsing + lexical/grammatical/schema-consistency tiers. PureCARD is its Pure analogue.
- **xgrammar** — Rust-cored grammar-constrained decoding; the context-independent/context-dependent token-mask partition and per-state caching (§4) follow its approach.
- **llama.cpp GBNF** and **Outlines** — grammar/regex-constrained decoding designs; useful references for byte-level automaton masking (§4.4).
- **Legend / Pure** — the FINOS Legend platform; the compile oracle is engine 4.113.0 at `http://localhost:6300/api`, endpoint `/pure/v1/compilation/lambdaReturnType` (§8.2).
