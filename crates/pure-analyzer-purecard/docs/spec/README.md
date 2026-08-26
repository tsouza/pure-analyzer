# PureCARD product reference

**A Rust grammar/schema-constrained decoder for Legend Pure (a "PICARD-for-Pure"
constrained-decoding library).**

- **OSS project name:** `PureCARD` — _Pure_ + _PICARD_ lineage; reads as the
  "reference **card** of legal moves" for Pure generation.
- **Package / modules:** the Cargo package is `pure-analyzer-purecard`; the Rust
  library and Python `#[pymodule]` remain `purecard`. There is no `picard_pure`
  identifier in the code.
- **Repository placement:** an independent sibling product colocated in the Pure
  Analyzer monorepo, with zero dependency edges in either direction
  ([ADR-0009](../decisions/0009-monorepo-placement.md)).

Together the files below specify the decoder product. Its external inputs at
build/test time are
(a) the _test corpus_ of gold Pure queries and (b) a running _Legend engine_,
both located in [`testing.md`](testing.md) §8. The host-side Python
model/tokenizer/inference stack that drives it is
out of scope here — see §2 and §9 — as are general Rust workspace conventions,
CI, and agentic dev setup. Those shared concerns are governed by the root
[constitution](../../../../constitution.md), `just` frontend, CI, and
[methodology](../../../../docs/methodology/).

Context in one line: an upstream project ("pure-lingua") trains an LLM to emit
Legend Pure queries; PureCARD provides a per-step logits transform over the
emitted subset. This reference is the authoritative source that
[`../domain-model.md`](../domain-model.md) navigates and elaborates.

The decoder has a hand-written emitted-subset PDA, lazy mask cache, an L2
overlay at selected schema-sensitive positions, a PyO3/wheel boundary, fuzz
targets, and benches. Its [operating limits](overview.md#10-operating-limits)
state the guarantees it does and does not provide.

Section numbers (`§N`) are preserved verbatim as headings, so any `§N` reference
resolves to the file below.

| Sections                          | File                               | Covers                                                                                 |
| --------------------------------- | ---------------------------------- | -------------------------------------------------------------------------------------- |
| §1, §2, §10, §12, Appendix B      | [overview.md](overview.md)         | What PureCARD is, the guarantee boundary, scope, limits, and prior art                 |
| §3, §4, §9                        | [architecture.md](architecture.md) | Architecture, the masking algorithm, the public Rust + PyO3 API                        |
| §5                                | [grammar.md](grammar.md)           | L1 — the emitted-Pure syntactic grammar                                                |
| §6, §7                            | [schema.md](schema.md)             | L2 — schema-consistency and the L1↔L2 contract                                         |
| §8, §13, §14                      | [testing.md](testing.md)           | The oracle-driven test strategy, the corpus, the Legend engine + CI                    |

For the testing _methodology_ (the layered pyramid that operationalizes §8), see
[../methodology/decoder-testing.md](../methodology/decoder-testing.md).
