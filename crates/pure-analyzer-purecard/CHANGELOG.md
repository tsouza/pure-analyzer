# Changelog

All notable changes to this project are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/) and this project adheres to
[Semantic Versioning](https://semver.org/).

## [0.4.2](https://github.com/tsouza/pure-analyzer/compare/purecard-v0.4.1...purecard-v0.4.2) - 2026-09-04

### Other

- *(purecard)* sweep the L2 overlay for a value-shape soundness gap ([#394](https://github.com/tsouza/pure-analyzer/pull/394))

## [0.4.1](https://github.com/tsouza/pure-analyzer/compare/purecard-v0.4.0...purecard-v0.4.1) - 2026-09-03

Patch: fixes a live regression shipped in 0.4.0.

### Fixed

- *(purecard)* with a class's `temporal` field set, the milestoning-arity
  narrowing at `Class.all(...)`/a milestoned property call masked a real
  date/date-time literal right after its own first digit — only `%latest`
  survived the arity gate, so a dated as-of query was unwritable while
  `temporal` was set (issue
  [#391](https://github.com/tsouza/pure-analyzer/issues/391),
  [#393](https://github.com/tsouza/pure-analyzer/pull/393))

## [0.4.0](https://github.com/tsouza/pure-analyzer/compare/purecard-v0.3.1...purecard-v0.4.0) - 2026-09-03

Milestoning-arity narrowing, closing out the last major error class the
downstream NL-to-Pure consumer's coverage sweep surfaced. Minor bump (not
patch): the schema contract gains a genuinely new, backward-compatible
capability. No breaking changes — `State`/`L2Position` were already
`#[non_exhaustive]`/`#[doc(hidden)]`, and every existing schema JSON blob
keeps deserializing unchanged (`temporal` is `#[serde(default)]`).

### Added

- *(purecard)* an optional per-class `temporal` field in the L2 schema
  contract (`"business"`, `"processing"`, or `"bitemporal"`), and a
  narrowing rule for the pipeline source's own `Class.all(...)` call: once
  a class's milestoning stereotype is known, the number of comma-separated
  date arguments is narrowed to exactly what that stereotype requires
  (issue [#384](https://github.com/tsouza/pure-analyzer/issues/384),
  [#387](https://github.com/tsouza/pure-analyzer/pull/387))
- *(purecard)* the same arity rule at a milestoned property/association
  navigation's own call (`$x.prop(%latest)`), keyed off the navigated-to
  class rather than the pipeline source (issue
  [#386](https://github.com/tsouza/pure-analyzer/issues/386),
  [#389](https://github.com/tsouza/pure-analyzer/pull/389))

### Fixed

- *(purecard)* admit a `$`-variable at a milestoned property navigation's
  own date-argument call, the sibling of the milestoning-source fix
  shipped in 0.3.1 (issue
  [#385](https://github.com/tsouza/pure-analyzer/issues/385),
  [#388](https://github.com/tsouza/pure-analyzer/pull/388))

## [0.3.1](https://github.com/tsouza/pure-analyzer/compare/purecard-v0.3.0...purecard-v0.3.1) - 2026-09-03

Six more fixes for gaps a downstream NL-to-Pure consumer's grammar/schema
coverage sweep surfaced, all live-verified against the pinned
`finos/legend-engine-server:4.113.0` engine. No breaking changes — `State`
was already `#[non_exhaustive]` (since 0.3.0), so every new variant added
below is additive.

### Fixed

- *(purecard)* admit a `$`-variable as the milestoning date argument of
  `all(...)`, alongside the already-admitted `%`-literal forms (issue
  [#367](https://github.com/tsouza/pure-analyzer/issues/367),
  [#373](https://github.com/tsouza/pure-analyzer/pull/373))
- *(purecard)* close arm-R's first `~`-column colon against a typed binder
  — `over(~[k: t::A[*]|$k.k])`-shaped lambda columns now correctly die
  (issue [#368](https://github.com/tsouza/pure-analyzer/issues/368),
  [#376](https://github.com/tsouza/pure-analyzer/pull/376))
- *(purecard)* require a call after every `->`-step target name — a bare
  `->name->` pipeline step (the dominant real-world rejection shape under
  live sampling) is no longer admitted (issue
  [#369](https://github.com/tsouza/pure-analyzer/issues/369),
  [#375](https://github.com/tsouza/pure-analyzer/pull/375))
- *(purecard)* admit a window aggregation's own reducer binder (`$y` in
  `{p,w,r|...}:y|$y->average()`), previously masked as unbound even though
  L1 already admitted it (issue
  [#377](https://github.com/tsouza/pure-analyzer/issues/377),
  [#380](https://github.com/tsouza/pure-analyzer/pull/380))
- *(purecard)* narrow a `let` binder's own classpath value against the real
  schema, closing a fabricated-class-path gap N3 left open there (issue
  [#371](https://github.com/tsouza/pure-analyzer/issues/371),
  [#382](https://github.com/tsouza/pure-analyzer/pull/382))
- *(purecard)* close arm-R's second `~`-column colon (`relAggSpec`/
  `winAggSpec`'s reducer separator) against a typed binder, the same
  over-admission #368 closed at the first colon (issue
  [#372](https://github.com/tsouza/pure-analyzer/issues/372),
  [#381](https://github.com/tsouza/pure-analyzer/pull/381))

## [0.3.0](https://github.com/tsouza/pure-analyzer/compare/purecard-v0.2.2...purecard-v0.3.0) - 2026-09-03

<!--
Not release-plz-generated: the version bump landed inside #364's own PR
(needed to satisfy `cargo semver-checks` against the already-published 0.2.2
baseline before merge — see that PR for why), so release-plz's automated
`release-pr` step found nothing left to bump and went straight to tagging and
publishing the already-committed 0.3.0 without ever writing this entry.
Backfilled by hand instead, following the same pattern as the 0.2.1/0.2.2
entries above. The first backfill (this PR's predecessor) missed #356/#351,
which merged before #360 but after 0.2.2 was cut — issue #366 caught the
omission.
-->

Five fixes for gaps a downstream NL-to-Pure consumer's grammar/schema
coverage sweep surfaced, all live-verified against the pinned
`finos/legend-engine-server:4.113.0` engine.

### Added

- *(purecard)* admit scalar/date initializers in `let` bindings, not only
  pipelines (issue [#352](https://github.com/tsouza/pure-analyzer/issues/352),
  [#360](https://github.com/tsouza/pure-analyzer/pull/360))

### Fixed

- *(purecard)* stop N7 masking a brace lambda's 3rd-and-on binder comma — the
  byte-PDA has no arity cap on a binder list, so `{x,y,z|…}`'s later binders
  reached the same anchor an ordinary value opens at, and L2's N7 rule masked
  them like one (issue
  [#351](https://github.com/tsouza/pure-analyzer/issues/351),
  [#356](https://github.com/tsouza/pure-analyzer/pull/356))
- *(purecard)* re-anchor L2 trie narrowing past a fused leading-whitespace run
  — a byte-BPE vocabulary that spells a phantom member with a leading space
  (`$x. zzz`) previously bypassed L2 narrowing entirely (issue
  [#353](https://github.com/tsouza/pure-analyzer/issues/353),
  [#362](https://github.com/tsouza/pure-analyzer/pull/362))
- *(purecard)* narrow member access after a class-typed lambda binder whose
  annotation resolves in the schema, rather than admitting every name (issue
  [#354](https://github.com/tsouza/pure-analyzer/issues/354),
  [#363](https://github.com/tsouza/pure-analyzer/pull/363))
- *(purecard)* **[breaking]** narrow arm-R `~`-column positions
  (`over(...)`, `rename(...)`, `sort(...)`, `groupBy(...)`) to exclude a
  lambda column L1 previously over-admitted there — `Frame` is now
  `#[non_exhaustive]` (mirroring `State`), which is itself the breaking
  change forcing this minor version bump: adding `Frame::RelColBracket`
  without it would have been a breaking `enum_variant_added`, and marking a
  previously-exhaustive public enum `#[non_exhaustive]` for the first time
  is a breaking change in its own right, caught by `cargo semver-checks`
  against the live crates.io baseline (issue
  [#361](https://github.com/tsouza/pure-analyzer/issues/361),
  [#364](https://github.com/tsouza/pure-analyzer/pull/364))

## [0.2.2](https://github.com/tsouza/pure-analyzer/compare/purecard-v0.2.1...purecard-v0.2.2) - 2026-09-02

No functional changes to `purecard` itself. `pyproject.toml`'s `dynamic`
list only ever named `version`, so — per maturin's own contract, listing
`[project]` at all means it "is not allowed to populate fields that are not
present in `project.dynamic`" — every other field maturin can lift from
`Cargo.toml` (description, README, license, authors, keywords, project
URLs) was built into the wheel empty. 0.2.1's PyPI listing had no
description, no rendered README, nothing (verified via
`pypi.org/pypi/purecard/json` and reproduced locally: the built wheel's
`dist-info/METADATA` held only `Name`/`Version`/`Requires-Python`). Fixed
`dynamic` to include the rest, added a `homepage` pointing at this crate's
own subtree rather than the two-product monorepo root, and confirmed
`twine check` passes clean on the rebuilt wheel. crates.io's 0.2.1 listing
is unaffected (its own metadata was already complete) and stays published,
in place; PyPI publishes only from this version onward.

## [0.2.1](https://github.com/tsouza/pure-analyzer/compare/purecard-v0.2.0...purecard-v0.2.1) - 2026-09-02

No functional changes to `purecard` itself. Re-cut purely to carry a release-
tooling fix through to a published PyPI wheel: 0.2.0's release build put a
stray, non-manylinux-tagged `linux_x86_64` wheel into the same output
directory as the real x86_64 Linux wheel (see #344), which PyPI's upload
validator rejects outright. crates.io already has 0.2.0 (unaffected by the
bug — wheels are a PyPI-only concern) and it stays published, in place; PyPI
publishes only from this version onward.

## [0.2.0](https://github.com/tsouza/pure-analyzer/releases/tag/purecard-v0.2.0) - 2026-09-02

<!--
The usual release-plz `compare/purecard-v0.1.0...purecard-v0.2.0` link 404s:
0.1.0 was published from the standalone pre-migration repo (see
docs/decisions/0006-purecard-resumes-publication.md), so no
`purecard-v0.1.0` tag exists here to compare against. Linked to the tag this
release creates instead.
-->

### Added

- *(purecard)* publish to crates.io and PyPI as `purecard` ([#329](https://github.com/tsouza/pure-analyzer/pull/329))
- *(purecard)* narrow the extent-method argument and the non-scalar operand ([#55](https://github.com/tsouza/pure-analyzer/pull/55)) ([#172](https://github.com/tsouza/pure-analyzer/pull/172))
- *(purecard)* deny relation builtins on a scalar receiver ([#55](https://github.com/tsouza/pure-analyzer/pull/55)) ([#170](https://github.com/tsouza/pure-analyzer/pull/170))
- *(purecard)* bind the classpath separator to a name or a string literal ([#55](https://github.com/tsouza/pure-analyzer/pull/55)) (re-ship of #153) ([#158](https://github.com/tsouza/pure-analyzer/pull/158))
- *(purecard)* bind the classpath separator to a name or a string literal ([#55](https://github.com/tsouza/pure-analyzer/pull/55)) ([#153](https://github.com/tsouza/pure-analyzer/pull/153))
- *(purecard)* spell the date literal, the group and the binder pipe as the engine does ([#55](https://github.com/tsouza/pure-analyzer/pull/55)) ([#145](https://github.com/tsouza/pure-analyzer/pull/145))
- *(purecard)* narrow string methods by the receiver's fixed type ([#116](https://github.com/tsouza/pure-analyzer/pull/116)) ([#143](https://github.com/tsouza/pure-analyzer/pull/143))
- *(purecard)* mask ordered comparators on a non-scalar navExpr ([#116](https://github.com/tsouza/pure-analyzer/pull/116)) ([#144](https://github.com/tsouza/pure-analyzer/pull/144))
- *(purecard)* spell the % literals and the typed binder as the engine does ([#55](https://github.com/tsouza/pure-analyzer/pull/55)) ([#141](https://github.com/tsouza/pure-analyzer/pull/141))
- *(purecard)* narrow the completed term's arity and operator set ([#55](https://github.com/tsouza/pure-analyzer/pull/55)) ([#138](https://github.com/tsouza/pure-analyzer/pull/138))
- *(purecard)* narrow the class extent's method by receiver category ([#55](https://github.com/tsouza/pure-analyzer/pull/55)) ([#137](https://github.com/tsouza/pure-analyzer/pull/137))
- *(purecard)* tighten L1 at the name/frame boundaries and fix the store call shape ([#55](https://github.com/tsouza/pure-analyzer/pull/55)) ([#135](https://github.com/tsouza/pure-analyzer/pull/135))
- *(purecard)* narrow the pipeline-source continuation ([#55](https://github.com/tsouza/pure-analyzer/pull/55)) ([#134](https://github.com/tsouza/pure-analyzer/pull/134))
- *(purecard)* make completion mask-aware and narrow bare value words ([#55](https://github.com/tsouza/pure-analyzer/pull/55)) ([#133](https://github.com/tsouza/pure-analyzer/pull/133))
- *(purecard)* narrow refVar names and classpath continuation ([#55](https://github.com/tsouza/pure-analyzer/pull/55)) ([#131](https://github.com/tsouza/pure-analyzer/pull/131))
- *(purecard)* measure the live compile rate per walk partition and per db ([#55](https://github.com/tsouza/pure-analyzer/pull/55)) ([#130](https://github.com/tsouza/pure-analyzer/pull/130))
- *(purecard)* add string-equality filter+project recipe ([#129](https://github.com/tsouza/pure-analyzer/pull/129))
- *(purecard)* add a live-verified groupBy+restrict recipe walk ([#125](https://github.com/tsouza/pure-analyzer/pull/125))
- *(purecard)* add groupBy HAVING+restrict recipe ([#128](https://github.com/tsouza/pure-analyzer/pull/128))
- *(purecard)* add a scalar multi-metric groupBy recipe ([#55](https://github.com/tsouza/pure-analyzer/pull/55)) ([#126](https://github.com/tsouza/pure-analyzer/pull/126))
- *(purecard)* bias random exploration toward numeric members after binder navigation ([#124](https://github.com/tsouza/pure-analyzer/pull/124))
- *(purecard)* add a real, compilable groupBy aggregation recipe ([#123](https://github.com/tsouza/pure-analyzer/pull/123))
- *(purecard)* extend recipe walks to the live-compile-rate eager generator ([#122](https://github.com/tsouza/pure-analyzer/pull/122))
- *(purecard)* add grammar-shape-aware recipe walks for class-member navigation ([#121](https://github.com/tsouza/pure-analyzer/pull/121))
- *(purecard)* bias schema-walker toward class-member navigation ([#120](https://github.com/tsouza/pure-analyzer/pull/120))
- *(purecard)* expose L2 rule identity for per-named-rule coverage ([#118](https://github.com/tsouza/pure-analyzer/pull/118))
- *(purecard)* ship T3 aggregation-reducer type rule for L2 ([#115](https://github.com/tsouza/pure-analyzer/pull/115))
- *(purecard)* drive real-model inference through PureCARD and Legend ([#111](https://github.com/tsouza/pure-analyzer/pull/111))
- *(purecard)* ship T2 ordered-comparator restriction for L2 ([#112](https://github.com/tsouza/pure-analyzer/pull/112))
- *(purecard)* extract schema_walker into its own crate for fuzz sharing ([#109](https://github.com/tsouza/pure-analyzer/pull/109))
- *(purecard)* derive the store grammar arm-A compilation needs for issue 55 ([#90](https://github.com/tsouza/pure-analyzer/pull/90))
- *(purecard)* mask phantom arguments to the source method's all() call ([#89](https://github.com/tsouza/pure-analyzer/pull/89))
- *(purecard)* narrow the pipeline-source dot to exactly all() (S1) ([#86](https://github.com/tsouza/pure-analyzer/pull/86))
- *(purecard)* [**breaking**] lower supplied grammar specs into the production PDA ([#79](https://github.com/tsouza/pure-analyzer/pull/79))
- *(purecard)* fold PureCARD into the umbrella workspace ([#11](https://github.com/tsouza/pure-analyzer/pull/11))
- migrate purecard into the workspace as pure-analyzer-purecard ([#5](https://github.com/tsouza/pure-analyzer/pull/5))

### Fixed

- *(purecard)* let a zero-step pipeline end where its grammar ends it ([#326](https://github.com/tsouza/pure-analyzer/pull/326))
- *(purecard)* keep the L2 mask live at every reachable position ([#275](https://github.com/tsouza/pure-analyzer/pull/275)) ([#324](https://github.com/tsouza/pure-analyzer/pull/324))
- *(purecard)* [**breaking**] gate stale self-description in CI and drop the unread eos_id ([#323](https://github.com/tsouza/pure-analyzer/pull/323))
- *(purecard)* make the parked real-model lane fail visibly and correct drifted doc claims ([#322](https://github.com/tsouza/pure-analyzer/pull/322))
- *(purecard)* stop the walker trusting is_complete() at a forced identifier ([#88](https://github.com/tsouza/pure-analyzer/pull/88))
- *(deps)* use patched pytest without raising Python floor ([#14](https://github.com/tsouza/pure-analyzer/pull/14))

### Other

- *(purecard)* attest the zero-step pipeline against the live engine ([#330](https://github.com/tsouza/pure-analyzer/pull/330))
- *(ci)* remove CodSpeed integration ([#181](https://github.com/tsouza/pure-analyzer/pull/181))
- Revert "feat(purecard): bind the classpath separator to a name or a string literal ([#55](https://github.com/tsouza/pure-analyzer/pull/55)) ([#153](https://github.com/tsouza/pure-analyzer/pull/153))" ([#157](https://github.com/tsouza/pure-analyzer/pull/157))
- *(purecard)* retire T7 as falsified by the engine ([#116](https://github.com/tsouza/pure-analyzer/pull/116)) ([#146](https://github.com/tsouza/pure-analyzer/pull/146))
- *(purecard)* gate every L2 rule on having a frozen fixture ([#55](https://github.com/tsouza/pure-analyzer/pull/55)) ([#136](https://github.com/tsouza/pure-analyzer/pull/136))
- *(purecard)* flag T6's fixture-corpus evidence gap ([#114](https://github.com/tsouza/pure-analyzer/pull/114))
- *(purecard)* prove per-PDA-state coverage of the schema-walk corpus ([#96](https://github.com/tsouza/pure-analyzer/pull/96))
- *(purecard)* assert the running Legend engine matches its pinned version ([#91](https://github.com/tsouza/pure-analyzer/pull/91))
- *(purecard)* prove schema-walk construct and frame coverage ([#59](https://github.com/tsouza/pure-analyzer/pull/59)) ([#85](https://github.com/tsouza/pure-analyzer/pull/85))
- *(purecard)* wire up live Legend compile-rate proof, fix walker gaps it found ([#84](https://github.com/tsouza/pure-analyzer/pull/84))
- *(purecard)* property-test schema-walk L2 mask containment ([#81](https://github.com/tsouza/pure-analyzer/pull/81))
- *(purecard)* generate deterministic schema-aware accepting walks ([#80](https://github.com/tsouza/pure-analyzer/pull/80))
- remove checked-in work ledger ([#70](https://github.com/tsouza/pure-analyzer/pull/70))
- *(purecard)* add bounded PDA reachability ([#19](https://github.com/tsouza/pure-analyzer/pull/19))
- *(purecard)* decompose PDA state transitions ([#15](https://github.com/tsouza/pure-analyzer/pull/15))
