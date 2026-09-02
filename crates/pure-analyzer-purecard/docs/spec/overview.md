# PureCARD Spec — Overview

_[Spec index](README.md) · [domain model](../domain-model.md)_

This file covers the interface and guarantee boundary (§1), scope and non-goals
(§2), operating limits (§10), and prior art (Appendix B). The grammar, schema,
architecture, and testing rules live in the other [`docs/spec/`](README.md)
files — see the [index](README.md) to route.

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
output inside its hand-written emitted-subset recognizer. A host inference loop
applies the mask (sets disallowed logits to −∞) before sampling. PureCARD does
not include that loop or a full Legend compiler, so it is not a general
compiler-validity guarantee.

Two constraint levels are represented; a third is structurally out of scope:

| Level                      | Boundary                                                                                    | PureCARD behavior                                             |
| -------------------------- | ------------------------------------------------------------------------------------------- | ------------------------------------------------------------- |
| **L1 — syntactic**         | output parses as emitted-subset Pure                                                        | fixed hand-written emitted-subset recognizer                  |
| **L2 — schema-consistent** | identifiers/types resolve against _this_ model — no phantom classes/props, no type mismatch | schema overlay at covered positions                           |
| L3 — faithful              | query answers the question that was asked                                                   | structurally unavailable to a decode-time mask                |

### 1.3 The guarantee boundary

PureCARD preserves reachability in its hand-written emitted-subset recognizer
and narrows selected class/property positions against a supplied schema. It is
not a full Pure compiler or type checker, and it does **NOT** and **CANNOT**
guarantee **faithfulness** — that a query means what was asked.

The three levels form a strict containment hierarchy:

```
                 faithful  ⊂  schema-consistent  ⊂  syntactic
        (answers the Q)      (compiles on model)     (parses)
        L3 — out of scope    L2 — in scope           L1 — in scope
```

Read the conceptual containment right-to-left: every faithful query is
schema-consistent, and every schema-consistent query is syntactic — but not vice
versa. PureCARD enforces the emitted-subset syntactic set (L1) and, when given a
schema, applies its partial L2 overlay. It **cannot** move output into the
_faithful_ set.

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
implemented overlay can block a non-existent member at covered positions; it
does not guarantee full name resolution or type-checking across the query.

**False-confidence risk.** A compiling query can still be 100% wrong (wrong
column, wrong join, wrong aggregate). Schema constraints can narrow syntax,
phantom-reference, and type-error surfaces; they cannot shrink the wrong-answer
class and may enlarge it at the margin. Measuring that class needs
execution-equivalence against real data, which nothing in this product
measures. The closest lane, `tests/real_model_legend_compile.rs`, reports
compile success plus **return-type** faithfulness against a hand-authored gold
reference, over a store that seeds no rows (`docs/spec/testing.md` §13), and it
runs only on the opt-in self-hosted lane described there — never per PR.

---

## 2. Scope and non-goals

**In scope:** the independent `purecard` Cargo package, whose Rust library
selects the emitted-Pure byte-level pushdown automaton,
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
  validity in a few places (§5.6); the Legend compiler oracle (§8) classifies
  escapes. Do not gold-plate — keep the emitted subset minimal.
- **Not** runtime data values. L2 never constrains literal _values_ (only their _types_), because any type-valid literal compiles.
- **Not** an analyzer subsystem. PureCARD and Pure Analyzer are sibling products
  with no dependency edges in either direction; only root governance and
  orchestration are shared ([ADR-0009](../decisions/0009-monorepo-placement.md)).

---

## 10. Operating limits

PureCARD exposes a fixed hand-written emitted-subset PDA, an optional L2 schema
overlay at its covered positions, and a PyO3 masking boundary. Those are not a
full Pure compiler, a full type checker, or a model-inference runner. A host may
instead supply its own grammar through `CompiledGrammar::from_spec`, which
validates and lowers it (ADR-0010) into a bounded, data-driven automaton with
L1 syntactic recognition only — the L2 schema overlay stays fixed-PDA-specific,
and accepting-walk generation is schema-agnostic.
The frozen corpus includes `map` (6 gold records), so the fixed grammar includes
that emitted pipeline step.

The real-Qwen lane (`tests/qwen_soundness.rs`) verifies token-ID replay against
the pinned tokenizer. It does not establish behavior of a host inference loop or
compiler validity of every accepted query. PureCARD also cannot establish
semantic faithfulness because a decoder mask does not observe user intent.

---

## 12. Repository position

PureCARD is an inference-time serving component, not a dependency of the Pure
Analyzer engine. It is colocated so both products share root toolchain, CI, and
governance while retaining independent internals and release posture. Any
cross-product parser, model, or corpus sharing requires a GitHub Issue and an
ADR that revises the product boundary.

---

## Appendix B — Prior art / references (for the implementer)

- **PICARD** (Scholak, Schucher, Bahdanau, _"Parsing Incrementally for Constrained Auto-Regressive Decoding from Language Models,"_ EMNLP 2021) — the original SQL constrained decoder; incremental parsing + lexical/grammatical/schema-consistency tiers. PureCARD is its Pure analogue.
- **xgrammar** — Rust-cored grammar-constrained decoding; the context-independent/context-dependent token-mask partition and per-state caching (§4) follow its approach.
- **llama.cpp GBNF** and **Outlines** — grammar/regex-constrained decoding designs; useful references for byte-level automaton masking (§4.4).
- **Legend / Pure** — the FINOS Legend platform; the compile oracle is engine 4.113.0 at `http://localhost:6300/api`, endpoint `/pure/v1/compilation/lambdaReturnType` (§8.2).
