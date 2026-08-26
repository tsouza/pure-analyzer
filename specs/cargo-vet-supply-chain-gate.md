# Spec: cargo-vet-supply-chain-gate

- Status: complete
- Created: 2026-08-26
- Owner: shared repository infrastructure

## Problem

The dependency-vetting workflow says that every third-party crate is checked by
`cargo-vet`, but the repository has no cargo-vet store, local `just` target, or
CI gate. A dependency can therefore pass the documented process while no
mechanical check proves that its exact version is covered by an audit or an
explicit exemption.

## Goals

- [x] Pin cargo-vet 0.10.2 for contributors and CI through the existing mise
      toolchain. Authenticate all seven supported QuickInstall archives against
      the pinned upstream Minisign key, then lock their exact URLs and SHA-256
      digests for checksum-enforced installs.
- [x] Commit a cargo-vet store whose imported audits come only from Mozilla,
      Google, Bytecode Alliance, Embark Studios, and ISRG, with the remaining
      current dependency versions recorded as explicit bootstrap audit debt.
- [x] Make `just vet` run `cargo vet --locked`, so the gate never fetches or
      silently refreshes third-party audit data.
- [x] Run `just vet` in the existing CI supply-chain job and from `just ci-full`.
- [x] Treat changes to audit coverage, exemptions, trust, criteria mappings, and
      tool/install locks or gate wiring as protected supply-chain policy.
- [x] Ensure a PR that changes only `supply-chain/**` or `mise.lock` still runs
      the code gates.

## Non-goals

- No workspace dependency, `Cargo.toml`, `Cargo.lock`, product code, parser, or
  PureCARD behavior changes.
- No claim that a bootstrap exemption is an audit or review of the exempted
  crate's source.
- No self-certification of existing dependencies and no locally authored
  trusted-publisher or wildcard-audit entries. The locked imports retain the
  explicitly approved peers' publisher-bound wildcard coverage described below.
- No duplicate cargo-vet job in the scheduled security workflow; cargo-audit
  remains the monitor for newly disclosed RustSec advisories, while cargo-vet
  checks committed review coverage on every dependency-changing PR.
- No new required status-check name. The existing supply-chain job already
  flows through `no-warnings` and the required `ci-gate` aggregate.

## Design

This is shared repository infrastructure and touches neither product's Cargo
dependency graph. cargo-vet 0.10.2 is the current stable crate release. Its
upstream GitHub binary release lags at 0.10.0, and source installation with the
upstream locked dependency set emits yanked-dependency warnings. The repository
therefore pins the signed 0.10.2 QuickInstall release through mise's supported
GitHub backend. All seven Linux, macOS, and Windows archives were verified
against cargo-quickinstall's Minisign key `6B2490DA9B769EDD`, pinned at upstream
commit `60ad72eb1555ce1648ed30bc7422e4d835362b53` (public-key file SHA-256
`e46888d4cdb645fe475f9fc213a4575f16ba590aed56668bcf6b3d3ec216af0e`).
Because `mise.lock` is project-scoped, it locks all 11 binary tools configured in
`.mise.toml`; cargo-vet's 11 generated platform aliases map to the seven unique
Minisign-verified archives. The lock records each exact archive URL and SHA-256,
and locked installation enforces those reviewed bytes. The release has neither
a GitHub artifact attestation nor SLSA provenance, so this design makes no claim
that mise verifies either channel. The existing mise action caches the installed
tool in CI.

`cargo vet init` creates exact-version exemptions for the current graph. Imports
are then added explicitly from the five named organizations and
`cargo vet regenerate exemptions` minimizes the remaining baseline. The
generated `imports.lock` is committed. CI and the normal local gate use only
`cargo vet --locked`, which reads that snapshot without fetching or refreshing
remote audit data. Cargo metadata can still need the normal Cargo registry
cache/network; a fully offline run additionally requires `--frozen` and a
prepopulated Cargo cache.

The approved peer snapshot contains six publisher records and six wildcard
audits: Bytecode Alliance coverage for `bumpalo`, `wasip2`, and `wit-bindgen`,
and Mozilla coverage for `unicode-segmentation`, `unicode-width`, and
`utf8_iter`. The `wit-bindgen` publisher record and wildcard audit serialize one
trusted-publisher relationship. These are inherited peer assertions, not local
trust declarations. In locked mode cargo-vet can reify them only for publisher
versions already recorded in `imports.lock`; an unseen future version cannot
gain coverage until a protected import-lock refresh is reviewed.

The initial exact-version exemptions are a gate bootstrap, not a gate
weakening: no cargo-vet coverage existed before this change, and a new version
not covered by the committed imports will fail. After bootstrap, adding or
broadening an exemption, adding publisher trust, weakening criteria, remapping
an imported criterion, refreshing the imported publisher snapshot, excluding
imported audits, removing audit coverage, or changing an installer URL/checksum
is a protected loosening that requires explicit maintainer judgment.

## API / contract impact

None. `just vet` is a new repository-maintenance command. The CI supply-chain
job gains one check but keeps its existing aggregate status-check topology.

## Testing plan

- Failing-first: before creating the store, `cargo vet --locked` must fail with
  `cargo vet is not configured`.
- Bootstrap: run `cargo vet init`, import the five approved audit sets, and run
  `cargo vet regenerate exemptions`.
- Gate: `just vet` succeeds without refreshing remote audits and reports the
  committed fully-audited/exempted counts. With a populated Cargo cache,
  `cargo vet --locked --frozen` proves the snapshot itself is offline-capable.
- Non-vacuity: in a temporary copy, remove one current exemption that has no
  imported audit and confirm `just vet` fails naming the unvetted package; do not
  retain the perturbation.
- Config: independently verify every locked archive's Minisign signature and
  digest, then validate a fresh `mise install --locked cargo-vet` and
  `cargo vet --version`; lint workflows and Markdown through their `just`
  targets.
- Full verification: run `just ci` and `just ci-full`, then an independent
  spec-versus-diff review.

## Verification

- Two fresh project-wide `mise lock --yes` passes produced the same 653-line
  lockfile (SHA-256
  `9d13657e0f48d59bc0ff31ed9d5aaa780d38c5768bed7a975f8dbfbbf83d94dd`)
  with 127 resolved platform entries. All seven unique cargo-vet archives were
  independently Minisign-verified before their digests were committed; a fresh
  empty-cache `mise install --locked cargo-vet` checksum-verified and installed
  cargo-vet 0.10.2 without changing the lock.
- `just vet` and `cargo vet --locked --frozen` pass with 59 fully audited and
  169 exempted crates. Removing the exact `ahash` 0.8.12 exemption in an
  isolated copy makes the gate fail and name that missing coverage.
- `just ci`, `just coverage`, `just review`, `just deny`, `just audit`,
  `just machete`, `just release-plz-check`, `just semver`, `just sweep`,
  `just postponed-markers`, `just docs`, `just test-scripts`,
  `just lint-actions`, `just zizmor`, `just check-doc-links`, and
  `git diff --check` pass. Mutation is deliberately deferred because this
  branch changes no product code or dependency graph and is being held for a
  post-#16 rebase; the protected PR gate will run the full mutation lane after
  that rebase.
- An independent spec-versus-diff and generated-lock review found no remaining
  merge blocker.

## Risks & rollout

- Imported audits are direct trust relationships. The import set is deliberately
  limited, locked, code-owned, and reviewed as protected policy. Its six
  publisher-bound wildcard assertions cannot expand to unrecorded versions in
  locked mode.
- The baseline contains exact exemptions because this repository has no prior
  audit program. Their exact versions make dependency updates fail closed while
  the audit debt is reduced over time.
- A third-party prebuilt tool is acceptable only because the seven upstream
  signatures were authenticated before their exact URLs/checksums were locked;
  normal installation can accept only those reviewed bytes.
- Rollback removes a net-new gate and therefore requires explicit maintainer
  approval as a protected loosening.
