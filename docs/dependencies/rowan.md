# Vetting: Rowan 0.17.0

- **Purpose evaluated:** the lossless green tree, navigable syntax cursor,
  parser builder, and typed-language foundation named by the analyzer design.
- **Versions checked:** the stale workspace declaration was 0.16.1; the audit
  also checked current crates.io 0.17.0 at source commit
  [`c781c336f44a28210fefaa4d86b8c9902b781c8e`](https://github.com/rust-analyzer/rowan/commit/c781c336f44a28210fefaa4d86b8c9902b781c8e).
- **Decision:** **REJECT** for the analyzer syntax tree until the upstream
  safety work is merged, released, and independently qualified.

## Decisive safety evidence

Rowan's cursor module is an intentionally concentrated unsafe boundary. The
audited release describes it as "utterly and horribly unsafe," says the API is
believed sound in principle but the implementation may have bugs, and maintains
custom transient reference counts and pointers into green nodes:

- [`cursor.rs` lines 17-39](https://github.com/rust-analyzer/rowan/blob/c781c336f44a28210fefaa4d86b8c9902b781c8e/src/cursor.rs#L17-L39)
- [`cursor.rs` lines 61-82](https://github.com/rust-analyzer/rowan/blob/c781c336f44a28210fefaa4d86b8c9902b781c8e/src/cursor.rs#L61-L82)

Unsafe internals are not automatically disqualifying, but this boundary has
unresolved upstream undefined-behavior evidence in paths the analyzer would
load-bear:

- [#108: Miri UB with `-Zmiri-track-raw-pointers`](https://github.com/rust-analyzer/rowan/issues/108)
  — open at the 2026-08-26 audit.
- [#163: Undefined Behavior](https://github.com/rust-analyzer/rowan/issues/163)
  — open at the audit.
- [#192: Miri UB in Rowan `Arc::drop`](https://github.com/rust-analyzer/rowan/issues/192)
  — open at the audit.
- [#211: repair Miri UB in Arc, ThinArc, green types, and cursor](https://github.com/rust-analyzer/rowan/pull/211)
  — open and unmerged at the audit; head
  `dcbece400019397b97764070435eba62c7aa5336`.

The proposed repair's scope reaches the custom Arc, ThinArc, green types, and
cursor together. That is the same ownership and traversal substrate this
project would rely on, not an unused optional feature. Pinning the current
release would knowingly adopt a version predating that unmerged repair.

## Rubric result and short-circuit

- **Reputation and fit:** Rowan is established and its lossless green-tree/event
  model is a close conceptual fit. Reputation cannot override current safety
  evidence.
- **Maintenance:** the relevant reports remain open, and no released version
  contains the proposed broad repair.
- **Safety and correctness:** hard failure for a load-bearing tree. Open Miri UB
  reports exist in the ownership/build/traversal boundary the analyzer needs.
- **Supply chain, performance, detailed API adaptation, and local Miri
  qualification:** deliberately stopped after source and upstream triage. A
  downstream green Miri sample could only show that one sample did not trigger
  the known defects; it could not invalidate the open counterexamples.

Safety is the first dependency gate. This short-circuit is intentional: doing
benchmarks or generating typed wrappers on top of an ineligible release would
create sunk-cost pressure and misleading evidence, not confidence.

## Why a fork is not the answer

Carrying #211 or inventing another patch would make this repository responsible
for Rowan's custom Arc, ThinArc, cursor pointers, mutation invariants, and green
layout. That unsafe maintenance surface is substantially larger than the
analyzer's current need for immutable tokens/nodes, event folding, checkpoints,
and typed views. A small safe owned tree is the lower-risk implementation.

## Revisit conditions

Rowan may be reconsidered only after:

1. the cited UB issues are resolved with an upstream explanation of the repaired
   invariants;
2. the repair is merged and published in a stable crates.io release;
3. this repository audits that exact release rather than a proposed patch;
4. exact-version Miri, malformed-builder, clone/drop, traversal, and threaded
   qualification passes locally; and
5. the remaining supply-chain, API-fit, and performance rubric is completed
   without weakening the owned tree's checked raw-kind and panic-free contracts.

This decision does not assert a RustSec advisory. It records why unresolved
upstream Miri evidence is sufficient to reject Rowan for this particular
load-bearing role.
