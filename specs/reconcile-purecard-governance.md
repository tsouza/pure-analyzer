# Spec: reconcile-purecard-governance

- Status: complete
- Created: 2026-08-26
- Owner: agent (continuation of the user-approved PureCARD fold)

## Problem

PureCARD now lives in the root Cargo workspace, but root governance still reads
as though the repository contains only a mostly implemented analyzer inherited
from a Rust server starter kit. The resulting documentation has three classes of
error:

1. It presents target analyzer behavior as shipped even though lexer and
   diagnostics are the only substantive analyzer layers and CLI subcommands are
   placeholders.
2. It describes model and resolver as sibling analyzer layers even though the
   permitted and intended dependency direction is resolver-to-model.
3. It does not state whether PureCARD is part of the analyzer DAG, who owns its
   corpus and parser concerns, or whether its migrated crate and wheel are
   published products.

The ambiguity makes accidental product coupling look authorized and leaves
contributors without one truthful current-state entry point.

## Goals

- [x] Define the repository as an umbrella with two independent sibling
      products: the early-scaffold `pure-analyzer` and the PureCARD decoder,
      whose M0–M5 code artifacts exist while end-to-end proof obligations remain.
- [x] Record zero analyzer–PureCARD Cargo edges in either direction across every
      dependency kind, with root topology verification enforcing the boundary.
- [x] Ratify ADR-0003 as analyzer-only and correct its processing order to
      `lexer → syntax → parser → model → resolve → analysis → libpure
      → cli`.
- [x] State Cargo direction unambiguously: resolver may depend on model; model
      must not depend on resolver; diagnostics remains an analyzer leaf.
- [x] Accept a root ADR for PureCARD's independent-product placement, shared
      infrastructure boundary, future spec/ADR requirement for parser or corpus
      sharing, and unpublished status.
- [x] Reconcile root onboarding, constitution, domain, design, methodology,
      contribution, and security docs with actual implementation status.
- [x] Restore the optional-gate anchor referenced by testing docs and explain
      how `CODSPEED_ENABLED` and `PUBLIC_API_ENABLED` affect protection.

## Non-goals

- Implementing analyzer parser, model, resolver, analysis, CLI, or LSP behavior.
- Changing PureCARD decoder behavior, public APIs, runtime contracts, corpus, or
  Python boundary. Nested product documentation is reconciled where migration
  made its repository or implementation-status facts stale.
- Sharing parser code, model types, test corpora, or ownership across products.
- Publishing the migrated Rust package or Python wheel.
- Replacing the analyzer design document; it remains the target design after an
  explicit current-status note is added.
- Deleting or archiving the former standalone PureCARD repository.

## Design

### Repository topology

Root governance names three classes of workspace concern:

- **Analyzer product:** the ADR-0003 crate pipeline and future front ends.
- **PureCARD product:** the independently owned constrained decoder at
  `crates/pure-analyzer-purecard/`.
- **Shared infrastructure:** `xtask`, `just`, root CI, toolchain declarations,
  and root policy. Infrastructure may invoke either product but is not a product
  layer and does not create a runtime dependency.

The products have zero dependency edges in either direction. The topology check
combines root Cargo metadata with fail-closed parsing of workspace-excluded
fuzz/lint manifests, so normal, development, build, optional, and renamed edges
cannot bypass the boundary.

### Analyzer truth

All root current-state summaries distinguish implemented capability from target
design. Lexer and diagnostics are substantive. Syntax, parser, model, resolver,
analysis, and `libpure` are mostly version-reporting stubs. CLI subcommands
return `not implemented yet`. Future validate/lint/eq/fmt/LSP contracts remain in
the design document as roadmap scope.

Pipeline arrows describe processing order. When documentation discusses Cargo,
it says that edges point toward prerequisites and calls out
`pure-analyzer-resolve → pure-analyzer-model`; the reverse is forbidden.

### PureCARD truth

Root docs describe PureCARD's M0–M5 code artifacts as implemented while retaining
the documented open end-to-end proof obligations and explicitly avoiding any claim of
feature completeness. They link to its nested product docs for details. PureCARD
is not an analyzer layer or `libpure` front end. Its package stays
`publish = false`, and CI wheel construction is verification only.

Co-location does not authorize shared parser or corpus ownership. A future
integration must begin with a dedicated spec and ADR that define ownership,
stable contract, compatibility, and release effects.

### Documentation precedence

The analyzer design document remains authoritative for intended analyzer
behavior. The root README and domain model describe current implementation
status. Root ADR-0004 and PureCARD ADR-0009 jointly record repository placement;
ADR-0003 remains exclusive to the analyzer DAG.

## Acceptance criteria

- [x] Root README presents both products, current status, the boundary, build
      entry points, and an `Optional gates (off by default)` heading.
- [x] Constitution no longer claims an empty server domain and protects both the
      analyzer ordering and independent-product boundary.
- [x] Domain model distinguishes current implementation truth from target
      analyzer design and models both products plus shared infrastructure.
- [x] Analyzer design starts with an honest implementation-status note and does
      not put PureCARD in its DAG.
- [x] Methodology, testing, security, and contribution docs contain no active
      starter-kit/server claims or nonexistent bolero/OSS-Fuzz posture.
- [x] ADR-0003 is Accepted, analyzer-scoped, and model-before-resolve.
- [x] ADR-0004 is Accepted and records zero edges, infrastructure scope,
      integration decision requirements, and unpublished PureCARD artifacts.
- [x] The analyzer–PureCARD boundary is rejected by the topology gate for all
      Cargo dependency kinds.

## Verification

- Run the repository Markdown lint over every touched root document.
- Run `cargo xtask verify-layering` and its targeted topology tests.
- Search touched root docs for stale `{model, resolve}`, sibling-layer,
  starter-kit/server, missing optional-anchor, and shipped-analyzer claims.
- Run `git diff --check` and inspect the final path list for scope.

## Risks and rollout

The primary risk is replacing one overstatement with another. Present-tense
status is therefore grounded in manifests and crate entry points, while future
behavior remains explicitly labeled target design. The second risk is treating
shared automation as shared product code; the ADRs distinguish orchestration
from runtime ownership and the topology gate checks the resulting zero-edge
rule. No release authority or product behavior changes in this reconciliation.
