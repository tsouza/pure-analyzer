# PureCARD specification

**A Rust grammar/schema-constrained decoder for Legend Pure (a "PICARD-for-Pure"
constrained-decoding library).**

- **OSS project name:** `PureCARD` — _Pure_ + _PICARD_ lineage; reads as the
  "reference **card** of legal moves" for Pure generation.
- **Crate / repo:** `purecard` — the crate is named `purecard` everywhere (the
  `#[pymodule]` is `purecard`, the lib is `purecard`); there is no `picard_pure`
  identifier in the code.
- **Status:** Implemented — all milestones M0–M5 are shipped; this spec is the
  authoritative design the shipped decoder tracks.

Together the files below are the _complete specification of the decoder_: a
fresh engineer (or a fresh Claude instance) can build the PureCARD crate from
them alone — no other design docs. Its external inputs at build/test time are
(a) the _test corpus_ of gold Pure queries and (b) a running _Legend engine_,
both located in [`testing.md`](testing.md) §8. The host-side Python
model/tokenizer/inference stack that drives it (the M4 integration surface) is
out of scope here — see §2 and §9 — as are general Rust workspace conventions,
CI, and agentic dev setup.

Context in one line: an upstream project ("pure-lingua") trains an LLM to emit
Legend Pure queries; at serving time we want _guaranteed-valid_ output without a
compile-repair round-trip. PureCARD provides that guarantee as a per-step logits
transform during autoregressive decoding. This spec is the authoritative source
that [`../domain-model.md`](../domain-model.md) navigates and elaborates.

Section numbers (`§N`) are preserved verbatim as headings, so any `§N` reference
resolves to the file below.

| Sections                          | File                               | Covers                                                                                 |
| --------------------------------- | ---------------------------------- | -------------------------------------------------------------------------------------- |
| §1, §2, §10, §11, §12, Appendix B | [overview.md](overview.md)         | What PureCARD is, the guarantee boundary, scope, milestones, risks, roadmap, prior art |
| §3, §4, §9                        | [architecture.md](architecture.md) | Architecture, the masking algorithm, the public Rust + PyO3 API                        |
| §5                                | [grammar.md](grammar.md)           | L1 — the emitted-Pure syntactic grammar                                                |
| §6, §7                            | [schema.md](schema.md)             | L2 — schema-consistency and the L1↔L2 contract                                         |
| §8, §13, §14                      | [testing.md](testing.md)           | The oracle-driven test strategy, the corpus, the Legend engine + CI                    |

For the testing _methodology_ (the layered pyramid that operationalizes §8), see
[../methodology/decoder-testing.md](../methodology/decoder-testing.md).
