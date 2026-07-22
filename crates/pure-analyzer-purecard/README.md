# PureCARD

**A grammar- and schema-constrained decoder for [Legend Pure](https://legend.finos.org/).**

PureCARD sits between a language model's logits and its sampler and masks every
next token that cannot lead to a valid Pure query — so a model's output is valid
**by construction** in a single forward pass, with no compile-repair round-trip.
It is the Pure analogue of [PICARD](https://arxiv.org/abs/2109.05093) (the SQL
constrained decoder), solved at the byte level.

> **Guarantees validity — never faithfulness.** PureCARD makes a query *parse*
> and *resolve against a schema*. It cannot make the query *mean what was asked*;
> that is structurally out of reach at decode time.

## The guarantee boundary

The levels form a strict containment hierarchy — every faithful query is
schema-consistent, and every schema-consistent query is syntactic:

| Level                     | Guarantee                                                                                    | Status                     |
| ------------------------- | -------------------------------------------------------------------------------------------- | -------------------------- |
| **L1 · Syntactic**        | Output parses as (emitted-subset) Pure                                                       | Core                       |
| **L2 · SchemaConsistent** | Every identifier and type resolves against *this* model — no phantom names, no type mismatch | Overlay                    |
| **L3 · Faithful**         | The query answers the question that was asked                                                | Out of scope (undecidable) |

See [the specification](docs/spec/README.md) for the full design: the
emitted-Pure grammar, the byte-level masking algorithm, the schema-consistency
overlay, the public API, and the oracle-driven test strategy.

## How it works

At each decode step the host applies PureCARD's mask before sampling:

```text
logits  = model.forward(...)
mask    = session.allowed_mask()     # bitmask over the vocabulary
logits[!mask] = -inf                 # disallowed tokens can never be sampled
tok     = sample(logits)
session.accept_token(tok)            # advances the PDA + scope; errors on illegal
```

The recognizer is a byte-level pushdown automaton (L1) with a typed-scope overlay
that narrows identifier and type positions to a specific model's schema (L2).
A per-state mask cache keeps mask generation off the critical path.

## Status

All milestones **M0–M5** are shipped; the decoder is feature-complete. The
milestone arc tracked in [`docs/spec/overview.md`](docs/spec/overview.md) §10 is
delivered end to end:

- **M0** oracle harness · **M1** L1 grammar · **M2** performance ·
  **M3** L2 schema overlay · **M4** PyO3 boundary · **M5** hardening.

That means the L1 syntactic recognizer and the L2 schema-consistency overlay, the
per-state mask cache, the feature-gated PyO3 boundary, and the hardening pass
(tokenizer self-check, EOS finalization, the split `DecodeError`, fuzz targets,
and benches) are all in `main`.

The soundness backbone — replaying the committed gold corpus through the decoder
— runs fully offline, with no Legend engine required.

## Corpus

[`corpus/`](corpus/) ships the test oracle, self-contained:

- `gold_queries.jsonl` — 5,034 execution-verified gold Pure queries across 161
  databases (the offline **soundness** oracle, and the empirical basis the
  grammar is derived from).
- `schemas/*.md` — 8 database schemas (the **L2** test inputs).
- `legend-stack/` — the pinned Legend engine docker stack (the **completeness**
  oracle, engine-backed, opt-in).

## Python

PureCARD is the Rust half of a Rust/Python split. Python owns training, datagen,
and the inference loop; PureCARD exposes itself through a thin **PyO3** boundary
(a maturin wheel) that constrains only the final-query span of a trajectory. That
boundary landed in M4 — see [`src/ffi.rs`](src/ffi.rs), the `python` Cargo
feature, and the maturin wheel build.

## Development

This repository also carries an AI-driven engineering methodology: deterministic
quality gates, a spec-driven change loop, and a stronger reviewer agent as the
merge gate.

```sh
mise install && mise run install-cargo-tools   # provision toolchain + hooks (once)
just ci                                        # the full local gate
just new-feature <name>                        # worktree + branch for a change
just spec <name>                               # scaffold a spec, then /spec
```

The rules are in [`constitution.md`](constitution.md); the loop is documented
under [`docs/methodology/`](docs/methodology/). `just` is the only supported
entry point.

## Contributing

See [`CONTRIBUTING.md`](CONTRIBUTING.md), [`CODE_OF_CONDUCT.md`](CODE_OF_CONDUCT.md),
and [`SECURITY.md`](SECURITY.md). Contributions run through the same gates the
agent does.

## License

Apache-2.0. See [`LICENSE`](LICENSE).
