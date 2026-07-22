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

This is a starter kit under active development. Security fixes are applied to
`main`. There is no long-term-support branch.

## Our own guardrails

Security is partly enforced in CI, not just by policy:

- `gitleaks` runs in a pre-commit hook and in CI to catch committed secrets.
- `cargo-audit` and `cargo-deny` fail the build on known-vulnerable or
  disallowed dependencies.
- `#![forbid(unsafe_code)]` is mandatory in every crate.
- Untrusted HTTP/gRPC input is exercised by the fuzzing layer (bolero + OSS-Fuzz).

See [`docs/methodology/quality-layers.md`](docs/methodology/quality-layers.md) for
how these fit together.
