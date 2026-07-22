# PureCARD Spec — correctness, corpus & engine

_Part of the [PureCARD spec](README.md); see also the [domain model](../domain-model.md)._

> The layered testing pyramid that operationalizes this strategy is in
> [../methodology/decoder-testing.md](../methodology/decoder-testing.md).

## 8. Correctness — the oracle-driven test strategy (most important section)

A constrained decoder fails _silently and catastrophically_: a **soundness** bug masks valid tokens (the model can never produce correct queries); a **completeness** bug lets the model down a dead end. Both are mechanically testable here because the project owns a ground-truth oracle and a large verified corpus. This is the crux of the whole component — build it test-first.

### 8.1 Soundness — never mask a valid continuation (the killer test)

The repo has **thousands of execution-verified gold Pure queries** (~1,791+ at drafting time). Where they live and how to obtain the test corpus:

- **`data/phase2/armC_*.jsonl`** — verified armC gold queries (JSONL). The Pure query is in the `pure_text` / `final_query` field of each record.
- **`data/pilot/armC2_results_*.jsonl`** — pilot armC2 results (JSONL); same `pure_text` / `final_query` fields.
- **`data/phase2/navC_train.jsonl`** — navigation-heavy training queries.
- (also present: `data/phase2/armA_*.jsonl`.)

Each record's `pure_text` (equivalently `final_query`) field holds a single execution-verified gold Pure query string — extract those to form the soundness test set. The upstream project accesses these via `uv run` Python tooling; for the decoder's Rust tests, read the JSONL directly and pull the query field.

**The soundness test.** For each gold query: tokenize with the target model's tokenizer, replay through the decoder, and **assert at every step that the actual next token is in `allowed_mask()`**. Any gold token masked = soundness bug. This corpus _is_ the L1 test spec, and, with schemas attached, the L2 test spec. For L2, replay against the query's matching `Schema` (built from the DB's ctx brief / MCP reflection) and assert no N/T rule masks a token that actually appears — this catches navigability-direction, inheritance, and multiplicity mistakes mechanically.

### 8.2 Completeness — no dead ends (differential compile test)

Generate under constraint (random accepting walks over the PDA, or model-driven walks), then **compile every result via the real Legend engine**. The engine runs at:

```
http://localhost:6300/api           (self-hosted docker stack, engine 4.113.0)
compile endpoint:  /pure/v1/compilation/lambdaReturnType
```

Target: **100% of constrained generations compile.** Any compile failure = a grammar/overlay gap; tighten the grammar there (oracle-driven, never speculative). This reuses the project's existing engine client and applies the same execution-verification philosophy to the decoder.

### 8.3 Schema-consistency verification (L2)

Constrained generation against schema `S` must never reference a non-`S` identifier or a type-illegal operation. Verify against the compiler's name/type resolution on the `S` model: assert **zero** phantom-identifier / type-mismatch compile errors under L2, using the same `/pure/v1/compilation/lambdaReturnType` oracle.

### 8.4 Differential fuzzing

Random accepting walks over the PDA → all must compile; feed adversarial near-miss prefixes to check masks reject exactly the invalid next-tokens.

### 8.5 Property tests

Using `proptest`: `accept_token` after any token in `allowed_mask()` never panics and never dead-ends before an accepting state is reachable.

### 8.6 Corpus-derivation invariant

Any production/rule a gold query violates is _wrong_ and must be relaxed to admit the corpus; any construct the corpus lacks stays out until a gold query adds it. The verified queries, not intuition, bound the grammar and the rules.

### 8.7 CI gate (non-negotiable)

**100% gold-corpus soundness + 100% constrained-generation compile rate on a held-out schema set.** These are mechanical and non-negotiable gates for the component.

---

## 13. Test corpus — contents, provenance, location

The oracle-driven test strategy of §8 needs two concrete inputs: a large set of execution-verified gold Pure queries (the **soundness** oracle) and per-database schemas (the **L2** test inputs). Both are already assembled and ship **inside the PureCARD workspace** under `corpus/` (committed to the PureCARD repo). A fresh Claude on a fresh machine needs nothing but this checkout to run the entire soundness backbone; the corpus is self-contained and engine-free. This section documents exactly what is in `corpus/`, where it came from, and how to extend it.

### 13.1 `corpus/gold_queries.jsonl` — the soundness oracle

**5,034 unique, execution-verified gold Pure query strings** spanning **161 databases**. This is the SOUNDNESS oracle of §8.1: replay every gold query through the L1 decoder and assert at every step that the actual next token is in `allowed_mask()`; any gold token the mask would forbid is a grammar (soundness) bug. It is simultaneously the **empirical basis the L1 grammar (§5) was derived from** — the verified corpus _is_ the spec (§5, §8.6), and this file is that corpus in shippable form.

**Soundness testing over this file is FULLY OFFLINE — no Legend engine required.** It needs only the gold query text + the grammar + the model tokenizer's byte representation of tokens (§9). This is the whole point: the core correctness backbone runs in any CI with zero infrastructure.

Provenance: distilled from the upstream **pure-lingua** project's Phase-2 output — `data/phase2/armA_*.jsonl` + `data/phase2/armC_*.jsonl`, keeping only `accepted=true` (execution-verified) records and de-duplicating query strings. The full `data/phase2/` directory is **231 MB** (not GitHub-committable); this distillation is **4.8 MB** and is committed to the PureCARD repo.

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

These are the **L2 test inputs** (§6, §8.1 L2-mode, §8.3): the `Schema` data-contract (§6.2) is populated **from these files** (host-side, never by the decoder), then a gold query for that DB is replayed under L2 asserting no N/T rule masks a token that actually appears. This is what mechanically catches navigability-direction (§6.2.3), inheritance, and multiplicity mistakes. The pilot set backs M3 schema-soundness; the 3 OOS DBs are the **held-out schema set** the §8.7 CI gate and M3 done-criterion refer to.

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

|              | pure-lingua source repo                                   | PureCARD workspace                                         |
| ------------ | --------------------------------------------------------- | ---------------------------------------------------------- |
| Gold queries | `data/phase2/armA_*.jsonl` + `armC_*.jsonl` (231 MB, raw) | `corpus/gold_queries.jsonl` (4.8 MB, distilled, committed) |
| Schemas      | `data/pilot/armC_ctx_<db>.md` (+ OOS ctx briefs)          | `corpus/schemas/<db>.md` (committed)                       |
| Legend stack | `infra/legend-stack/`                                     | `corpus/legend-stack/` (§14)                               |

**The shipped `corpus/` is sufficient for M0–M3** (M1 L1 soundness, M2 perf, M3 L2 overlay) with no upstream access. To **regenerate or extend** the corpus — more schemas, more query shapes, new constructs the grammar does not yet exercise — the reader needs the full **pure-lingua repo + its Legend stack** (the datagen pipeline that produced `data/phase2/` and the ctx briefs). That is out of scope for building PureCARD; note it only so a future maintainer knows the upstream provenance path exists. For the decoder itself, the committed corpus is the complete test spec.

---

## 14. Legend engine setup (for the completeness oracle) + CI

The **soundness** half of §8 is offline (§13.1). The **completeness** half (§8.2 — _do constrained generations actually compile?_ — and §8.3 — _does L2 output resolve on the real model?_) needs a **live Legend engine**. This section documents that engine, taken verbatim from the real infra files (`infra/legend-stack/docker-compose.yml`, `engine-config.yml`, `sdlc-config.yml`) and the Gate-0 probe findings (`docs/probes/gate0-findings.md`) — not invented. The stack ships to the PureCARD workspace under `corpus/legend-stack/`.

### 14.1 The stack

`docker compose` with two pinned, anonymous-auth (no GitLab, no Mongo) services, both `platform: linux/amd64`:

| Service         | Image                                            | Port | Health endpoint           |
| --------------- | ------------------------------------------------ | ---: | ------------------------- |
| `legend-engine` | `finos/legend-engine-server-http-server:4.113.0` | 6300 | `GET /api/server/v1/info` |
| `legend-sdlc`   | `finos/legend-sdlc-server-fs:0.195.0`            | 6100 | `GET /api/info`           |

The engine runs `org.finos.legend.engine.server.Server server /config/engine-config.yml`; the SDLC runs `org.finos.legend.sdlc.server.startup.LegendSDLCServerFS server /config/sdlc-config.yml` (filesystem backend, entities under `/data/sdlc`). Both configs use `AnonymousClient` (`deployment.mode: TEST_IGNORE_FUNCTION_MATCH`; `pac4j.bypassPaths: ["/api/server/v1/info"]`). Total image footprint ≈ **1.7 GB**.

Bring-up (from `corpus/legend-stack/`):

```bash
docker compose -f corpus/legend-stack/docker-compose.yml up -d

# health-wait (compose sets engine start_period 60s, sdlc 30s):
curl -sf http://localhost:6300/api/server/v1/info   # engine ready
curl -sf http://localhost:6100/api/info             # sdlc ready
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

### 14.4 CI guidance (the reader must decide — here is the reasoning)

Two test classes with very different infrastructure cost:

| Test class                                                                                                      | What it needs                                                                                    | CI stance                                                                                                                                                                                                                                           |
| --------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------ | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **Soundness** — replay 5,034 gold queries through L1 (§8.1); L2 replay against `corpus/schemas/` (§8.1 L2-mode) | **Nothing** — just the committed `corpus/` + the model tokenizer bytes. Fully offline, hermetic. | **Run in EVERY CI run.** Zero infra. This is the core correctness backbone.                                                                                                                                                                         |
| **Completeness** — constrained generations must compile (§8.2); L2 resolves on the real model (§8.3)            | **A live Legend engine** — two amd64 images, ≈ 1.7 GB, docker-compose up + health-wait.          | **Separate engine-backed job.** Either (a) spin the compose up in a dedicated CI job on an **x86 runner** (feasible; document the health-wait of §14.1), or (b) gate it as **opt-in / nightly / local-only** to keep the main CI fast and hermetic. |

**Recommendation:** run **offline soundness in every CI run**; run **completeness as a separate engine-backed job — nightly or on-demand** — on an x86 runner. State plainly to the reader: **the core correctness backbone (soundness replay of all 5,034 gold queries) needs NO engine**, so PureCARD is CI-testable out of the box with only the committed corpus; the Legend engine is required **only** for the completeness half, and that half can be deferred to a nightly/on-demand job without weakening the always-on soundness gate. (The §8.7 CI gate remains the target — 100% gold soundness always, 100% constrained-generation compile rate on the completeness job.)

---
