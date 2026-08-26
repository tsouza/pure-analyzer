# Vetting: cstree 0.14.0

- **Purpose evaluated:** lossless immutable green tree, navigable syntax nodes,
  parser checkpoints, token interning, and typed syntax support.
- **Version checked:** crates.io 0.14.0, released 2026-04-22.
- **Source checked:** release and current `master` both resolve to
  [`c0c513d5065402305d06b6b2425a150d4da048ed`](https://github.com/domenicquirl/cstree/commit/c0c513d5065402305d06b6b2425a150d4da048ed).
- **Decision:** **REJECT** for the analyzer syntax tree.

## Decisive safety finding

cstree declares `SyntaxNode<S, D>` unconditionally `Send` and `Sync` for every
`D: 'static`:

- [`node.rs` lines 37-38](https://github.com/domenicquirl/cstree/blob/c0c513d5065402305d06b6b2425a150d4da048ed/cstree/src/syntax/node.rs#L37-L38)

The allocation behind that node contains an `RwLock<Option<Arc<D>>>` without a
`D: Send + Sync` bound:

- [`NodeData` lines 258-264](https://github.com/domenicquirl/cstree/blob/c0c513d5065402305d06b6b2425a150d4da048ed/cstree/src/syntax/node.rs#L258-L264)

Resolved roots also store `Arc<dyn Resolver<TokenKey>>` without `Send + Sync` on
the trait object:

- [`Kind::Root` line 241](https://github.com/domenicquirl/cstree/blob/c0c513d5065402305d06b6b2425a150d4da048ed/cstree/src/syntax/node.rs#L240-L247)
- [`new_root_with_resolver` lines 352-355](https://github.com/domenicquirl/cstree/blob/c0c513d5065402305d06b6b2425a150d4da048ed/cstree/src/syntax/node.rs#L352-L355)

Those facts make the safe public API unsound. The problem is not that the crate
contains encapsulated unsafe code; the unsafe auto-trait promises are broader
than the safe constructors and stored fields can uphold.

## Safe-API counterexamples

### Non-thread-safe node data

Choose `D = Rc<Cell<u32>>`. Keep one `Rc` clone on the creating thread, store
another through the node's safe data API, clone the `SyntaxNode`, and move the
node to a spawned thread. The unconditional `Send` implementation permits the
move even though the node reaches the same `Rc<Cell<_>>`. Mutating through the
retained clone and the node-data clone from separate threads violates the
`Cell` and non-atomic `Rc` contracts without any unsafe code in the caller.

The necessary repair is to restrict the relevant syntax-node auto traits and
data APIs to `D: Send + Sync`; an `RwLock` around `Arc<D>` does not make an
arbitrary `D` thread-safe.

### Non-thread-safe resolver

Implement `Resolver<TokenKey>` with state backed by `Rc<RefCell<_>>` and pass it
to `SyntaxNode::new_root_with_resolver`. The constructor requires only
`Resolver<TokenKey> + 'static`. The root stores it behind
`Arc<dyn Resolver<TokenKey>>`, but the returned syntax node is still declared
`Send + Sync`. Sharing or moving the node and resolving token text can therefore
cross a non-`Send`, non-`Sync` resolver between threads through safe calls.

The necessary repair is a `Send + Sync` resolver trait object and matching
constructor bound whenever the containing node claims those auto traits.

## Miri evidence does not close the gap

cstree has explicit threaded tests, but every relevant test is skipped under
Miri:

- [`send` lines 41-43](https://github.com/domenicquirl/cstree/blob/c0c513d5065402305d06b6b2425a150d4da048ed/cstree/tests/it/sendsync.rs#L41-L43)
- [`send_data` lines 61-63](https://github.com/domenicquirl/cstree/blob/c0c513d5065402305d06b6b2425a150d4da048ed/cstree/tests/it/sendsync.rs#L61-L63)
- [`sync` lines 102-104](https://github.com/domenicquirl/cstree/blob/c0c513d5065402305d06b6b2425a150d4da048ed/cstree/tests/it/sendsync.rs#L102-L104)
- [`drop_send` lines 126-128](https://github.com/domenicquirl/cstree/blob/c0c513d5065402305d06b6b2425a150d4da048ed/cstree/tests/it/sendsync.rs#L126-L128)
- [`drop_sync` lines 150-153](https://github.com/domenicquirl/cstree/blob/c0c513d5065402305d06b6b2425a150d4da048ed/cstree/tests/it/sendsync.rs#L150-L153)

Even running those tests under Miri would not prove the generic safe API: their
fixtures use thread-safe data and the crate's own interner. They do not
instantiate either counterexample. Trait bounds are universal promises, so one
safe counterexample is decisive.

## Rubric result and short-circuit

- **License and provenance:** no finding was needed to reject the crate.
- **Maintenance:** the latest release and current branch were checked and are
  the same audited commit, so switching from the release to Git cannot avoid the
  finding.
- **API fit:** the advertised green/red tree, checkpoint, and interning surface
  is directionally relevant, but detailed adaptation work was stopped.
- **Safety and correctness:** hard failure. Safe callers can violate Rust's
  thread-safety contract.
- **Supply chain, performance, and downstream Miri qualification:** not
  completed. Under the repository rubric, later strengths cannot compensate
  for a safe-API soundness failure.

This deliberate short-circuit avoids producing misleading benchmark, Miri, or
ergonomic evidence for a dependency that is already ineligible for the
load-bearing role.

## Revisit conditions

Reconsideration requires all of the following in a released version:

1. `SyntaxNode` auto-trait implementations carry sound bounds for node data and
   every stored resolver.
2. Safe compile-fail or trait tests reject `Rc<Cell<_>>` data and
   `Rc<RefCell<_>>` resolvers from crossing threads.
3. Threaded data, resolver, traversal, and drop tests run under Miri rather than
   being skipped, or an upstream limitation is paired with equivalent local
   qualification.
4. This repository reruns source audit, exact-version Miri/thread tests, API-fit
   proof, and the remaining dependency rubric from the beginning.

Until then, cstree must not be added directly, indirectly, or through its derive
crate for the analyzer syntax tree.
