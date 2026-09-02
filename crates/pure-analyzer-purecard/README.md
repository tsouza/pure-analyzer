# PureCARD

**A grammar- and schema-constrained decoder for
[Legend Pure](https://legend.finos.org/).**

PureCARD sits between a language model's logits and its sampler and masks next
tokens that cannot continue an emitted-subset Pure query. It is the Pure analogue
of [PICARD](https://arxiv.org/abs/2109.05093), implemented as a byte-level Rust
decoder with an optional Python boundary.

PureCARD is an **independent sibling product** colocated with Pure Analyzer in
this monorepo. One name covers everything a consumer names it by: the Cargo
package, the Rust library, and the Python module are all `purecard`, and only
the workspace directory is `crates/pure-analyzer-purecard/`. The crate publishes
to crates.io and its abi3 wheels to PyPI. Neither product depends on the other's
internals. They share repository governance and automation, not a runtime
architecture.

> **Validity is not faithfulness.** PureCARD constrains membership in its fixed
> emitted-subset recognizer and narrows at implemented schema positions. Those
> constraints are not a general compiler-validity guarantee, and they cannot
> decide whether a query means what the user asked.

## The guarantee boundary

The constraint levels form a strict containment hierarchy:

| Level                     | Contract                                                                       | Scope                                                      |
| ------------------------- | ------------------------------------------------------------------------------ | ---------------------------------------------------------- |
| **L1 · Syntactic**        | Output belongs to the hand-written emitted-subset Pure grammar                 | Fixed-PDA membership                                       |
| **L2 · SchemaConsistent** | Covered identifier/type positions narrow against the supplied model schema     | Selected schema positions only                             |
| **L3 · Faithful**         | The query answers the question that was asked                                  | Out of scope                                               |

See [the product reference](docs/spec/README.md) for the emitted-Pure grammar, the
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

## Operating limits

PureCARD does not include a model-inference runner, a full Pure compiler, or a
full type checker. `CompiledGrammar::from_spec` selects the fixed PDA and the
walker is schema-agnostic. The Qwen lane verifies tokenizer token-ID replay; it
does not establish host inference-loop behavior or compiler validity of every
accepted query. See the [product reference](docs/spec/overview.md#10-operating-limits)
for the precise boundary.

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

The Cargo package has `publish = true` — alone in this workspace — and releases
to crates.io as `purecard`; its maturin wheels release to PyPI under the same
name. Both are cut from release-plz's release PR, on a published GitHub Release,
so no ordinary merge to `main` ships anything.

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
