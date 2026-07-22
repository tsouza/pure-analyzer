# pure-analyzer

A mechanical, standalone, Rust static-analysis toolchain for Legend Pure (the
modern `Relation<>` dialect) — no LLM, no runtime engine, no network. It is
built by an AI coding agent under a machine-enforced methodology inherited
from a domain-agnostic starter kit; see [`CLAUDE.md`](CLAUDE.md) and
[`constitution.md`](constitution.md).

pure-analyzer targets a real, proven-hard gap: getting the `%latest`
milestoning-date arity right on a navigation chain is a genuine Legend/Reladomo
developer footgun that even frontier LLMs get wrong end-to-end.
`pure-analyzer lint` mechanically decides it. See
[`docs/design/pure-analyzer-design.md`](docs/design/pure-analyzer-design.md)
for the full specification this project implements — background, grammar,
the milestoning-arity algorithm, subcommand contracts, and staged milestones.

## Why it exists

An AI agent can write a lot of code quickly. The hard part is keeping that code
*correct, consistent, and honest* over hundreds of changes without a human
re-reviewing every line. The methodology this project inherited answers that by:

- Pushing quality checks down into **deterministic gates** wherever possible, so
  judgment (and tokens) are spent only where a machine can't decide.
- Splitting the agent into a **cheap generator and a stronger reviewer**, so
  review is rigorous without being ruinously expensive.
- Making the system **self-learning but ratcheted**: it can loosen its
  understanding of the domain freely, but it can only ever tighten its guardrails.

Read [`docs/methodology/overview.md`](docs/methodology/overview.md) for the full
picture.

## Quickstart

Day-to-day:

```sh
just ci                 # run the full local gate — build, lint, test, audit
```

To re-provision a tool later: `mise install && mise run install-cargo-tools`.

Start a feature the way the agent does:

```sh
just new-feature <name> # create a git worktree + branch for the change
just spec <name>        # scaffold specs/<name>.md, then drive /spec:
                        #   plan → implement → verify
```

`just` is the only supported entry point. If you need a target that doesn't
exist yet, add it — that's a rule, not a suggestion.

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
xtask/    typed CI logic invoked by just
docs/
  design/pure-analyzer-design.md   the full implementation spec
  domain-model.md                  the evolving "what", elaborating the design doc
  lessons.md                       heuristics ledger (provisional → confirmed)
  decisions/                       architecture decision records
  methodology/                     how this kit works, in depth
constitution.md         the non-negotiable rules
CLAUDE.md               what the agent reads every session (thin; links here)
```

Dependencies point inward only, along the DAG: `lexer -> syntax -> parser ->
{model, resolve} -> analysis -> libpure -> cli`, with `pure-analyzer-diagnostics`
as a shared leaf. Only the front-end crate (`pure-analyzer-cli` today;
`pure-analyzer-lsp` in v0.2) may depend on a renderer (`ariadne`,
`codespan-reporting`) or protocol crate (`clap`, later `tower-lsp`). The
layering is enforced by `cargo xtask verify-layering` (see ADR-0003) and the
`no-front-end-deps-in-core` ast-grep rule, not by good intentions.

## Working with the agent

The agent reads [`CLAUDE.md`](CLAUDE.md) each session; that file is deliberately
thin and links into `docs/`. The rules it must obey are in
[`constitution.md`](constitution.md). The methodology docs explain the testing
pyramid, the L0–L4 quality layers, the self-learning loop, and the reviewer
cascade.

## Optional gates (off by default)

A few gates ship **wired but disabled**, so the project isn't red on day one.
Each stays dormant until you flip a repo variable or add a baseline — and until
then the thing it guards ships **unprotected**:

- [ ] **Performance regression (CodSpeed)** — install the
  [CodSpeed GitHub App](https://codspeed.io/) on the repo, then set the repo
  variable `CODSPEED_ENABLED=true`. Until then, criterion deltas are not gated.
- [ ] **Public-API snapshot (`cargo-public-api`)** — generate baselines with
  `just public-api-bless`, commit them, then set `PUBLIC_API_ENABLED=true` (needs
  a nightly toolchain). Until then, unintended public-API changes are not caught
  (`cargo-semver-checks` still runs on every PR).

Set a variable with `gh variable set CODSPEED_ENABLED --body true`, or via
**Settings → Secrets and variables → Actions → Variables**. The fuzz-smoke,
coverage, mutation, and structural gates are on by default — nothing to enable.

## Contributing

See [`CONTRIBUTING.md`](CONTRIBUTING.md), [`CODE_OF_CONDUCT.md`](CODE_OF_CONDUCT.md),
and [`SECURITY.md`](SECURITY.md). Contributions run through the same gates the
agent does.

## License

Apache-2.0. See [`LICENSE`](LICENSE).
