---
name: dependency-vetting
description: >-
  Vet a third-party crate before adding it to the workspace. Use whenever you are
  about to add a dependency, the user says "should we use crate X", "add a
  dependency", "vet this crate", "is X safe to depend on", or a plan calls for
  pulling in an external library. Produces a go/no-go decision (adopt vs.
  write-our-own) plus a vetting note and the cargo-deny / cargo-vet entry.
---

# Dependency Vetting

Adding a dependency is a permanent liability: license, supply-chain, maintenance,
and adaptation cost all transfer to us. The default answer is "not yet" until the
crate clears this rubric. This repo is **Apache-2.0** and gates dependencies with
`cargo-deny` (`deny.toml`) and `cargo-vet`. Nothing lands unvetted.

## When this triggers

Before running `cargo add`, before editing `[workspace.dependencies]` in the root
`Cargo.toml`, or whenever a plan/spec introduces a new external crate.

## Rubric — score each axis, record the evidence

1. **License compatibility (blocking).**
   - Must be compatible with Apache-2.0 distribution and present in the
     `deny.toml` allowlist. Green: `Apache-2.0`, `MIT`, `BSD-2/3-Clause`,
     `ISC`, `Zlib`, `Unicode-DFS-2016`, `Apache-2.0 WITH LLVM-exception`.
     Red: `GPL-*`, `AGPL-*`, `MPL` (case-by-case), `CC-BY-NC`, no license,
     or a dual license where neither arm is green.
   - Check transitive deps too: `cargo deny check licenses`.
2. **Maintenance & liveness.**
   - Last release date, release cadence, open vs. closed issue/PR ratio, whether
     it builds on our pinned toolchain (edition 2024, Rust 1.85+).
   - Red flags: last release > 18 months with open soundness issues; a lone
     unresponsive maintainer; "looking for maintainer" notices.
3. **Reputation & community.**
   - Download count / reverse-deps on crates.io, GitHub stars *as a weak signal
     only*, whether it's in the dependency tree of crates we already trust.
     Popularity is corroborating, never sufficient.
4. **Supply-chain / rug-pull risk.**
   - `cargo audit` (RustSec advisories) and `cargo vet` status.
   - Number and trust of transitive deps; any `build.rs` doing network/codegen;
     unsafe surface (`cargo geiger` if available); typosquat-shaped name;
     single-maintainer crate with publish rights to a widely-depended package.
5. **Fit for use / adaptation cost.**
   - Does it solve *our* problem or 80% of a different one? How much glue,
     wrapping, or forking would we need? A crate we must heavily adapt is often
     more expensive than a small purpose-built module.

## Pinning the version — never hand-type it

`cargo add <crate>` (or `cargo add --dev <crate>`, no explicit version arg) is
the *only* way a version enters `Cargo.toml`. Cargo resolves the actual current
release from the registry; typing a remembered version number is exactly how
scaffold-time pins went stale by months (constitution §2, "latest stable,
verified"). Dependabot is the last-mile safety net that catches drift
afterward — it is not a substitute for pinning correctly the first time.

## Run the tooling (through `just`)

```sh
just lint            # includes cargo-deny gate
cargo deny check licenses bans sources    # focused license/ban/source check
cargo audit                                # RustSec advisories
cargo vet check                            # supply-chain audit state
cargo tree -i <crate>                      # what pulls it in / its transitive tree
```

## Decision: adopt vs. write-our-own

- **Adopt** when: license green, actively maintained, small/trusted transitive
  tree, clean audit, and it fits with little adaptation.
- **Write-our-own** when: adaptation cost rivals the feature, the crate drags in
  a large/untrusted tree, it's abandoned, or the surface we need is small enough
  to own (a few hundred lines we can test and maintain). Prefer a small,
  well-tested internal module over a heavy dependency we don't understand.
- **Defer** when: promising but unaudited — file the vetting note as "pending"
  and get a human sign-off before it lands.

## Outputs (both required)

**1. Vetting note** — commit to `docs/dependencies/<crate>.md` (or the PR body):

```md
# Vetting: <crate> <version>
- Purpose: <why we need it, what it replaces>
- License: <SPDX> — compatible with Apache-2.0: yes/no (deny.toml allowlisted: yes/no)
- Maintenance: last release <date>, cadence <…>, issues <open/closed>
- Reputation: <downloads/reverse-deps/where-else-used>
- Supply chain: transitive deps <n>, build.rs <yes/no + what>, cargo-audit <clean/CVEs>, cargo-vet <status>
- Fit / adaptation cost: <low/med/high — describe glue needed>
- Decision: ADOPT | WRITE-OUR-OWN | DEFER — <one-line justification>
- Reviewer sign-off: <required for DEFER/edge licenses>
```

**2. cargo-deny / cargo-vet entry.** On ADOPT, add the crate to
`[workspace.dependencies]` and, if it needs an explicit exception, the entry in
`deny.toml` (license/ban/source) and/or record the audit via
`cargo vet certify <crate> <version>`. Loosening `deny.toml` (allowing a new
license or ban skip) is a **protected-gate change** — the reviewer requires human
sign-off (see `reviewer`).
