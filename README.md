# pure-analyzer

An umbrella Rust workspace for two Legend Pure tools that share repository
infrastructure, but remain independent products.

## Products

### pure-analyzer

`pure-analyzer` is a mechanical, standalone static-analysis toolchain for
[Legend Pure](https://legend.finos.org/) and its modern `Relation<>` dialect.
Its target design is deterministic and engine-free at runtime: lexer, parser,
model loader, resolver, analysis passes, and thin CLI/LSP front ends.

The analyzer is currently an **early scaffold**. The lexer and shared diagnostic
model contain real implementations. Syntax, parser, model, resolver, analysis,
and `libpure` are mostly version-reporting stubs, and the CLI subcommands return
`not implemented yet`. The planned `validate`, `lint`, `eq`/`diff`, `fmt`, and
LSP behavior is a roadmap, not a claim about the current binary.

See [`docs/design/pure-analyzer-design.md`](docs/design/pure-analyzer-design.md)
for that target design and [`docs/domain-model.md`](docs/domain-model.md) for
current domain and implementation truth.

### pure-analyzer-purecard

[`pure-analyzer-purecard`](crates/pure-analyzer-purecard/) is the PureCARD
constrained decoder. It masks language-model tokens against its emitted-subset
grammar and, at implemented L2 positions, an optional schema. Its M0–M5 code
artifacts—including the L1/L2 decoder, PyO3 boundary, offline gold corpus, fuzz
targets, and benchmarks—are implemented, while its
[end-to-end proof obligations](crates/pure-analyzer-purecard/docs/spec/overview.md#10-milestone-implementation-status-m0m5)
remain open; PureCARD does not yet claim feature completeness.

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
  integration requires a dedicated spec and ADR.

`cargo xtask verify-layering` enforces both the analyzer DAG and the
analyzer–PureCARD product boundary. See
[ADR-0003](docs/decisions/0003-analysis-engine-crate-dag.md),
[ADR-0004](docs/decisions/0004-purecard-independent-workspace-product.md), and
PureCARD's
[ADR-0009](crates/pure-analyzer-purecard/docs/decisions/0009-monorepo-placement.md).

## Layout

```text
crates/
  pure-analyzer-lexer/       implemented analyzer lexer
  pure-analyzer-syntax/      analyzer syntax scaffold
  pure-analyzer-parser/      analyzer parser scaffold
  pure-analyzer-model/       analyzer model-loader scaffold
  pure-analyzer-resolve/     analyzer resolver scaffold
  pure-analyzer-analysis/    analyzer pass scaffold
  pure-analyzer-diagnostics/ implemented shared analyzer diagnostics
  libpure/                   analyzer facade scaffold
  pure-analyzer-cli/         analyzer CLI scaffold
  pure-analyzer-purecard/    independent constrained-decoder product
xtask/                       shared repository automation
docs/                        root governance, design, and methodology
specs/                       root change specifications
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

## Optional gates (off by default)

Two CI protections require repository administration and therefore start
disabled. Until enabled, their absence is an explicit protection gap rather
than evidence that those properties were checked.

- **CodSpeed:** install the CodSpeed GitHub App, then set the repository Actions
  variable `CODSPEED_ENABLED=true`. The `bench (codspeed)` job will run
  `just codspeed` for code changes. Without it, CI does not block performance
  regressions; `just bench` remains available for local measurements.
- **Public API snapshots:** run `just public-api-bless`, review and commit the
  generated `public-api/` baselines, then set the repository Actions variable
  `PUBLIC_API_ENABLED=true`. The nightly-backed snapshot comparison then joins
  the always-on semantic-version check. Without it, exact public-surface drift
  is not blocked by snapshot comparison.

## License

Apache-2.0. See [`LICENSE`](LICENSE).
