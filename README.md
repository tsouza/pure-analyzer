# pure-analyzer

A mechanical, standalone Rust static-analysis toolchain for [Legend
Pure](https://legend.finos.org/) — the modern `Relation<>` dialect
(`meta::pure::functions::relation::*`), not the legacy `TabularDataSet` API. No
LLM, no runtime engine, no network: identical inputs always produce
byte-identical output.

## Why it exists

Deep milestoned navigation — getting the count of `%latest` temporal date
arguments right on a navigation chain — is a real Legend/Reladomo developer
footgun, one even frontier LLMs routinely get wrong end-to-end. The required
arity depends on the *target* class's temporal stereotype (`bitemporal` →2,
`businesstemporal`/`processingtemporal` →1, none →0), threaded fresh through
every hop of a chain, with a context-gate that legalizes a bare 0-arg call only
when the immediate source class is itself compatibly milestoned. `pure-analyzer
lint` mechanically decides this — no engine required at runtime.

No fast, standalone static-analysis toolchain for Pure existed before this;
prior tooling is Java/IDE-based and needs the full Legend engine.

## What it does

One shared analysis engine (`libpure`: lexer → resilient parser → lossless CST
→ model loader → resolver), two front-ends over it — a CLI and, from v0.2, a
Language Server — so every check is available identically at the command line
and live in an editor:

- **`validate`** — grammar + shallow well-formedness, no model needed.
- **`lint`** — the milestoning `%latest`-arity core, unknown-property, and
  statically-determinate multiplicity misuse. Needs a model (PMCD JSON, or a
  parsed Pure model file — engine-free either way).
- **`eq` / `diff`** — sound, incomplete, 3-valued structural equivalence
  (`EQUIVALENT` / `NOT_EQUIVALENT`+witness / `INDECISIVE`+reason) over the
  decidable relational core. Never wrongly commits an equivalence verdict.
- **`fmt`** — canonical, idempotent formatting from the lossless CST.
- **LSP (`pure-analyzer-lsp`)** — the same `Diagnostic`s as live squiggles,
  each `Fix` as a code-action, `explain` as hover, go-to-definition via the
  resolver.

See [`docs/design/pure-analyzer-design.md`](docs/design/pure-analyzer-design.md)
for the full specification — grammar, the milestoning-arity algorithm,
subcommand contracts, model formats, and staged milestones.

## Layout

```text
crates/
  pure-analyzer-lexer/       logos-derived token layer (%latest/dates, islands)
  pure-analyzer-syntax/      SyntaxKind + rowan Language impl, typed AST views
  pure-analyzer-parser/      resilient RD + Pratt parser -> lossless CST
  pure-analyzer-model/       PMCD JSON + Pure-model-file loader -> ModelGraph
  pure-analyzer-resolve/     source-threaded nav resolution, milestoning arity
  pure-analyzer-analysis/    Pass/visitor layer: validate + lint
  pure-analyzer-diagnostics/ the shared Diagnostic model (leaf, no renderers)
  libpure/                   thin facade over the above; the whole product
  pure-analyzer-cli/         the `pure-analyzer` binary: clap, renderers, exit codes
docs/
  design/pure-analyzer-design.md   the full implementation spec
  domain-model.md                  the evolving "what": entities, workflows, invariants
  decisions/                       architecture decision records
```

Dependencies point inward only, along the DAG: `lexer -> syntax -> parser ->
{model, resolve} -> analysis -> libpure -> cli`, with `pure-analyzer-diagnostics`
as a shared leaf — enforced by `cargo xtask verify-layering`, not convention.

## Building

```sh
mise install && mise run install-cargo-tools   # provision toolchain once
just ci                                        # build, lint, test
```

`just` is the only supported entry point — see `just --list` for every target
(spec scaffolding, coverage, mutation testing, fuzzing, the full CI mirror).
This project is developed under a spec-driven, gate-enforced engineering
methodology; see [`CONTRIBUTING.md`](CONTRIBUTING.md) if you're submitting a
change, and [`constitution.md`](constitution.md) for the rules that govern it.

## License

Apache-2.0. See [`LICENSE`](LICENSE).
