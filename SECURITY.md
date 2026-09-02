# Security Policy

## Reporting a vulnerability

Please **do not** open a public issue for a security vulnerability.

Report it privately through GitHub's
[**Report a vulnerability**](https://github.com/tsouza/pure-analyzer/security/advisories/new)
flow (Security → Advisories → Report a vulnerability). If that is unavailable,
email **<tcostasouza@gmail.com>** with the subject line `SECURITY`.

Include, as best you can:

- the affected component and version / commit,
- a description of the issue and its impact,
- reproduction steps or a proof of concept.

We aim to acknowledge a report within **3 business days** and to agree on a
disclosure timeline with you. Please give us a reasonable window to ship a fix
before any public disclosure.

## Supported versions

This is a pre-1.0 umbrella repository under active development. Security fixes
for both the analyzer scaffold and PureCARD are applied to `main`. There is no
long-term-support branch. PureCARD is the only crate configured to publish:
fixes reach users as a new `purecard` release on crates.io, and as wheels on
PyPI once the first release is cut. Older releases are not patched in place. The
analyzer crates are unpublished, so `main` is the only place their fixes
exist.

## Our own guardrails

Security is partly enforced in CI, not just by policy:

- `gitleaks` runs in a pre-commit hook and in CI to catch committed secrets.
- `cargo-audit` and `cargo-deny` fail the build on known-vulnerable or
  disallowed dependencies.
- `#![forbid(unsafe_code)]` is mandatory in every crate.
- `cargo-fuzz`/libFuzzer exercises analyzer diagnostic serialization and
  PureCARD decoder boundaries in their separate fuzz workspaces. Analyzer
  parser fuzzing lands with the parser implementation. CI runs bounded fuzz
  lanes; long-horizon OSS-Fuzz integration is not currently deployed.
- `cargo xtask verify-layering` rejects analyzer–PureCARD dependency edges in
  either direction, including development and build dependencies, limiting
  accidental cross-product attack-surface coupling.

See [`docs/methodology/quality-layers.md`](docs/methodology/quality-layers.md) for
how these fit together.
