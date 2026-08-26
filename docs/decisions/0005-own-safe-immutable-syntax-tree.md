# 0005. Own a safe immutable syntax-tree foundation

- **Status:** Accepted
- **Date:** 2026-08-26
- **Deciders:** Project maintainers

## Context

The analyzer processing pipeline in ADR-0003 requires a lossless syntax layer
between the lexer and parser. The workspace previously declared Rowan and had
named cstree as a possible alternative. This tree is a load-bearing trust
boundary: every parser event, source byte, typed AST view, formatter traversal,
and later multi-file analysis passes through it. The workspace forbids unsafe
code in its own crates and requires malformed inputs to return typed errors
rather than panic.

Rowan does not clear that dependency bar. The audited 0.17.0 source still has an
unsafe cursor and custom reference-counting implementation, while upstream
issues [#108](https://github.com/rust-analyzer/rowan/issues/108),
[#163](https://github.com/rust-analyzer/rowan/issues/163), and
[#192](https://github.com/rust-analyzer/rowan/issues/192) remain open with Miri
undefined-behavior reports. The proposed broad repair
[#211](https://github.com/rust-analyzer/rowan/pull/211) is open and unmerged.
This is not a claim that Rowan has a RustSec advisory; it is a source- and
upstream-evidence rejection under this repository's load-bearing safety rubric.

cstree 0.14.0 and its current `master` are both commit
[`c0c513d`](https://github.com/domenicquirl/cstree/commit/c0c513d5065402305d06b6b2425a150d4da048ed).
Its public `SyntaxNode<S, D>` is unconditionally declared `Send + Sync` even
though node data can contain `D` without `D: Send + Sync`, and a resolved root
can store a resolver accepted without `Send + Sync` bounds. Safe callers can
therefore move `Rc<Cell<_>>` node data or an `Rc<RefCell<_>>` resolver across
threads. That makes the safe API unsound; it is not merely an internal use of
unsafe that needs more tests.

Safety is the first hard dependency gate. Once a safe public-API counterexample
exists, adoption, downstream Miri qualification, performance comparison, and
detailed API-fit proof cannot change the outcome and are deliberately
short-circuited. The complete evidence is recorded in
[`rowan.md`](../dependencies/rowan.md) and
[`cstree.md`](../dependencies/cstree.md).

## Decision

The workspace owns a deliberately small, immutable concrete-syntax-tree foundation in
`pure-analyzer-syntax`, implemented entirely in safe Rust. It consists of:

- an exhaustive syntax-kind superset of lexer terminals, with private raw
  representation and checked decoding;
- immutable `Arc`-backed tokens, elements, and nodes;
- a validated `Open`/`Advance`/`Close` event builder with builder-bound
  checkpoints and retroactive node opening;
- exact reuse of lexer `text_size::TextRange` values and lossless source
  re-emission;
- typed errors for malformed ranges, UTF-8 boundaries, event balance, token
  consumption, and root structure; and
- minimal manually written `AstNode`, `Root`, and `BinaryExpression` views.

The syntax crate may depend directly on lexer, `text-size`, and `thiserror`, all
consistent with ADR-0003 and existing dependency decisions. It gains no
PureCARD dependency. Rowan, cstree, num-derive, num-traits, and ungrammar are not
part of this foundation.

## Alternatives considered

- **Adopt Rowan 0.17.0.** Rejected because multiple open Miri UB reports affect
  its load-bearing cursor, green-tree, and reference-counting paths, and the
  proposed repair is not merged or released.
- **Adopt cstree 0.14.0.** Rejected because two safe-API counterexamples cross
  non-thread-safe state through its unconditional unsafe `Send + Sync`
  declarations. Miri cannot repair an invalid public trait contract.
- **Carry an upstream fork or local patch.** Rejected because that makes this
  project the maintainer of a large unsafe pointer/reference-counting boundary,
  which is more complex and riskier than the syntax surface currently needed.
- **Wait for an upstream release.** Rejected because it blocks the analyzer
  parser while the required initial tree is small and can be expressed with
  standard safe ownership.
- **Generate the complete typed AST with ungrammar now.** Rejected because the
  parser grammar has not landed. Hand-writing two wrappers proves the contract
  without freezing or generating a speculative taxonomy.

## Consequences

- The owned tree derives `Send + Sync` only from safe field types; compile-time
  assertions and real owned/borrowed threaded traversals pin that property.
- Parser construction can start now against a stable event/checkpoint contract,
  while invalid events and ranges fail with data errors rather than process
  failure.
- The repository owns traversal ergonomics, memory measurements, and future AST
  wrapper expansion. The first implementation intentionally omits mutable red
  cursors, parent links, interning, and incremental edits.
- Raw-kind and lexer-kind additions require explicit syntax-layer accounting;
  there is no unchecked discriminant conversion.
- Rowan or cstree may be reconsidered only through a new decision after the
  cited issues are fixed in a released version, the exact release passes this
  repository's source audit and Miri/threaded qualification, and its safe API
  proves the required auto-trait bounds. A benchmark or ergonomic advantage
  alone is not a revisit condition.
