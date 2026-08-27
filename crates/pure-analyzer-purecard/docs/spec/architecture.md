# PureCARD Spec — Architecture

_[Spec index](README.md) · [domain model](../domain-model.md)_

## 3. Architecture

PureCARD is an independent sibling product inside the Pure Analyzer monorepo.
The root workspace, `just`, CI, constitution, and methodology orchestrate it,
but neither product may depend on the other's internals. The Cargo package is
`pure-analyzer-purecard`; the Rust library and Python module are `purecard`
([ADR-0009](../decisions/0009-monorepo-placement.md)).

### 3.1 CFG skeleton (L1) + semantic narrowing (L2)

PureCARD uses a pushdown automaton (PDA) for Pure's context-free shape
(the `->` pipeline, bracket matching, and lambda structure) and a thin
type/scope tracker for context-sensitive decisions such as which property may
follow `$x.`. This mirrors PICARD's lexical/grammatical/schema-consistency tiers,
adapted to Pure: **L1 = the PDA over the emitted grammar; L2 = a typed-scope
overlay that intersects L1's terminal set with the schema-legal set at covered
identifier/type positions.** The shipped PDA is a deliberate emitted-subset
recognizer, and the shipped tracker implements only the N/T positions named in
§6.7; neither is a general Pure compiler.

L2 never _widens_ what L1 allows — it only narrows. `DecoderSession::new(grammar)`
runs the fixed PDA without schema narrowing — useful before a schema is
available and as a fast path. `DecoderSession::with_schema(grammar, schema)`
adds the partial L2 overlay.

### 3.2 Crate layout

Single Cargo package, `pure-analyzer-purecard`, whose Rust library is named
`purecard`, with an optional PyO3 feature exposing bindings. Internal modules
(the shipped layout):

```
src/
  lib.rs          crate root, public exports, and guarantee-level taxonomy
  grammar/        L1: emitted-Pure grammar -> byte-level pushdown automaton (PDA)
    mod.rs          Envelope classifier + DeadState carrier
    pda.rs          hand-written pushdown automaton (states, stack frames, byte transitions)
    compiled.rs     CompiledGrammar: vocabulary + lazy per-state mask cache (the perf core, §4)
    spec.rs         GrammarSpec: versioned, serde-based transition-table schema for a supplied grammar
    compile.rs      CompiledAutomaton/RtnPda: bounded, validated lowering of a GrammarSpec (ADR-0010)
    emitted_subset.rs   pub const EMITTED_SUBSET_SPEC: the shipped grammar as a canonical GrammarSpec JSON asset
  vocab.rs        model vocabulary as raw byte strings per token id
  mask.rs         BitMask: the dense per-step token bitset (§4)
  recognizer.rs   ByteRecognizer trait (the byte-at-a-time surface)
  schema/          partial L2 schema-consistency overlay
    mod.rs          Schema / SchemaError re-exports
    model.rs        Schema { classes -> {prop -> type} }, passed from the host at session init
    scope.rs        lambda scope / type environment tracker (what class is the row var bound to)
    narrow.rs       at implemented positions, restrict terminals to the schema-legal set
    trie.rs         byte-prefix trie: keep a token iff it can extend a legal name (BPE-aware)
  session.rs      DecoderSession: state + stack + scope; accept_token / allowed_mask / is_complete
  selfcheck.rs    tokenizer self-check: vocab round-trip before decode
  error.rs        DecodeError
  ffi.rs          #[cfg(feature="python")] PyO3 bindings (§9)
```

`CompiledGrammar::compile` always builds the fixed, hand-written emitted-Pure
grammar (§5) in `pda.rs`. A host may instead supply its own grammar via
`CompiledGrammar::from_spec`, which parses and validates a versioned
`GrammarSpec` (`spec.rs`) and lowers it into a `CompiledAutomaton` (`compile.rs`)
— a dense, bounded, data-driven automaton, not an EBNF interpreter (ADR-0010).
A spec-compiled grammar supports L1 syntactic recognition only: the L2 schema
overlay (§3.1, `schema/scope.rs`) is implemented against the fixed PDA's named
states and is unavailable for it (`DecoderSession::with_schema` returns
`Err(DecodeError::SchemaRequiresFixedGrammar)`). Masking lives in one `mask.rs`
(there is no `mask/` directory), and the soundness/differential harness lives
under `tests/`, not an in-crate `testing/` module (ADR-0003).

### 3.3 Core data flow (per generation)

```
Python (inference loop)                 Rust (purecard)
─────────────────────────               ───────────────────
build Schema from PMCD/MCP  ──init──▶    DecoderSession::with_schema(compiled_grammar, schema)
loop each decode step:
  logits = model.forward(...)
  mask   = session.allowed_mask()  ◀──   BitMask over vocab (cached + runtime + schema-narrowed)
  logits[!mask] = -inf
  tok    = sample(logits)
  session.accept_token(tok)        ──▶   advance PDA state + stack + scope; err if PDA-illegal
  if session.is_complete() and tok==EOS: break
```

- `allowed_mask` is called every step over the full vocab (~150k tokens) — it must be cheap (§4).
- `accept_token` advances the recognizer (PDA state + stack + scope), erroring
  if the token dead-ends the fixed PDA. It does not re-apply the schema mask;
  honoring `allowed_mask` before sampling is the host contract.
- `is_complete` is true when the fixed PDA is in an accepting state, so the loop
  knows EOS is legal to the recognizer. Even in a schema-enabled session it does
  not validate that the host honored earlier masks, nor is it a general Pure
  compiler or schema-validation result.

---

## 4. The masking algorithm (performance core)

Naive per-token PDA replay at every step over a 150k vocab is far too slow. PureCARD follows the **xgrammar-style split** into context-independent (cacheable) and context-dependent (runtime) token sets, with a per-state mask cache.

### 4.1 Compile once

1. **Bind** the fixed byte-level PDA to a vocabulary once per model vocabulary —
   the masks are vocabulary-indexed, so a different tokenizer needs its own
   `CompiledGrammar`. Bind the model vocabulary (`Vocab`: each token id → its
   raw byte string) into the grammar object, sizing an empty lazy per-state mask
   cache. Tokens are indexed directly by id; per-state acceptance is resolved by
   probing the PDA on first visit to each state (§4.5).

### 4.2 Partition the vocabulary per PDA state

1. Partition vocabulary tokens, per PDA state, into two classes:
   - **context-independent**: acceptance depends only on the current state, not on the stack contents (the vast majority — keywords, identifier characters, literals). Precompute a **per-state token bitmask cache**.
   - **context-dependent**: acceptance depends on the stack (e.g. a closing `)` / `]` is legal only if the matching opener is on top of the stack). This is a small set; check it at runtime by consulting the stack.

### 4.3 Per step

1. Compute the mask as:

   ```
   mask = cache[state]                         # cached context-independent bitmask
   mask = flip_context_dependent(mask, stack)  # small runtime stack check
   if an implemented L2 rule covers the current position:
       mask = mask ∩ schema_legal_terminals(scope)   # §6 narrowing
   return mask
   ```

   The context-dependent flip touches only the small set of stack-sensitive
   terminals. The current L2 intersection applies **only** at the selected
   identifier/type positions implemented from the §7 table, keeping the runtime
   fraction small.

### 4.4 Byte-level detokenization (BPE↔Pure alignment, solved)

1. Detokenization is **byte-level**, so subword boundaries never need special
   alignment: at L1, a candidate token is admissible iff **feeding its raw bytes
   advances the byte-PDA to a non-dead state**. This sidesteps the BPE/Pure-token
   misalignment that PICARD handled with explicit incremental parsing (§1.1).
   The decoder treats every model token as an opaque byte string; the host is
   responsible for supplying the correct raw bytes per token id (§9).

### 4.5 Cache construction

The per-state cache is built **lazily**: it memoizes each state's mask on first
visit rather than precomputing masks for unreachable states.

For L2, additionally **cache per-(state, class-scope) identifier masks**: the set of schema-legal identifiers after `$x.` depends only on the class `$x` is bound to, so it can be memoized per (position, class) pair rather than recomputed every step.

### 4.6 Performance measurements

The Criterion suite (`benches/allowed_mask.rs`) records relative per-step costs.
CI configuration defines which benchmark checks run for a change.

The families and the _relative_ cost each establishes (no absolute figures are quoted here: there is no gate asserting a hand-copied number against the bench output, so only the shape is stated — the bench itself holds the measurements):

- **`allowed_mask`** — steady-state per step, and the cheapest at shallow and identifier positions. The deep-stack worst case (nested open frames, maximal context-dependent re-probe) is the costliest per-step path.
- **`accept_token`** — a whole-token advance is cheap: a byte-fold through a PDA clone.
- **`cache_win`** — the partition cache: a warm step (word-wise copy) is dramatically cheaper than a cold first-visit build (which probes the whole ~150k-token vocabulary). This is why the lazy per-state cache is load-bearing, not an optimization.
- **`l2_overhead`** — the schema-narrowing block at an identifier position adds a small constant over the L1 mask (the `intersect` plus the scope-legal set build); L2 ⊆ L1 by construction, so it only ever narrows.

---

## 9. Public API (Rust + PyO3) and integration boundary

### 9.1 Rust core

This is a signature sketch, not compilable code. The authoritative, compile-and-run-checked usage example is the crate-root doctest in `src/lib.rs` (`cargo test --doc`), which drives this exact surface — so a rename or receiver change fails the build there, keeping this sketch honest.

```text
pub struct Vocab { /* token id -> raw bytes */ }
impl Vocab { pub fn from_byte_tokens(tokens: Vec<Vec<u8>>, eos: u32) -> Self; }

pub struct CompiledGrammar { /* owns Vocab + lazy per-state mask cache */ }
impl CompiledGrammar {
    pub fn compile(vocab: Vocab) -> Self;                             // fixed §5 PDA; bind vocab, size the lazy caches
    pub fn from_spec(spec: &str, vocab: Vocab) -> Result<Self, SpecError>; // validate + lower a supplied GrammarSpec (ADR-0010)
    pub fn vocab(&self) -> &Vocab;
}

pub struct Schema { /* §6.2 */ }
impl Schema { pub fn from_json(s: &str) -> Result<Self, SchemaError>; }

pub struct DecoderSession<'g> { /* cursor (fixed PDA or spec-compiled automaton), offset, scope, &CompiledGrammar */ }
impl<'g> DecoderSession<'g> {
    pub fn new(g: &'g CompiledGrammar) -> Self;                       // either grammar kind; L1 only
    pub fn with_schema(g: &'g CompiledGrammar, schema: Schema) -> Result<Self, DecodeError>; // fixed-PDA grammar only (partial L2 overlay)
    pub fn allowed_mask(&mut self) -> &BitMask;  // over vocab; EOS bit set iff is_complete().
                                                 // `&mut` because it refills the session's
                                                 // reused mask buffer and lazy per-state cache
                                                 // in place (no per-step alloc; unsafe is forbidden).
    pub fn accept_token(&mut self, id: u32) -> Result<(), DecodeError>;
    pub fn pda(&self) -> Option<&Pda>;            // Some only for a fixed-PDA-backed session
    pub fn is_complete(&self) -> bool;
    pub fn reset(&mut self);                      // reuse allocation across generations
}
```

`DecodeError` is the single decode-time error enum: a byte-level `DeadState` plus the token-level `InadmissibleToken` / `UnknownToken` / `UnexpectedEos` variants and `SchemaRequiresFixedGrammar` (from `with_schema` against a spec-compiled grammar). Grammar *construction* is fallible only through `from_spec`, which reports a malformed/unsupported/explosive spec as a distinct `SpecError` (`compile` itself stays infallible).

### 9.2 PyO3 boundary

`#[cfg(feature="python")]` — the _only_ Python-facing surface; keep it thin:

```python
# purecard (compiled extension)
g    = compile_grammar(spec_str, vocab_bytes, eos_id)     # once per (model, grammar)
sess = Session(g, schema_json_or_None)                    # once per generation
mask = sess.allowed_mask()        # -> np.ndarray[bool] or packed bits, len == vocab
sess.accept_token(tok_id)         # advance; raises on illegal token
sess.is_complete()                # bool
```

Maturin packages this module into a wheel to verify the boundary. The Cargo
package has `publish = false`, and the wheel is not a published release artifact.

### 9.3 Integration boundary (host code lives elsewhere, stated so the API is right)

PureCARD is the **Rust half of a Python/Rust split**. Python owns training,
datagen, inference orchestration, tokenization, and sampling; Rust owns the
decoder state and mask. PureCARD exposes itself via PyO3 and is designed to
constrain **only the final-query span** of an agentic trajectory (not the whole
trajectory).

Host-side contract for the inference loop (out of scope to build here as
shipped product code; `python/tests/test_real_model_inference.py` is a
reference implementation of it against a real model, `docs/spec/testing.md`
§8.8):

- The host provides the vocabulary as **raw byte strings per token id**, handling the tokenizer's metaspace / leading-space conventions (byte-BPE vs SentencePiece) _before_ handing bytes over. Getting this exactly right is a soundness prerequisite; the decoder treats tokens as opaque byte strings.
- The host builds `Schema` from the PMCD / MCP tools and passes it (as JSON) at session init.
- The host **activates constraint only over the final-query span** of a trajectory (a mode switch), not over tool calls or reasoning text.
- The host owns sampling; PureCARD only masks.
- Concrete loop: create a `Session` with the query's schema at the moment the final-query span begins; each step, `&`-mask the logits; sample; `accept_token`; stop when `is_complete()` and EOS is sampled.
