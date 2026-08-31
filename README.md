# pure-analyzer

An umbrella Rust workspace for two Legend Pure tools that share repository
infrastructure, but remain independent products.

## Products

### pure-analyzer

`pure-analyzer` is a mechanical, standalone static-analysis toolchain for
[Legend Pure](https://legend.finos.org/) and its modern `Relation<>` dialect.
Its analyzer crates follow the dependency direction described in the
[domain model](docs/domain-model.md).

For command use, configuration, output, and safe file-update behavior, see the
[Pure Analyzer user guide](docs/pure-analyzer.md).

### pure-analyzer-purecard

[`pure-analyzer-purecard`](crates/pure-analyzer-purecard/) is the PureCARD
constrained decoder. It masks language-model tokens against its fixed
emitted-subset grammar and, at covered L2 positions, an optional schema. Its
[product reference](crates/pure-analyzer-purecard/docs/spec/README.md) defines
the decoder boundary and operating limits.

PureCARD remains unpublished here: its Rust package has `publish = false`, and
CI builds Python wheels only as verification artifacts. See the
[`PureCARD README`](crates/pure-analyzer-purecard/README.md) for its API,
guarantee boundary, and specialized development lanes.

## Product boundary

The products are co-located, not layered together:

- There are zero analyzer-to-PureCARD or PureCARD-to-analyzer Cargo dependency
  edges, including normal, development, build, optional, and renamed edges.
- `xtask` and root automation are shared repository infrastructure, not a third
  product or an analyzer layer.
- The analyzer processing pipeline is `lexer → syntax → parser → model
  → resolve → analysis → libpure → cli`. Cargo dependencies point
  toward prerequisites: notably, resolver may depend on model; model must not
  depend on resolver. Diagnostics is a shared leaf within the analyzer product.
- Co-location does not authorize parser, corpus, or ownership sharing. Any such
  integration requires a GitHub Issue and ADR.

`cargo xtask verify-layering` enforces both the analyzer DAG and the
analyzer–PureCARD product boundary. See
[ADR-0003](docs/decisions/0003-analysis-engine-crate-dag.md),
[ADR-0004](docs/decisions/0004-purecard-independent-workspace-product.md), and
PureCARD's
[ADR-0009](crates/pure-analyzer-purecard/docs/decisions/0009-monorepo-placement.md).

## Layout

```text
crates/
  pure-analyzer-lexer/       analyzer lexer
  pure-analyzer-syntax/      analyzer syntax types
  pure-analyzer-parser/      analyzer parser
  pure-analyzer-model/       analyzer model loader
  pure-analyzer-resolve/     analyzer resolver
  pure-analyzer-analysis/    analyzer passes
  pure-analyzer-diagnostics/ shared analyzer diagnostics
  libpure/                   analyzer facade
  pure-analyzer-cli/         analyzer CLI
  pure-analyzer-purecard/    independent constrained-decoder product
xtask/                       shared repository automation
docs/                        root governance and methodology
```

## Building

```sh
mise install && mise run install-cargo-tools   # provision toolchain once
just ci                                        # fast workspace gate
just ci-full                                   # local mirror of reproducible PR gates
```

`just` is the supported entry point; use `just --list` to discover analyzer,
PureCARD, and repository-wide tasks. Contributions follow
[`CONTRIBUTING.md`](CONTRIBUTING.md) and the rules in
[`constitution.md`](constitution.md).

## Performance measurements

`just bench` runs the workspace Criterion benchmarks locally.

Public API snapshots are mandatory. `just public-api` compares every public
Rust crate's all-features surface with the reviewed files in `public-api/`;
missing or stale files fail the gate. For an intended API change, run
`just public-api-bless`, review the regenerated baseline diff, and commit it
with the implementation change.

## License

Apache-2.0. See [`LICENSE`](LICENSE).
