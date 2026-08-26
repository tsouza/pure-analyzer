# PureCARD Spec — correctness, corpus & engine

_Part of the [PureCARD spec](README.md); see also the [domain model](../domain-model.md)._

> The layered testing pyramid that operationalizes this strategy is in
> [../methodology/decoder-testing.md](../methodology/decoder-testing.md).

## 8. Correctness — the oracle-driven test strategy

A constrained decoder can fail silently: a **soundness** bug masks a continuation
that a valid query needs, while a **completeness** bug admits a path that the real
compiler cannot accept. The repository has strong offline evidence for the first
class. The live end-to-end evidence for the second remains incomplete and is
called out explicitly below.

### 8.1 Soundness — never mask a valid continuation

The committed oracle inputs live under `corpus/`:

- `gold_queries.jsonl` contains 5,034 execution-verified queries across 161
  databases;
- `modern_dialect_seeds.jsonl` carries provenance-distinct newer constructs; and
- `schemas/*.md` supplies the fixtures used by the implemented L2 subset.

The always-on Rust lanes replay those query bytes through the hand-written PDA
and assert that the recognizer never dead-states and ends accepting. L2 replay
adds the matching schema and asserts that the implemented N/T rules never mask a
gold continuation. The relevant tests include `soundness_replay.rs`,
`modern_dialect_soundness.rs`, `l2_soundness.rs`, and the BPE/fused-token
regression suites.

`tests/qwen_soundness.rs` is the stronger tokenizer-specific lane. It loads the
actual pinned Qwen tokenizer, builds `Vocab` in token-ID order, tokenizes every
gold query, and checks each actual next token ID against `allowed_mask()`. It
runs via `just qwen-oracle` and the scheduled/on-demand tokenizer workflow, not
the per-PR lane. This proves real-tokenizer token-ID replay. It does **not** run a
model forward pass and does **not** compile output with Legend.

### 8.2 Completeness — the live differential target

The target is to generate under constraint and compile every result through the
pinned Legend engine:

```text
http://localhost:6300/api
/pure/v1/grammar/grammarToJson/lambda
/pure/v1/compilation/lambdaReturnType
```

The current `legend_completeness.rs` lane health-waits the live engine, posts the
committed placeholder protocol fixtures, and classifies every response. It also
feeds every deterministic accepting walk through that live path. It proves
engine reachability, teardown-safe orchestration, and response classification;
it does **not** prove the target 100% compile rate. Raw-walk `grammarToJson`
lowering and schema-constrained walk generation remain outstanding.

### 8.3 Schema-consistency verification (L2)

The implemented L2 subset is covered hermetically by gold replay, targeted
precision/counterfactual tests, property tests, and the real-Qwen token-ID lane.
The end-state target remains zero phantom-identifier and type-mismatch compile
errors for schema-constrained accepting walks against live Legend. That live
claim is not yet established because the schema-aware generator is missing.

### 8.4 Differential fuzzing

Committed-seed accepting walks test recognizer liveness and reproducibility;
near-miss and structured-corpus tests probe mask precision. The excluded nightly
fuzz crate exercises decoder entry points. A live compiler verdict for every
generated walk remains part of §8.2's open target.

### 8.5 Property tests

`mask_properties.rs` and `l2_properties.rs` use `proptest` to exercise rollback,
mask/accept agreement, and the invariant that L2 only narrows L1. Seeds are
reported and regressions are committed.

### 8.6 Corpus-derivation invariant

Any production or narrowing rule a gold query violates is wrong and must be
relaxed. A construct absent from the frozen corpus requires a provenance-bearing
seed before the PDA is widened. The oracle inputs, not intuition, bound changes.

### 8.7 Enforced gates and acceptance target

The enforced hermetic gates include full byte-level corpus replay, the
implemented L2 fixture replay, properties/precision regressions, doc facts, and
normal workspace quality checks. Real-Qwen token-ID replay is scheduled and
on-demand because it is heavy and network-fed.

The **acceptance target**, not a currently satisfied pre-merge gate, is 100%
constrained-walk compilation plus zero schema-resolution/type errors on a
representative schema set. Do not report that target as achieved until
grammar-to-protocol lowering and schema-constrained walk generation make the live
Legend assertion real.

---

## 13. Test corpus — contents, provenance, location

The oracle-driven test strategy of §8 uses two concrete inputs: a large set of
execution-verified gold Pure queries (the **soundness** oracle) and per-database
schemas (the **L2** test inputs). Both ship under this package's `corpus/`
directory in the monorepo. A fresh checkout is sufficient for the byte-level
soundness backbone; no live engine is needed. This section documents what is in
`corpus/`, where it came from, and how to extend it.

### 13.1 `corpus/gold_queries.jsonl` — the soundness oracle

**5,034 unique, execution-verified gold Pure query strings** spanning **161
databases**. This is the frozen oracle of §8.1: replay every query through L1 and
assert that each byte continuation remains live and the complete query is
accepted. Token-ID replay is a separate tokenizer-specific lane. The file is
also the empirical basis from which the hand-written grammar was derived.

**Byte-level soundness testing over this file is fully offline — no Legend
engine or model tokenizer is required.** The real-Qwen token-ID lane additionally
needs the pinned tokenizer artifact, fetched cache-first by its scheduled or
on-demand workflow.

Provenance: distilled from the upstream **pure-lingua** project's Phase-2 output
— `data/phase2/armA_*.jsonl` + `data/phase2/armC_*.jsonl`, keeping only
`accepted=true` (execution-verified) records and de-duplicating query strings.
The full `data/phase2/` directory is **231 MB**; this distillation is **4.8 MB**
and is committed in the PureCARD package subtree.

Line schema (JSONL, one gold query per line):

```json
{ "db_id": "car_1",
  "source_id": "...",
  "arm": "A",                       // "A" = relational / tableToTDS idiom
                                    // "C" = class-navigation idiom
  "constructs": ["join", "group_by", "agg"],
  "pure_text": "|spider::car_1::model::default::Countries.all()->..." }
```

`pure_text` holds the single execution-verified gold Pure lambda string — the exact field the §8.1 replay reads. `arm` records which of the two emitted idioms produced it (see §5.2 / §5.7): **A = relational** (`tableToTDS`-style), **C = class-navigation** (the `.all()->filter(...)` class-anchored pipelines §5 is written around). Arm split: **A = 4,639, C = 395.**

**Construct coverage** (so the reader knows what the grammar is exercised against — these are the SQL-level constructs behind the gold queries, complementing the emitted-Pure inventory of §5.7):

| Construct  | Count | Construct       | Count |
| ---------- | ----: | --------------- | ----: |
| agg        | 2364  | limit           | 692   |
| join       | 2136  | having          | 297   |
| group_by   | 1155  | scalar_subquery | 225   |
| order_by   | 1054  | not_in_subquery | 164   |
| multi_join | 822   | intersect       | 156   |
| distinct   | 712   | except          | 124   |

### 13.2 `corpus/schemas/*.md` — the L2 (schema-consistency) test inputs

**8 database schema context files** — the 5 pilot DBs plus 3 out-of-sample (OOS) DBs:

- Pilot: `concert_singer`, `pets_1`, `battle_death`, `car_1`, `employee_hire_evaluation`
- OOS: `dog_kennels`, `student_transcripts_tracking`, `world_1`

These are the **L2 test inputs** (§6, §8.1 L2-mode, §8.3): the
`Schema` data contract is populated from these files, then matching gold queries
are replayed under the implemented L2 subset. The three OOS files form an in-repo
generalization partition. Because contributors and generators can read them,
they are not a reviewer-controlled held-out suite and no anti-gaming claim rests
on them.

**File format** (from the `concert_singer` example). Each file is Markdown with two load-bearing blocks:

1. An **`## Execution coordinates`** block — `project_id`, `workspace`, `database_path`, the autogen mapping/runtime paths, and the fully-qualified `classes:` and `associations:` lists. Only the class/property/association **structure** feeds L2; the coordinate paths matter to the completeness oracle (§14) when it needs a live model.

2. A **`## Pure model`** block — the autogen Pure grammar text: each `Class …::default::<Name> { prop: <Type>[<mult>]; … }` and each `Association …::fk_N { <endProp>: <TargetClass>[<mult>]; … }`. This is the direct source for the `Schema` contract: classes → `{prop → (type, multiplicity)}`, associations → the two directed navigations of §6.2.3. Example (abbreviated):

```pure
Class spider::concert_singer::model::default::Singer
{
  singerId: Integer[1];
  name: String[0..1];
  country: String[0..1];
  age: Integer[0..1];
  isMale: Boolean[0..1];
}
Association spider::concert_singer::model::fk_1
{
  fk1DefaultSingerInConcert: spider::concert_singer::model::default::SingerInConcert[1..*];
  fk1DefaultSinger:          spider::concert_singer::model::default::Singer[1];
}
```

(Most files also carry a `## Glossary` block mapping question vocabulary → model identifiers; L2 does **not** consume it — it is question-side, not schema-structure.)

**Stale-workspace caveat.** The `workspace:` id in the `## Execution coordinates` block (e.g. `concert-singer-1783544672`) is **ephemeral/throwaway** — fs-SDLC workspaces are disposable (§14.3), and the id will not exist on a fresh stack. Only the class / property / association **STRUCTURE** matters for L2. Never key anything off the workspace id; if the completeness oracle needs a live model, regenerate the workspace (§14).

### 13.3 Where it lives, and regenerating/extending

|              | pure-lingua source repo                                   | PureCARD package subtree                                   |
| ------------ | --------------------------------------------------------- | ---------------------------------------------------------- |
| Gold queries | `data/phase2/armA_*.jsonl` + `armC_*.jsonl` (231 MB, raw) | `corpus/gold_queries.jsonl` (4.8 MB, distilled, committed) |
| Schemas      | `data/pilot/armC_ctx_<db>.md` (+ OOS ctx briefs)          | `corpus/schemas/<db>.md` (committed)                       |
| Legend stack | `infra/legend-stack/`                                     | `corpus/legend-stack/` (§14)                               |

The shipped `corpus/` is sufficient for the implemented offline M0–M3 lanes
without upstream access. Regenerating or extending its provenance requires the
upstream pure-lingua datagen inputs. The committed corpus is a frozen empirical
oracle, not proof that every legal emitted-Pure construct or schema is covered.

---

## 14. Legend engine setup (for the completeness oracle) + CI

The byte-level **soundness** half of §8 is offline (§13.1). The live
**completeness** target needs a Legend engine. The pinned stack ships in the
PureCARD package subtree under `corpus/legend-stack/`. The current lane proves
reachability and classified responses; §8.2 records the lowering and generation
work still needed before it can claim compile-rate completeness.

### 14.1 The stack

`docker compose` with two pinned, anonymous-auth (no GitLab, no Mongo) services, both `platform: linux/amd64`:

| Service         | Image                                            | Port | Health endpoint           |
| --------------- | ------------------------------------------------ | ---: | ------------------------- |
| `legend-engine` | `finos/legend-engine-server-http-server:4.113.0` | 6300 | `GET /api/server/v1/info` |
| `legend-sdlc`   | `finos/legend-sdlc-server-fs:0.195.0`            | 6100 | `GET /api/info`           |

The engine runs `org.finos.legend.engine.server.Server server /config/engine-config.yml`; the SDLC runs `org.finos.legend.sdlc.server.startup.LegendSDLCServerFS server /config/sdlc-config.yml` (filesystem backend, entities under `/data/sdlc`). Both configs use `AnonymousClient` (`deployment.mode: TEST_IGNORE_FUNCTION_MATCH`; `pac4j.bypassPaths: ["/api/server/v1/info"]`). Total image footprint ≈ **1.7 GB**.

Managed completeness run (from the umbrella workspace root):

```bash
just test-legend
```

For manual inspection, bring the stack up from the workspace root and tear it
down afterward:

```bash
docker compose -f crates/pure-analyzer-purecard/corpus/legend-stack/docker-compose.yml up -d

# health-wait (compose sets engine start_period 60s, sdlc 30s):
curl -sf http://localhost:6300/api/server/v1/info   # engine ready
curl -sf http://localhost:6100/api/info             # sdlc ready

docker compose -f crates/pure-analyzer-purecard/corpus/legend-stack/docker-compose.yml down
```

The compose file already declares matching healthchecks (engine: `curl -sf http://localhost:6300/api/server/v1/info`, 60s start / 10s interval / 10 retries; sdlc: `curl -sf http://localhost:6100/api/info`, 30s start). A CI job should poll those two endpoints until 200 before running completeness tests.

### 14.2 The endpoints the completeness oracle uses

Compiling a candidate Pure lambda is a **two-call** sequence on the engine (both from `gate0-findings.md`; the `lambdaReturnType` compile call is the same oracle §8.2 already names):

1. **`POST /pure/v1/grammar/grammarToJson/lambda`** — body is the Pure lambda **text**; returns the lambda as **protocol JSON** (the `grammarToJson` family; per Gate-0, elements carry `package`+`name`, not a `path`).
2. **`POST /pure/v1/compilation/lambdaReturnType`** — body `{ "lambda": <protocol-json-from-step-1>, "model": <PMCD> }`; on success returns the lambda's **return type** (e.g. `TabularDataSet` for a projected pipeline — the Gate-0 end-to-end probe confirmed this), and on failure returns a **compile error**. A returned type == compiles == completeness satisfied for that generation; an error == a grammar/overlay gap to tighten (oracle-driven, never speculative — §8.2).

The `model` is the **PMCD** (PureModelContextData) for the DB — the same model structure the schema files (§13.2) describe, either regenerated into the fs-SDLC workspace or supplied inline. For **L2** verification (§8.3) the model is the specific DB's PMCD, and a phantom-identifier / type-mismatch generation surfaces as a `lambdaReturnType` compile error.

### 14.3 Key quirks that will bite (compilation-relevant subset)

From `gate0-findings.md` + the stack. Keep to what affects _compiling lambdas_ (not the full datagen pipeline):

- **`table` is a reserved SQL-grammar word.** In any relational store text it must be quoted: `"table" => '...'`. Relevant if you (re)generate a store/model rather than using a shipped PMCD.
- **fs-SDLC entity access.** `/entityPaths` 500s on empty workspaces — use `/entities` instead; entities are pushed via `POST .../workspaces/{ws}/entities` with `{message, entities:[{path, classifierPath, content}], replace:true}` (compose `package::name` for the `path`). fs-SDLC workspace **DELETE is broken** (jgit ref lingers) — always use **fresh throwaway workspace names** (this is why the schema files' `workspace:` ids are ephemeral, §13.2). Also verify the PMCD roundtrip after push (`GET .../pureModelContextData` count == pushed count): fs-SDLC **silently drops** elements its bundled protocol can't deserialize.
- **DuckDB is a dead end on stock images — H2 is the store.** The stock engine image lacks the DuckDB execution connector and the SDLC drops DuckDB connections in PMCD conversion. Both are closed facts; do not retry. H2 (`LocalH2`) is the proven store. This only matters if you regenerate models with a relational connection; for pure lambda _compilation_ against a supplied PMCD it is moot.
- **Images are amd64.** On Apple Silicon they run under Rosetta/QEMU emulation (works, slower). The intended **Ubuntu host is native x86**, so no emulation there — the stack runs natively on the target machine.

### 14.4 CI lanes

The repository separates the lanes by cost and evidence:

| Lane                      | Inputs                                            | Coverage boundary                                                     |
| ------------------------- | ------------------------------------------------- | --------------------------------------------------------------------- |
| Hermetic corpus replay    | Committed gold, modern seeds, and schema fixtures | Always-on byte/L2 soundness and regression evidence; no engine        |
| Real-Qwen token-ID replay | Pinned tokenizer artifact, fetched cache-first    | Scheduled/on-demand actual tokenization; no model inference or engine |
| Live Legend               | Two pinned amd64 images plus health-wait          | Local/on-demand reachability and response classification              |

The offline soundness suite runs in every CI execution. The real-tokenizer oracle
is scheduled/on-demand and the live stack is isolated behind `just test-legend`.
The live lane exercises stack reachability and response classification; it does
not claim a compiler-validity guarantee for every decoder output.

---
