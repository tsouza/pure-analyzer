# PureCARD

**A grammar- and schema-constrained decoder for
[Legend Pure](https://legend.finos.org/).**

PureCARD sits between a language model's logits and its sampler and masks next
tokens that cannot continue an emitted-subset Pure query. It is the Pure analogue
of [PICARD](https://arxiv.org/abs/2109.05093), implemented as a byte-level Rust
decoder with an optional Python boundary.

PureCARD is an **independent sibling product** colocated with Pure Analyzer in
this monorepo. The Cargo package is `pure-analyzer-purecard`; its Rust library
and Python module remain `purecard`. Neither product depends on the other's
internals. They share repository governance and automation, not a runtime
architecture.

> **Validity is not faithfulness.** PureCARD constrains membership in its fixed
> emitted-subset recognizer and narrows at implemented schema positions. Those
> constraints are not a general compiler-validity guarantee, and they cannot
> decide whether a query means what the user asked.

## The guarantee boundary

The intended levels form a strict containment hierarchy:

| Level                     | Contract                                                                       | Scope                                                      |
| ------------------------- | ------------------------------------------------------------------------------ | ---------------------------------------------------------- |
| **L1 · Syntactic**        | Output belongs to the hand-written emitted-subset Pure grammar                 | Fixed-PDA membership implemented; live validity proof open |
| **L2 · SchemaConsistent** | Implemented identifier/type positions narrow against the supplied model schema | Implemented, partial overlay                               |
| **L3 · Faithful**         | The query answers the question that was asked                                  | Out of scope                                               |

See [the specification](docs/spec/README.md) for the emitted-Pure grammar, the
byte-level masking algorithm, the implemented schema overlay, the public API,
and the oracle-driven test strategy.

## How it works

At each decode step the host applies PureCARD's mask before sampling:

```text
logits  = model.forward(...)
mask    = session.allowed_mask()
logits[!mask] = -inf
tok     = sample(logits)
session.accept_token(tok)
```

The recognizer is a hand-written byte-level pushdown automaton (L1). A typed
scope overlay narrows selected identifier and type positions against a supplied
schema (L2). A lazy per-state mask cache keeps repeated mask generation off the
critical path.

## Current status

The code artifacts planned for M0–M5 are implemented:

- **M0:** committed oracle/corpus harness and offline replay;
- **M1:** hand-written PDA for the emitted subset;
- **M2:** lazy mask cache, equivalence tests, and benchmarks;
- **M3:** the implemented schema-overlay subset;
- **M4:** thin, feature-gated PyO3 boundary and maturin wheel build; and
- **M5:** tokenizer self-check, EOS/error hardening, fuzz targets, and benches.

The always-on offline lanes replay the 5,034-query corpus and the modern-dialect
seeds. In addition, [`tests/qwen_soundness.rs`](tests/qwen_soundness.rs) loads the
actual pinned Qwen tokenizer and replays real tokenizer token IDs; it runs
on-demand and in the scheduled
[`purecard-qwen-oracle.yml`](../../.github/workflows/purecard-qwen-oracle.yml)
workflow. This is real-tokenizer token-ID evidence. It is **not** real-model
inference and it does not invoke Legend.

The following end-to-end proof obligations remain open:

1. validate a 100% constrained-walk compile rate against a live Legend engine;
2. lower grammar specifications into the PDA (`CompiledGrammar::from_spec`
   currently selects the fixed PDA);
3. generate schema-constrained accepting walks; and
4. run real-model Python inference, constrain the produced query, and compile it
   against live Legend.

Accordingly, PureCARD does not claim to be feature-complete or to have proven
the original M0–M5 milestone definitions end to end.

## Corpus

[`corpus/`](corpus/) ships the test inputs inside this crate:

- `gold_queries.jsonl` — 5,034 execution-verified Pure queries across 161
  databases, used as the frozen offline soundness oracle;
- `modern_dialect_seeds.jsonl` — provenance-distinct seeds for newer emitted
  Legend constructs;
- `schemas/*.md` — eight database schemas used by the implemented L2 tests; and
- `legend-stack/` — the pinned Legend engine stack for the opt-in live lane.

## Python and distribution

The `python` Cargo feature exposes the Rust library through a thin PyO3 module
named `purecard`. Python owns model inference, tokenization orchestration, and
sampling; the boundary constrains only the final-query span.

The Cargo package has `publish = false`. Maturin wheels are CI and integration
verification artifacts only; the monorepo does not publish this package or its
wheels as a release product.

## Development and governance

Run PureCARD from the repository root through the shared `just` frontend:

```sh
mise install && mise run install-cargo-tools
just ci
just qwen-oracle
just test-legend
just wheel
```

PureCARD follows the root [constitution](../../constitution.md),
[contribution guide](../../CONTRIBUTING.md), and
[methodology](../../docs/methodology/). Its decoder-specific specification,
ADRs, and testing guidance remain under this crate. The placement and boundary
decision is recorded in
[ADR-0009](docs/decisions/0009-monorepo-placement.md).

## Contributing

See the root [contribution guide](../../CONTRIBUTING.md),
[code of conduct](../../CODE_OF_CONDUCT.md), and
[security policy](../../SECURITY.md). Contributions use the same root gates as
the analyzer.

## License

Apache-2.0. See the root [license](../../LICENSE).
