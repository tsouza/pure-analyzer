# Spider structural schemas

158 Spider database schemas in PureCARD's L2 wire format (`{db_id, db_path,
classes, associations, enums}` — the same shape `Schema::from_json` ingests and the
eight canonical `tests/fixtures/schemas` use). They give the L2 structural gate
broad schema variety — hundreds of real class/member/association shapes rather than
the eight hand-picked fixtures.

## What they are for

`tests/spider_corpus_replay.rs` *derives* its cases from these schemas rather than
vendoring a redundant case list: every soundness and precision case is a pure
function of a schema's classes, members, and associations (see
`tests/support/spider_corpus.rs`). The gate replays each derived case through a
schema-aware `DecoderSession` and asserts absolute soundness (no real member,
navigation, or legal fused nav-dot is ever masked) plus documented precision (every
phantom is masked, except the two tracked over-approximation classes).

## Provenance and the one caveat you must know

Derived upstream from the Spider `tables.json` set (table → PascalCase class, column
→ camelCase property; class paths `spider::<db>::model::default::<Class>`). The
eight databases that also ship as `tests/fixtures/schemas` are intentionally **not**
duplicated here — those fixtures carry real column types and stay the source of
truth for type-dependent (T1) tests.

**Types were not preserved.** Every property in every schema here is typed
`String[0..1]`; the real Spider column types (Integer, Float, Boolean, …) were lost
in the upstream derivation. These schemas are therefore faithful for **structural**
rules only — N1 member existence, N2 chained navigation, N3 source-class existence,
and the fused nav-dot narrowing — and for the one type case an all-`String` schema
still expresses soundly (a numeric literal against a `String` column, T1-mask). The
real-typed rules (a string literal against a numeric column, and the rest of T1–T7)
stay on the canonical fixtures. Do not read a property type here as ground truth.
