# Changelog

All notable changes to this project are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/) and this project adheres to
[Semantic Versioning](https://semver.org/).

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
