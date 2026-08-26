# pure-analyzer — Design Document

**A mechanical, standalone, Rust static-analysis toolchain for Legend Pure (the modern `Relation<>` dialect)**

Status: target design specification (v1). This document defines intended
analyzer behavior; it is not a current implementation inventory.

> **Implementation status (2026-08-26).** `pure-analyzer` is an early
> scaffold. The lexer and diagnostic model contain substantive code; syntax,
> parser, model, resolve, analysis, and `libpure` are mostly version stubs, and
> CLI subcommands return `not implemented yet`. The umbrella workspace also
> contains `pure-analyzer-purecard` as an independent, unpublished sibling
> product. Its M0–M5 code artifacts exist, but its documented end-to-end proof
> obligations remain, so PureCARD does not claim feature completeness. It is not part of
> this analyzer design or its crate DAG. See the
> [domain model](../domain-model.md) and [ADR-0004](../decisions/0004-purecard-independent-workspace-product.md)
> for current repository topology.

---

## Table of Contents

1. Overview & Scope
2. Background an Implementer Needs
3. Analyzer Product / Crate Layout
4. `libpure` — Parser, Model Loader, Resolver (+ the full milestoning-arity algorithm)
5. Subcommand Contracts — `validate`, `lint`, `eq`/`diff`, `fmt`
6. Diagnostic / Output Format, Exit Codes, Config
7. Model Input (PMCD JSON and/or Pure-model stereotypes — staying engine-free)
8. Test Strategy
9. Staged Milestones
10. Honest Limits & Explicitly Out of Scope

---

## 1. Overview & Scope

### 1.1 What pure-analyzer is

`pure-analyzer` is a fast, deterministic, **no-LLM, no-runtime-engine** command-line static-analysis toolchain for **Legend Pure**, the query language of the FINOS Legend platform. It targets the **modern `Relation<>` dialect** (`meta::pure::functions::relation::*` over `Relation<(col:Type[mult], …)>`), not the legacy `TabularDataSet` (TDS) API.

Within the umbrella repository, it is designed as **one shared analysis engine
(`libpure`) with two first-class front-ends over it**: a CLI binary
(`pure-analyzer`, five subcommands) and a Language Server
(`pure-analyzer-lsp`). Once implemented, both front-ends consume the same
parser, resolver, and `Diagnostic` model, so each check below is available
identically at the command line and live in an editor.

- **`validate`** — grammar + shallow well-formedness. Converges on the Legend engine's acceptance behavior. Needs no model.
- **`lint`** — the differentiated value: **milestoning `%latest`-arity checking** (the proven core), unknown-property, and statically-determinate multiplicity misuse. Needs a model.
- **`eq` / `diff`** — SOUND, INCOMPLETE, **3-valued** structural equivalence (`EQUIVALENT` / `NOT_EQUIVALENT`+witness / `INDECISIVE`+reason-code) over the decidable relational core.
- **`fmt`** — canonical formatting (a lossless-CST layout mode, plus an optional `--canonical` mode built from the eq normalizer).
- **LSP (`pure-analyzer-lsp`)** — the same `validate`/`lint` findings as live editor squiggles-on-change, each `Fix` as a code-action, `explain` as hover, and go-to-definition on navigation via the resolver: the analysis engine as an editor service, not a bolt-on.

### 1.2 Why it exists (the gap)

No fast standalone static-analysis toolchain for Pure exists today; current tooling is Java/IDE-based and requires the full Legend engine. In particular, deep **milestoned navigation** (getting the count of `%latest` temporal arguments right on a navigation chain) is a real Legend/Reladomo developer footgun that even frontier LLMs get wrong end-to-end. `pure-analyzer lint` mechanically decides it.

### 1.3 Design constraints (non-negotiable)

- **Mechanical / deterministic.** No LLM, no network, no clock, no randomness in output. Identical `(inputs, model, config, flags)` with `--jobs 1` ⇒ **byte-identical** output and exit code.
- **Standalone at runtime.** Inputs are Pure query files plus a model. The Legend engine is used **only** (a) offline, once, to optionally produce a PMCD JSON model, and (b) at **dev/CI time** as the differential-testing oracle for `validate` fidelity and `eq` soundness. Never at runtime.
- **Engine-free model path is first-class.** The single fact milestoning arity needs — the *target class's temporal stereotype* — is present in Pure model source (`<<temporal.X>>`). A parsed **Pure model file is the first-class model input**; PMCD JSON is an **optional coverage booster** (associations, richer qualified properties). pure-analyzer must deliver full-strength arity linting **without** running the engine. (See §7.)
- **Rust; FINOS-open-source-friendly; domain-agnostic.** pure-analyzer is about the *language*, never any customer data or private domain. Zero private-domain content.
- **Correct-first.** `validate` must not over-reject (breaking legal code is as fatal as over-admitting). `eq` **soundness is sacred**: it must never wrongly commit `EQUIVALENT` or `NOT_EQUIVALENT`.
- **Ships incrementally, LSP-first in the core.** validate+lint ship first (small, complete, high value), then eq+fmt and the LSP surface. The **LSP is a first-class front-end, not an afterthought** — the `Diagnostic` model, byte-offset spans, structured `Fix`es, and `explain` text are designed to drive an editor from v0.1 onward (see §1.5); the LSP *server* lands as soon as there are diagnostics to serve.

### 1.4 What is IN v1 vs future

| Capability                                                                                             | v0.1                           | v0.2 | v0.3 | v2+ |
| ------------------------------------------------------------------------------------------------------ | ------------------------------ | ---- | ---- | --- |
| lexer + resilient parser + lossless CST + spans                                                        | ✅                             |      |      |     |
| `validate` (grammar fidelity + over-admission guards)                                                  | ✅                             |      |      |     |
| model loader (Pure-file **and** PMCD) + resolver                                                       | ✅                             |      |      |     |
| `lint` (milestoning arity core + unknown-property + cardinality)                                       | ✅                             |      |      |     |
| `fmt` **default layout mode** (lossless-CST re-emit)                                                   | ✅                             |      |      |     |
| `eq`/`diff` structural NF + schema/structural refutation (**M4a**)                                     |                                | ✅   |      |     |
| `eq` bounded bag-interpreter witness search (**M4b**)                                                  |                                | ✅¹  |      |     |
| `fmt --canonical` (serialize eq NF)                                                                    |                                | ✅   |      |     |
| reason-code taxonomy + `explain` + doc pages                                                           |                                | ✅   |      |     |
| **`pure-analyzer-lsp` — diagnostics-on-change + code-actions (`Fix`) + hover (`explain`) + go-to-def** |                                | ✅   |      |     |
| LSP `salsa` incremental recompute (only if profiling demands)                                          |                                |      | ✅   |     |
| SMT symbolic eq arm (feature-gated)                                                                    |                                |      |      | ✅  |
| Research-grade milestoning-equivalence THEORY                                                          | ❌ never in this project scope |      |      |     |

¹ M4b ships only after its null/constraint/partiality semantics are pinned by the engine differential corpus (§8). If not ready, v0.2 ships M4a only; the general witness refuter waits.

**Explicitly OUT of v1 forever-as-far-as-this-doc-is-concerned:** the research-grade milestoning-equivalence decision procedure (SMT/semiring bitemporal as-of laws for window/pareto/multi-step-fiscal/division equivalence). `eq` is honestly `INDECISIVE` (with a `FUNDAMENTAL` reason code) on all of these. See §10.

---

### 1.5 Two surfaces, one core (the LSP is first-class)

`pure-analyzer` is **an analysis engine with two co-equal front-ends**, not a CLI that might grow an LSP later. `libpure` (parser → resolved model → passes → `Diagnostic`) is the whole product; the CLI and the LSP are thin adapters over it:

- The **CLI** (`pure-analyzer`) renders `Diagnostic`s to a terminal / JSON / SARIF and returns exit codes.
- The **LSP** (`pure-analyzer-lsp`, `tower-lsp`) renders the *same* `Diagnostic`s as live editor squiggles, turns each `Fix` into an LSP `CodeAction`/`WorkspaceEdit`, serves `explain` text as hover, and answers go-to-definition on navigation via the resolver — over VS Code, JetBrains (LSP4IJ), Neovim, and any LSP client.

Three things in the core exist **specifically** so the LSP is a free adapter rather than a rewrite: (1) every `Diagnostic` carries **byte-offset spans** convertible to LSP UTF-16 positions at the boundary only (`codespan-lsp`); (2) `Fix` is a **structured edit** (span + replacement), not a rendered string, so it maps directly to a `WorkspaceEdit`; (3) the parser is **resilient** (error-recovering, lossless CST) so it yields a usable tree + diagnostics on every keystroke, including on incomplete input. `salsa` incremental recomputation is added **only** if profiling shows re-parse cost matters (§9) — resilient full re-parse of a single query file is already sub-millisecond, so correctness never depends on it.

---

## 2. Background an Implementer Needs

This section carries everything about Pure required to implement pure-analyzer. Primary ground truth is `finos/legend-engine` (ANTLR4 `.g4` grammars + `Milestoning.java`).

### 2.1 The Pure Relation dialect essentials

A Pure query is a **lambda / function body** built by **left-associative pipe application** of `->function(...)` arrow calls and `.property` navigations over a root.

Two root forms matter:

- **Class root:** `model::Person.all()` yields all instances of a class; navigation is `.prop`.
- **Relation root (store table pointer):** `#>{db::path.TableName}#` yields a `Relation<>` from a database table; access is via column specs (`~col`), not `.prop`.

**Relation type:** `meta::pure::metamodel::relation::Relation<(id:Integer[0..1], name:String[0..1])>` — a **parenthesized, ORDERED, named tuple** of `col:Type[mult]`. **Column order is observable and semantically significant.** (This matters critically for `eq`; see §5.3.)

**Canonical worked query** (from engine test `relationMappingSetup.pure`):

```text
#>{db::testDB.personTable}#
  ->join(#>{db::testDB.groupMembershipTable}#, JoinKind.INNER, {x,y| $x.ID == $y.PERSONID})
  ->extend(over(~GROUPID, ~SALARY->ascending()), ~[RANK:{p,w,r| $p->rank($w, $r)}]);
```

**Core relation operators** (package `meta::pure::functions::relation::*`). The **closed v1 whitelist** (the authoritative set validate recognizes and eq lowers; anything else is unknown):

```text
filter, select, extend, rename, groupBy, join, sort, distinct,
limit, take, drop, slice, size, over, asOfJoin, pivot
```

Shapes:

- `filter(rel, x| <Boolean>)`
- `select(rel, ~[cols])` — column subset (modern project/restrict)
- `extend(rel, ~newCol: {…})` or `extend(rel, over(…), ~winCol:{p,w,r| …})` (window)
- `rename(rel, ~old, ~new)`
- `groupBy(rel, ~[groupCols], ~[aggName: mapLambda : reduceLambda])`
- `join(rel1, rel2, JoinKind.X, {x,y| <cond>})`
- `sort(rel, [~col->ascending(), …])`; `distinct(rel)`; `limit(rel, n)`; `take/drop/slice`; `size(rel)`
- `over(…)`, `asOfJoin`, `pivot` — present but **OPAQUE** in the eq core (§5.3).

**`JoinKind`** (`meta::pure::functions::relation::JoinKind`) is the **closed enum `{INNER, LEFT}`** today. Any other value (`FULL_OUTER`, etc.) is unknown.

**Sort direction:** `ascending()` / `descending()`.

**Window functions** (inside `extend(over(...), ~c:{p,w,r| ...})`): `rowNumber, rank, denseRank, percentRank, ntile, cumulativeDistribution, lag, lead, first, last, nth`.

**Aggregators** (`meta::pure::functions::math::*`): `sum, min, max, count, percentile, stdDevSample, stdDevPopulation, varianceSample, variancePopulation`.

### 2.2 Grammar architecture to mirror

The engine grammar is ANTLR4, split lexer/parser, layered by import: **Core → M3 → Domain**.

- **Core / token layer** (`CoreLexerGrammar.g4`): literals `%latest`, dates `%YYYY-MM-DD[Thh:mm:ss]`, symbols `~ $ -> | @ : ^ # . , :: ( ) [ ] { }`, `%` (PERCENT), `==`, `!=`, arithmetic.
- **M3 / expression layer** (`M3ParserGrammar.g4`, `M3LexerGrammar.g4`): `Class.all()->…` chains, `->fn(args)` arrow chaining, `.prop` navigation with optional milestoning args, `~`-colSpecs, lambdas, `^`-new-instance, `$var`, `@`-cast, keywords `all let allVersions allVersionsInRange toBytes`.
- **Domain / model-definition layer** (`DomainParserGrammar.g4`): `Class` / `Association` / `Profile` / `enum` / `measure` / `function` definitions. pure-analyzer parses this **only** for the Pure-model-file input path (§7).

**Verbatim M3 productions pure-analyzer must implement** (the contract):

```text
atomicExpression      : dsl | instanceLiteralToken | expressionInstance | unitInstance
                      | variable | columnBuilders | (AT type) | anyLambda | instanceReference
expression            : nonArrowOrEqualExpression ((propertyOrFunctionExpression)* (equalNotEqual)?)
propertyOrFunctionExpression : propertyExpression | functionExpression | propertyBracketExpression
functionExpression    : ARROW qualifiedName functionExpressionParameters
                        (ARROW qualifiedName functionExpressionParameters)*
functionExpressionParameters : '(' (combinedExpression (',' combinedExpression)*)? ')'
propertyExpression    : '.' identifier (functionExpressionLatestMilestoningDateParameter
                                        | functionExpressionParameters)?
functionExpressionLatestMilestoningDateParameter : '(' LATEST_DATE (',' LATEST_DATE)? ')'
buildMilestoningVariableExpression : LATEST_DATE | DATE | variable
propertyBracketExpression : '[' (STRING | INTEGER) ']'
instanceReference     : (PATH_SEPARATOR | qualifiedName | unitName) allOrFunction?
allOrFunction         : allFunction | allVersionsFunction | allVersionsInRangeFunction
                      | allFunctionWithMilestoning | functionExpressionParameters
allFunction           : '.' 'all' '(' ')'
allVersionsInRangeFunction : '.' 'allVersionsInRange' '(' buildMilestoningVariableExpression ','
                                                          buildMilestoningVariableExpression ')'
lambdaFunction        : '{' (lambdaParam (',' lambdaParam)*)? lambdaPipe '}'
lambdaPipe            : '|' codeBlock
lambdaParam           : identifier lambdaParamType?
anyLambda             : lambdaPipe | lambdaFunction | lambdaParam lambdaPipe
expressionInstance    : NEW_SYMBOL (variable | qualifiedName) ...     // ^Class(prop=...)
variable              : '$' identifier
columnBuilders        : '~' (oneColSpec | colSpecArray)
oneColSpec            : stereotypes? taggedValues? identifier
                        (':' (type multiplicity? | anyLambda) extraFunction?)?
colSpecArray          : '[' (oneColSpec (',' oneColSpec)*)? ']'
extraFunction         : ':' anyLambda
relationType          : '(' columnInfo (',' columnInfo)* ')'
columnInfo            : columnName ':' type multiplicity?
```

**Critical grammar realities the critiques surfaced:**

1. **A lambda body is a `codeBlock`, not a single expression.** It is a semicolon-separated statement list that can include `let x = …; $x->…`. The parser and AST **must** model multi-statement lambda bodies and `let`. Under-admitting these is a fidelity failure.
2. **Single-param lambdas may omit braces** (`x|$x.foo`); multi-param require braces (`{x,y|…}`).
3. **`columnBuilders` (`~col`) IS an `atomicExpression`** — grammatically legal anywhere an expression is. A stray `~col` is *not* a grammar rejection (see §5.1).
4. **`.prop(%latest)` vs `.prop($businessDate)` vs `.prop(25)` are syntactically identical** at the parser level (all `functionExpressionParameters`), EXCEPT that the dedicated `functionExpressionLatestMilestoningDateParameter` production admits *only* `%latest` tokens. In practice the engine also accepts `$businessDate` / explicit `DATE` as the one-arg milestoning form via ordinary `functionExpressionParameters`. **Whether a given argument list *is* a milestoning-date list vs an ordinary qualified-property call is model-dependent** — it cannot be decided from syntax alone. This drives the fact/policy split (§4.4, §5.2).
5. **`allVersionsInRange` is its own production** taking a **date range** (`buildMilestoningVariableExpression`, i.e. `$var`/`DATE`, **not** `%latest`), not a `%latest` point arity. It must **not** be folded into the `%latest` arity machinery.

### 2.3 Islands (`#…#`)

`#…#` blocks are lexed as **islands** and are **not** folded into the main grammar. Discriminate on the leading marker:

- `#>{ db::path.Table }#` → **StoreTablePointer**. Deep-parse only the pointer `(databasePath, tableName)` (grammar `databasePointer` from `RelationalParserGrammar.g4`, below). **Columns are NOT here** — they live in a separate `Database` store definition (deferred to a later milestone; see §7.4).
- `#{ … }#` / `#TDS … #` → **TdsLiteral**. Captured opaque (spanned raw).
- `#/model::Class/prop#` (`NAVIGATION_PATH_BLOCK`) → **NavPathBlock**. Optionally deep-parsed; opaque otherwise.
- anything else → **OpaqueIsland** (spanned raw).

`databasePointer` (verbatim, for the deep-parse of `#>{…}#`):

```text
databasePointer : '#>{' qualifiedName '.' identifier '}#'   // db::path::Db . tableName
```

**Island lexing note (correcting a hazard):** `logos` is a stateless DFA generator and **cannot cleanly implement a nesting `pushMode/popMode` stack** with `{ … }` depth inside islands. **Do not** attempt a logos mode stack. Instead lex `#`, `#>{`, `#{`, `#/`, `{`, `}`, `}#` as **raw tokens** and **balance islands in the parser** (or a tiny dedicated island sub-scanner with a hand-managed depth counter). An unterminated / unbalanced island is `PUR0101 unterminated-island` (Error).

### 2.4 Milestoning + `%latest` arity (the proven core)

**Milestoning** is a **class stereotype** from the fixed profile:

```text
Profile meta::pure::profiles::temporal {
  stereotypes: [bitemporal, businesstemporal, processingtemporal];
}
```

A milestoned class is declared `Class <<temporal.bitemporal>> pkg::MyClass { … }`.

**The arity law (validated against `Milestoning.java`):** the number of `%latest` (as-of) date arguments a milestoned navigation takes = **the temporal stereotype of the navigation's TARGET (return) class**, NOT the source:

| Target class stereotype | Required `%latest` args | dates                          |
| ----------------------- | ----------------------- | ------------------------------ |
| `bitemporal`            | **2**                   | (processingDate, businessDate) |
| `businesstemporal`      | **1**                   | (businessDate)                 |
| `processingtemporal`    | **1**                   | (processingDate)               |
| none                    | **0**                   | plain nav                      |

Engine confirmation: `UNI_TEMPORAL_STEREOTYPE_NAMES = ["businesstemporal","processingtemporal"]`; arity = `getTemporalDatePropertyNames().size()`; the generated qualified property is keyed off `returnTypeMilestoningStereotype`.

**Source-threading is mandatory.** The *same property name* on *different source classes* points to *different target classes* and thus can have different arity. pure-analyzer must look each property up on the **current threaded source class**, never a global property table.

**The context gate (the false-positive killer).** A **no-arg** milestoned navigation (0 args on a milestoned target) is **legal** iff the *immediate source class of that hop* is itself milestoned compatibly — the as-of date propagates. Engine rule (`Milestoning.java`): a no-arg generated qualified property is emitted iff

```text
sourceMilestoningStereotype != None  AND  (source == target  OR  source == bitemporal)
```

where **`source` = the milestoning of the IMMEDIATE source class of THAT hop**.

**CORRECTION (critique-mandated, this is a real bug in the naive design): the context is NOT a global propagated flag.** It must be recomputed **fresh at each hop** as `milestoning(Csrc_of_this_hop)`. After stepping through a *non-milestoned* intermediate class, the source is non-milestoned, so the next milestoned nav is **out of context and REQUIRES explicit `%latest`.**

Worked witness of the bug the correction fixes:

```text
BiClass.all()->filter(x | $x.plainProp.biProp == ...)
```

`plainProp`'s target is non-milestoned; therefore the context at the `biProp` hop is `None`, so `biProp` (bitemporal target) **requires `(%latest,%latest)`**. A design that propagated `Bitemporal` from the root would wrongly accept 0 args (a false negative). The correct rule: context of a hop = temporal stereotype of *that hop's source class*.

**Generated vs user qualified properties (critical false-positive gate).** The `%latest` arity check applies **ONLY** to *generated milestoned qualified properties* — those carrying a stereotype from `meta::pure::profiles::milestoning` (`generatedmilestoningproperty` / the `WithArg` / `AllVersions` / `AllVersionsInRange` variants). A **user-defined qualifiedProperty** (e.g. `tradesForClient($clientId)`) uses ordinary `functionExpressionParameters` whose args are its *own* business parameters, resolved by its **compiled signature**. It must **never** be fed through the `%latest` counter even if it *returns* a milestoned class. Getting this wrong false-positives correct code (`.tradesForClient($clientId)` returning bitemporal → "requires 2, found 1"). The resolver must consult the **milestoning-generation stereotype ON THE PROPERTY**, not merely the target-class stereotype.

**`AllVersions` / `AllVersionsInRange` / edge-point.** These are separate generated forms: `AllVersions` strips the point-in-time context (accepts 0 args, returns all rows); `AllVersionsInRange(d1,d2)` takes a **date range** (via `buildMilestoningVariableExpression`), matched against the property's **range signature**, not `%latest`. Edge-point (unbounded-multiplicity) generated props take the target's point arity. If v1 cannot reliably classify these, it **degrades to a recoverable, no-flag path** (§4.4), never a false Error.

**Milestoned associations.** Legend/Reladomo supports `Association <<temporal.businesstemporal>>`. Navigating a milestoned-association property can require/propagate an as-of date **independent of the target class's own milestoning**. The association's temporal stereotype must be loaded and folded into required arity for association-contributed hops. If the association stereotype is unknown (Pure-file model omission), degrade to recoverable (§4.4).

**Full milestoning-arity truth table (materialized — the artifact the critiques demanded).** For a hop where `Ssrc = milestoning(Csrc)` (source, fresh), `Stgt = milestoning(Ctgt)` (target), and the property is a **generated milestoned qualified property**:

| `Ssrc` (this hop's source) | `Stgt` (target) | base = arity(Stgt) | context-gated? `Ssrc∈{Stgt,Bitemporal}` & `Ssrc≠None` | **Accepted actual arities** |
| -------------------------- | --------------- | ------------------ | ----------------------------------------------------- | --------------------------- |
| any                        | none            | 0                  | n/a                                                   | **{0}**                     |
| None                       | business        | 1                  | no                                                    | **{1}**                     |
| None                       | processing      | 1                  | no                                                    | **{1}**                     |
| None                       | bitemporal      | 2                  | no                                                    | **{2}**                     |
| business                   | business        | 1                  | yes                                                   | **{0, 1}**                  |
| business                   | processing      | 1                  | no (`bus≠proc`, `bus≠bi`)                             | **{1}**                     |
| business                   | bitemporal      | 2                  | no                                                    | **{2}**                     |
| processing                 | processing      | 1                  | yes                                                   | **{0, 1}**                  |
| processing                 | business        | 1                  | no                                                    | **{1}**                     |
| processing                 | bitemporal      | 2                  | no                                                    | **{2}**                     |
| bitemporal                 | business        | 1                  | yes (`Ssrc==Bitemporal`)                              | **{0, 1}**                  |
| bitemporal                 | processing      | 1                  | yes                                                   | **{0, 1}**                  |
| bitemporal                 | bitemporal      | 2                  | yes                                                   | **{0, 2}**                  |

For non-generated (user) qualified properties: **arity check is disabled**; the call is checked against the compiled signature (arg count) if available, else recoverable-skipped. For `AllVersions`: accepted `{0}`. For `AllVersionsInRange`: checked against range signature (2 range args), not this table.

### 2.5 The PMCD / model format (what the model layer reads)

The compiled-model protocol (M3 JSON, produced by engine `grammarToJson`/model build) encodes, per element:

- **`Class`**: `path` (package + name); `stereotypes: [{profile, value}]` (temporal one is `{profile:"meta::pure::profiles::temporal", value:"bitemporal|businesstemporal|processingtemporal"}`); `superTypes` (generalizations); `properties` (name → target type + multiplicity); `qualifiedProperties` (incl. generated milestoned ends carrying `meta::pure::profiles::milestoning` stereotypes).
- **`Association`**: two property ends (`classA.propAtoB`, `classB.propBtoA`); may carry a temporal stereotype.
- **`Profile`**: stereotype/tag declarations.
- **`Database` / relational store**: table → column types (deferred; §7.4).

**Minimal PMCD JSON shape (worked example):**

```json
{ "elements": [
  { "_type": "class", "package": "model", "name": "Trade",
    "superTypes": ["model::Instrument"],
    "stereotypes": [{"profile":"meta::pure::profiles::temporal","value":"bitemporal"}],
    "properties": [
      {"name":"quantity","genericType":{"rawType":"Integer"},"multiplicity":{"lowerBound":1,"upperBound":1}},
      {"name":"product","genericType":{"rawType":"model::Product"},"multiplicity":{"lowerBound":0,"upperBound":1}}
    ],
    "qualifiedProperties": [
      {"name":"product","stereotypes":[{"profile":"meta::pure::profiles::milestoning","value":"generatedmilestoningproperty"}],
       "genericType":{"rawType":"model::Product"},"returnMultiplicity":{"lowerBound":0,"upperBound":1}}
    ]
  },
  { "_type": "association", "package":"model", "name":"Trade_Product",
    "stereotypes": [{"profile":"meta::pure::profiles::temporal","value":"businesstemporal"}],
    "properties": [ {"name":"product","genericType":{"rawType":"model::Product"},...},
                    {"name":"trades","genericType":{"rawType":"model::Trade"},...} ] }
] }
```

**How to produce a PMCD JSON (for completeness — an OFFLINE, once step):** feed the Pure model text to the Legend engine's `grammarToJson` / model-compile endpoint (the `legend-engine-language-pure-grammar` + compiler jar), which emits the M3 protocol JSON. This is optional; the Pure-model-file path (§7) needs no engine.

---

## 3. Analyzer Product / Crate Layout

The analyzer target is a set of small crates, modeled on ruff / rust-analyzer /
Biome: **parse once → one lossless CST + one resolved model → many passes →
one `Diagnostic` model → render at the boundary.** It shares the root Cargo
workspace with PureCARD, but no product dependencies or ownership.

```text
pure-analyzer/                          # workspace root (Cargo.toml [workspace], resolver = "3")
├── Cargo.toml                   # [workspace.dependencies] — single pin source
├── crates/
│   ├── pure-analyzer-purecard/        # independent constrained decoder; NOT in the analyzer DAG
│   ├── pure-analyzer-lexer/            # logos #[repr(u16)] token enum; % dates; # raw island tokens
│   ├── pure-analyzer-syntax/           # SyntaxKind + rowan Language impl (num-derive FromPrimitive,
│   │                            #   NOT unsafe transmute); ungrammar-driven typed AstNode views
│   ├── pure-analyzer-parser/           # hand-written resilient RD + Pratt; event stream -> rowan
│   │                            #   green tree + Vec<Diagnostic>. Parses M3 (query) + Domain (model)
│   ├── pure-analyzer-model/            # PMCD JSON loader + Pure-model-file (Domain) loader -> ModelGraph
│   ├── pure-analyzer-resolve/          # source-threaded nav resolution + milestoning arity + local
│   │                            #   lambda-param / let type environment
│   ├── pure-analyzer-diagnostics/      # Diagnostic{code,severity,spans,message,fix?,verdict?,reason?,url?}
│   │                            #   + ReasonCode enum (Fundamental|Recoverable). NO renderers.
│   ├── pure-analyzer-ir/               # (v0.2) RA-IR + normalizer (canonical NF); shared by eq & fmt --canonical
│   ├── pure-analyzer-eq/               # (v0.2) eq decision procedure: refutation arm + bounded interpreter
│   ├── pure-analyzer-analysis/         # Pass/visitor trait + check_* hooks; validate + lint passes;
│   │                            #   thin wrappers dispatching eq/fmt into pure-analyzer-ir/pure-analyzer-eq
│   ├── libpure/                 # thin facade: pub use of syntax/parser/model/resolve/(ir/eq)
│   ├── pure-analyzer-cli/              # the `pure-analyzer` binary: clap, config, renderers, exit codes
│   ├── pure-analyzer-lsp/              # (v0.2, first-class surface) tower-lsp; Diagnostic -> lsp_types (byte->UTF-16, Fix->CodeAction, explain->hover)
│   └── pure-analyzer-eq-smt/           # (v2+, feature="smt") easy-smt -> later z3; behind a flag
├── xtask/                       # shared repository orchestration; not a product layer
├── tests/
│   ├── corpus/{accept,reject}/  # engine-parity snippets (reject tagged with expected PUR code)
│   ├── milestoning/             # arity fixtures (+PMCD and +Pure-file parity)
│   ├── golden/                  # per-subcommand input(+model) -> expected serialized Diagnostic
│   └── fuzz/                    # cargo-fuzz targets (parser no-panic/no-hang; eq soundness)
└── docs/reason-codes/           # one markdown page per reason/rule code (explain / url target)
```

**Analyzer processing pipeline:**

```text
lexer → syntax → parser → model → resolve → analysis → libpure → cli
```

This arrow describes processing order. Cargo dependency arrows point toward
prerequisites. In particular, `pure-analyzer-resolve → pure-analyzer-model`
is permitted and the reverse is forbidden. `pure-analyzer-diagnostics` is a
shared leaf depended on by parser-and-above analyzer crates. Planned
`pure-analyzer-ir`, `pure-analyzer-eq`, and LSP/SMT crates extend this analyzer
graph only when their milestone updates the allow-set; they do not change the
model-before-resolve ordering.

PureCARD has no place in this graph. Analyzer and PureCARD crates have zero
Cargo edges in either direction across normal, development, build, optional,
and renamed dependencies. `cargo xtask verify-layering` enforces both the
ADR-0003 analyzer allow-set and the ADR-0004 product boundary against
`cargo metadata`.

Rules:

- Only the **front-end crates** (`pure-analyzer-cli`, `pure-analyzer-lsp`) may depend on renderers (`ariadne`, `codespan-reporting`, `codespan-lsp`) or protocol crates (`clap`, `tower-lsp`, `lsp-types`). Everything below them emits only structured `Diagnostic`s, so the CLI and LSP render identical findings.
- **`pure-analyzer-eq` / `pure-analyzer-ir` are their own crates** so the sound core (validate+lint) builds/ships without pulling the heavy, soundness-critical interpreter. `pure-analyzer-analysis` must not become a grab-bag.
- **`smt` is feature-gated** and behind `pure-analyzer-eq-smt`; a CI `--no-default-features` build asserts the sound core builds with **zero solver dependency**. Soundness never depends on a solver being installed.
- Parser or corpus reuse with PureCARD is not implied by co-location. It needs a
  future spec and ADR before either product may take such a dependency or share
  ownership of an asset.

**Pinned crates:** `logos`; `rowan` (evaluate `cstree` if traversal-bound); `ungrammar` + `num-derive`; `clap` (derive); `serde`/`serde_json`; `ariadne` + `codespan-reporting` (+ `codespan-lsp`); `rayon`; `ignore`/`walkdir`; `insta`; `anyhow` (CLI only — libpure returns typed errors). LSP-only: `tower-lsp` + `lsp-types`, optional `salsa` (v0.3+). SMT-only: `easy-smt` then `z3`.

**Codegen freshness** is gated in CI (`xtask codegen --check`): a grammar change not re-codegen'd fails the build.

---

## 4. `libpure` — Parser, Model Loader, Resolver

`libpure` is the deterministic, no-LLM foundation. It turns (a) Pure source files and (b) a model into two artifacts every subcommand consumes: a **lossless CST + typed AST** with byte-accurate spans, and a **ResolvedModel** graph plus on-demand **source-threaded resolution** of navigation chains yielding required `%latest` arity per hop.

libpure emits **facts + structural-impossibility diagnostics only** (lex/parse errors, unresolvable references). It emits **no policy** — wrong-arity is a lint Warning computed by `pure-analyzer-analysis` comparing the fact against the surface.

### 4.1 Lexer (`pure-analyzer-lexer`)

`logos`-derived DFA emitting `Vec<(SyntaxKind, TextRange)>`. Token classes (1:1 with Core/M3 lexers):

- **Date family (longest match first):** `DATE_TIME %YYYY-MM-DDThh:mm:ss`, `STRICT_DATE %YYYY-MM-DD`, `LATEST_DATE '%latest'`, then bare `PERCENT '%'`.
- **Symbols:** `TILDE ~`, `DOLLAR $`, `ARROW ->`, `PIPE |`, `AT @`, `COLON :`, `NEW_SYMBOL ^`, `DOT .`, `COMMA ,`, `PATH_SEPARATOR ::`, `PAREN/BRACKET/BRACE` open/close, `EQ ==`, `NEQ !=`, arithmetic.
- **Keywords (M3):** `all let allVersions allVersionsInRange toBytes`. (`JoinKind`, operator names are `qualifiedName`, not keywords.)
- **Literals:** `IDENT INTEGER STRING BOOLEAN`.
- **Island raw tokens (NOT a logos mode stack — see §2.3):** `HASH_STORE_OPEN '#>{'`, `HASH_ISLAND_OPEN '#{'`, `NAV_PATH_BLOCK '#/' (~[#])* '#'` (whole token), `HASH '#'`, `BRACE_OPEN '{'`, `BRACE_CLOSE '}'`, `ISLAND_END '}#'`. Balancing is the parser's job.
- **Trivia:** whitespace + `//` and `/* */` comments are lexed as trivia tokens and **kept in the green tree** (losslessness — required for `fmt`).

### 4.2 Parser (`pure-analyzer-parser`) — resilient RD + Pratt, event-based

Matklad event model: the parser never returns `Result` for structure; it emits `[Open(kind) | Advance | Close]` plus a `Vec<Diagnostic>`; a builder folds events into a `rowan` GreenNode. Any node can hold any children (dynamically-typed CST) → error tolerance. A typed `AstNode` view (ungrammar-codegen'd) is layered on top. **`SyntaxKind(u16) ↔ enum` uses `num-derive` `FromPrimitive`, never `unsafe transmute`.**

**Recovery:** each rule carries a recovery set = union of FIRST/FOLLOW plus recursively-accumulated ancestor follow-sets. On an unexpected token emit an `Error` node up to the nearest recovery token, record a diagnostic, continue. **Every recovery loop is guarded by a "made progress?" check plus a bounded fuel counter** (mitigates matklad's infinite-loop pitfall; enforced by cargo-fuzz).

**Expression parsing = Pratt/TDOP.** Postfix `->fn(...)` arrow-chaining and `.prop` navigation are the tightest left-assoc postfix operators; then `== / !=`; then arithmetic.

**Intentional permissiveness at over-admission sites (resolving a critique contradiction).** The parser is **deliberately permissive** at the four PureCARD over-admission productions so that `validate`'s V2 pass can emit *targeted* diagnostics with precise spans (rather than a generic parse error). The parser therefore does **not** claim rule-for-rule ANTLR equivalence; the acceptance boundary is pinned by the **differential corpus** (§8), not by construction. Specifically, the parser accepts (and tags with a distinguishable CST shape) each of: bare `(a,b)` value tuples, non-literal bracket indices, `~col` in arbitrary expression position, and `.prop(...)` milestoning-parameter shapes with 0 or 3+ dates / trailing commas. `validate` V2 owns their rejection.

**Islands as first-class AST nodes.** A `#…#` region is an `Island { discriminator, raw_span, parsed: Option<StoreTablePointer | NavPathBlock> }`. Only `#>{…}#` is deep-parsed (for the pointer `(dbPath, table)`); everything else stays opaque with a span.

**Lambda bodies are `codeBlock`s** (statement lists with `let`), not single expressions. The AST models this.

**AST shape (typed view over the CST; ungrammar codegen produces the real structs; every node carries `TextRange`):**

```text
SourceFile     = (FunctionDef | ClassDef | AssociationDef | ProfileDef | QueryExpr)*
QueryExpr      = Primary Postfix*
Primary        = AllExpr | Island | Variable | Literal | NewInstance | Lambda | RelationTypeExpr
Postfix        = PropertyNav | ArrowCall | BracketIndex
PropertyNav    = '.' name:Ident  args:CallArgs?          // CallArgs may be MilestoningArgs or ordinary
MilestoningArgs= '(' (LATEST_DATE (',' LATEST_DATE)?)? ')'| ordinary-args   // surface only; classified by model
ArrowCall      = '->' fn:QualifiedName '(' Expr* ')'
BracketIndex   = '[' (String | Integer | <other:flagged>) ']'
AllExpr        = recv:QualifiedName '.' ('all'|'allVersions'|'allVersionsInRange') '(' args? ')'
Lambda         = '{' params:Ident* '|' body:CodeBlock '}'  |  param:Ident '|' body:CodeBlock
CodeBlock      = Stmt (';' Stmt)*                          // Stmt = LetStmt | Expr
LetStmt        = 'let' Ident '=' Expr
ColSpec        = '~' name:Ident (':' body:(Type Mult? | Lambda) reduce:(':' Lambda)?)?
ColSpecArray   = '~' '[' ColSpec (',' ColSpec)* ']'
Island         = discriminator + raw_span + parsed?
// model-def layer (Domain):
ClassDef       = 'Class' Stereotype* path:QualifiedName ('extends' QualifiedName (',' QualifiedName)*)?
                 '{' Property* QualifiedPropertyDef* '}'
Property       = name:Ident ':' targetType:QualifiedName mult:Multiplicity?
AssociationDef = 'Association' Stereotype* path:QualifiedName '{' Property Property '}'
Stereotype     = '<<' profilePath '.' value '>>'
```

**Public surface:**

```rust
pub struct Parse { pub green: GreenNode, pub errors: Vec<Diagnostic> }
pub fn parse_query(text: &str, file: FileId) -> Parse;   // M3 entry (query/function body)
pub fn parse_model(text: &str, file: FileId) -> Parse;   // Domain entry (Class/Association/Profile)
pub fn root(&Parse) -> ast::SourceFile;
```

**Invariants:** total parsing (never panics/bails); span completeness (every node + diagnostic has a byte-accurate `(FileId, TextRange)`); losslessness (all trivia retained).

### 4.3 Model loader (`pure-analyzer-model`)

```rust
pub enum ModelSource { PmcdJson(PathBuf), PureModelFile(PathBuf) }
pub fn load_model(sources: &[ModelSource]) -> Result<ModelGraph, ModelError>;

pub struct ModelGraph {
    classes:      BTreeMap<QName, ClassInfo>,
    by_path:      BTreeMap<QName, ClassId>,
    associations: Vec<AssocInfo>,
    // stores: deferred to a later milestone (§7.4)
}
pub struct ClassInfo {
    path: QName,
    supertypes: Vec<QName>,                    // generalization chain (MUST be present)
    temporal: Option<Temporal>,                // own stereotype only; effective one is resolved up-chain
    properties: BTreeMap<Name, PropInfo>,      // simple + assoc-contributed ends
    qualified_properties: BTreeMap<Name, QpInfo>,
    provenance: Provenance,                     // Pmcd | PureFile   (per-class)
    coverage_gap: bool,                         // true if stereotype/assoc info could not be confirmed
}
pub struct PropInfo { name: Name, target: TypeRef, mult: Multiplicity, from_assoc: bool }
pub struct QpInfo {
    name: Name, target: TypeRef, mult: Multiplicity,
    kind: QpKind,                               // classification below
    signature: Option<Vec<TypeRef>>,            // for user QPs (compiled arg types)
}
pub enum QpKind {
    UserQualified,                              // user-defined; args = signature; NO %latest check
    MilestonedPoint,                            // generatedmilestoningproperty; %latest arity applies
    AllVersions,                                // accepts 0 args, strips context
    AllVersionsInRange,                         // takes (d1,d2) range; own signature
    EdgePoint,                                  // point-in-time, target arity
}
pub enum Temporal { Bitemporal, BusinessTemporal, ProcessingTemporal }  // arity 2/1/1; None=0
pub struct AssocInfo { end_a: (QName, PropInfo), end_b: (QName, PropInfo), temporal: Option<Temporal> }
pub enum Provenance { Pmcd, PureFile }
```

**Loading from PMCD JSON (authoritative when present):** for each `Class`: path; `supertypes` from `superTypes`; `temporal` by scanning `stereotypes` for `profile == "meta::pure::profiles::temporal"`; `properties`; `qualifiedProperties` classified by their milestoning stereotype (`generatedmilestoningproperty` → `MilestonedPoint`; name-suffix `AllVersions`/`AllVersionsInRange` + stereotype → those kinds; unbounded-mult generated → `EdgePoint`; otherwise `UserQualified` with `signature`). For each `Association`: materialize **both** ends as `PropInfo` on their owning classes (`from_assoc = true`) **and** record `AssocInfo.temporal`.

**Loading from a Pure model file (first-class, engine-free):** parse with `parse_model` (Domain grammar) and lower `ClassDef` (incl. `extends`), `AssociationDef` (incl. its stereotype), `ProfileDef`. Read `<<temporal.X>>` directly. **Closed-world vs open-world:** a Pure file often omits associations or generated qualified properties. Therefore:

- **Missing generated milestoned end** → **synthesize** it from the target class's `temporal` stereotype (the single source of truth), tag class `coverage_gap = false` for that synthesis but attach `PUR2100 model.derived-qualprop` (Info).
- **A stereotype or association that cannot be confirmed** → set `coverage_gap = true` on the affected class. `lint` then runs **open-world**: an unknown property or unconfirmed stereotype **downgrades** to a `Recoverable`-flagged Warning (`MODEL_INCOMPLETE`), never a hard Error (§4.4).

**Provenance policy:** PMCD classes are **closed-world** (unknown property = real Error). PureFile classes with `coverage_gap` are **open-world** (unknown = recoverable, skip the hop). This directly implements the ground-truth caveat: arity is undecidable without the target's temporal stereotype, so pure-analyzer never emits a hard arity Error against a class whose stereotype it could not confirm.

**Property/qualified-property shadowing precedence (explicit rule).** When a plain property and a generated milestoned qualified property share a name: the **generated milestoned qualified property is the milestoned nav** (`.prop(%latest…)` / context-gated `.prop`); the underlying base property is reachable via the generated `AllVersions` / edge-point forms. Lookup precedence at a hop: (1) generated milestoned QP, (2) user QP, (3) plain property, (4) association-contributed end — searched **up the generalization chain** (see resolver). Record the resolved `QpKind` so the arity gate keys on it.

**Merge:** multiple `--model` inputs merge; last-wins on QName collision + `PUR900 model-merge-conflict` (Warning). Load failure (malformed JSON / unparseable model file) → `ModelError` → exit 3.

### 4.4 Resolver (`pure-analyzer-resolve`) — source-threaded arity + a local type environment

This is the core value. It must analyze navigation chains **as they actually appear** — overwhelmingly **inside lambda bodies rooted at `$param`**, not as bare top-level `Class.all().a.b.c` chains. That requires a small **type environment**, not a single `cur_class`.

**Type environment.**

```rust
pub enum EntryType {
    Class(ClassId),          // a class instance
    RelationRow(RowType?),   // a Relation row (columns known only if store model loaded; else None)
    Unknown,                 // could not infer
}
pub struct TypeEnv { vars: BTreeMap<Name, EntryType> }   // $param and let-bound names
```

**Lambda-parameter binding (the load-bearing inference).** When descending into a relation-operator lambda, bind its parameter(s) to the **element type of the receiver**:

- Receiver rooted at `Class.all()` → the lambda param binds to `Class(ClassId)`.
- Receiver rooted at a `Relation<>` (`#>{db.Table}#` or the output of prior relation ops) → the lambda param binds to `RelationRow`. If the store model is not loaded, `RowType = None` → column refs are **not** flagged (recoverable), and milestoning arity does **not** apply to relation columns.
- `join`'s `{x,y|…}` binds `x`,`y` to the two operands' element types.
- `groupBy`/`extend` colSpec lambdas bind their param to the row/element type.
- `let x = <expr>` binds `x` to the inferred type of `<expr>` (best-effort: a `.all()` chain resolves to its final class; otherwise `Unknown`).

This is a **small, bounded local type-inferencer** — explicitly *not* full Hindley-Milner. Anything it cannot infer resolves to `Unknown` → arity checks on downstream hops are **suppressed** and surfaced once as `PUR2101 resolve.unknown-source` (Info, Recoverable). This is an honest scope call: v1 covers `Class.all()`-rooted lambda navigation (the common milestoned case); exotic higher-order threading degrades quietly rather than false-firing.

**Property lookup walks the generalization chain (MRO).** `lookup_property(model, class, name)` searches `class` then its `supertypes` transitively (breadth-first, first match wins), across generated-QP / user-QP / plain / association buckets per the precedence rule. Effective temporal stereotype is likewise resolved up the chain: `effective_temporal(class) = class.temporal.or_else(walk supertypes)`.

**The algorithm (the proven Stage-0 rule, corrected).**

```text
resolve_chain(model, root_expr, env) -> ChainResolution:
  (cur, ) = entry_type(root_expr, model, env)         // Class | RelationRow | Unknown
  hops = []
  for hop in ordered_property_navs(root_expr):         // left-to-right; skip ArrowCall/BracketIndex
     if cur is not Class(c):                            // RelationRow or Unknown
        emit PUR2101 resolve.unknown-source (Info) once; break   // no milestoning threading on relations
     Csrc = c
     Ssrc = effective_temporal(model, Csrc)            // FRESH per hop — NOT propagated
     pinfo = lookup_property_up_chain(model, Csrc, hop.name)
     if pinfo is None:
        if model.class(Csrc).is_open_world():           // PureFile + coverage_gap
           emit MODEL_INCOMPLETE (Recoverable Warning); break     // never a hard error
        else:
           emit PUR2002 unknown-property (Error, closed-world); break    // stop threading (no cascade)
     Ctgt = target_class(pinfo)
     Stgt = effective_temporal(model, Ctgt)
     hop_kind = pinfo.qp_kind_or_plain()
     // ---- arity fact (only for GENERATED milestoned point props) ----
     match hop_kind:
        MilestonedPoint | EdgePoint:
           base = arity(Stgt)
           context_gated = Ssrc.is_some() && (Ssrc == Stgt || Ssrc == Some(Bitemporal))
           accepted = if base == 0 { {0} }
                      else if context_gated { {0, base} }
                      else { {base} }
           surface = count_date_args(hop)               // %latest / DATE / $var count in the milestoning slot
           hops.push(Hop{ Csrc, Ctgt, Stgt, base, accepted, context_gated, surface, kind: Milestoned })
        AllVersions:
           hops.push(Hop{ ..., accepted:{0}, kind: AllVersions })
        AllVersionsInRange:
           hops.push(Hop{ ..., kind: RangeSig(pinfo.signature) })   // checked vs range sig, not %latest
        UserQualified:
           // NO %latest check; validate arg count vs pinfo.signature if known, else recoverable-skip
           hops.push(Hop{ ..., kind: UserQP(pinfo.signature) })
        Plain:
           hops.push(Hop{ ..., accepted:{0}, kind: Plain })
     // milestoned association overlay:
     if pinfo.from_assoc && assoc_temporal(model, Csrc, hop.name).is_some():
        fold association arity into accepted (per assoc stereotype)
     cur = Class(Ctgt)                                  // thread the class; context is recomputed next hop
  return ChainResolution{ hops, final_type: cur }

arity(t) = match t { Some(Bitemporal)=>2, Some(Business|Processing)=>1, None=>0 }
```

**Fact/policy boundary.** `resolve_chain` emits only **structural** diagnostics (`PUR2002` unknown-property, `PUR2101` unknown-source, `MODEL_INCOMPLETE`). It produces `Hop { surface, accepted, kind, … }`. **`lint` compares `surface` vs `accepted`** to emit the policy Warning/Error `PUR2001`.

**Worked example (deep milestoned chain):**
`Trade.all()->filter(x | $x.product(%latest).clientSegment.defaultClientSegment == ...)` with `Trade`=bitemporal, `product` target `Product`=businesstemporal, `clientSegment` target plain, etc.

- lambda `x` binds to `Class(Trade)`; chain root `$x` → `Class(Trade)`.
- hop `product`: `Ssrc = effective_temporal(Trade) = Bitemporal`; target `Product` business → base 1; context-gated (`Bitemporal ∈ {Business, Bitemporal}` → yes) → accepted `{0,1}`; surface 1 → OK.
- hop `clientSegment`: `Ssrc = effective_temporal(Product) = Business`; target plain → base 0 → accepted `{0}`; surface 0 → OK.
- hop `defaultClientSegment`: `Ssrc = None` (source plain); target plain → `{0}`; surface 0 → OK.

Same property name on a different source class threads to a different target and thus different arity — handled because lookup is always on the current threaded class.

**Deliberately deferred:** store-column resolution (§7.4). In v1 the `#>{db.Table}#` island yields only `(dbPath, table)` lexically; relation column linting is off (recoverable) unless a store model is loaded in a later milestone.

**Public surface:**

```rust
pub fn resolve_chain(model: &ModelGraph, root: &ast::QueryExpr, env: &TypeEnv) -> ChainResolution;
pub fn required_arity(model: &ModelGraph, target: ClassId) -> u8; // 0|1|2
```

---

## 5. Subcommand Contracts

All subcommands are passes over `(CST, ResolvedModel?)`. `validate` needs no model; `lint`/`eq`/`diff` use the resolved model. Dispatch **gates `resolve()` on `cmd.needs_model()`** so `validate` never runs the resolver.

### 5.1 `validate` — grammar fidelity + over-admission guards

**Honest guarantee (relabeled per critique).** validate does **not** provably "reject exactly what the engine rejects" — a hand-written RD+Pratt parser is not equivalence-provable against ANTLR4 ALL(*) by any finite corpus. The guarantee is: **validate rejects the enumerated over-admission classes and continuously converges on the engine via a differential corpus** (§8); residual divergence is a tracked liability. validate has **two sub-scopes**, both honest:

- **Grammar-validate (CFG, model-free):** parser-native syntax errors + the genuinely *grammatical* rejections.
- **Semantic-shape-validate (still model-free, but compiler-level):** shape rejections the engine makes in the compiler, not the grammar. These are clearly labeled as such.

**Phase V1 — parser-native diagnostics.** The resilient parser's `Vec<Diagnostic>` (unexpected token, unbalanced parens/braces/brackets, unterminated island). Surfaced as `PUR0xxx`. Island balance uses parser-managed depth (§2.3): unbalanced → `PUR0101 unterminated-island`.

**Phase V2 — over-admission guards** (one linear CST walk; each a local subtree predicate). **Corrected roster** (V103 and V110 from the earlier draft are cut/re-scoped):

| Code                                 | Kind  | Scope label             | Detection                                                                                                                                                                                                                                                                                                                                             | Fix  |
| ------------------------------------ | ----- | ----------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---- |
| `PUR1201` paren-tuple                | Error | **grammar**             | a `(` comma-list used as a **value** expression whose parent is none of {type-argument position (`relationType`), `functionExpressionParameters`, milestoning-date param}.                                                                                                                                                                            | none |
| `PUR1202` illegal-bracket-index      | Error | **grammar**             | `propertyBracketExpression` whose single child is not a bare `STRING`/`INTEGER` (reject `$x[$i]`, `$x[a+1]`, `$x[]`, `$x[0..1]`).                                                                                                                                                                                                                     | none |
| `PUR1204` malformed-milestone-parens | Error | **grammar**             | a milestoning-date parameter node (the parser's flagged over-permissive shape) not matching exactly 1-or-2 `LATEST_DATE`: trailing comma `.p(%latest,)`, or 3+ `%latest`. NOTE: `.p($businessDate)` / `.p(%2020-01-01)` are ordinary `functionExpressionParameters` — **do not** flag. Empty `.p()` is a zero-arg call handled by lint, **not** here. | none |
| `PUR1210` unknown-joinkind           | Error | **shape** (closed enum) | `JoinKind.X`, `X ∉ {INNER, LEFT}`.                                                                                                                                                                                                                                                                                                                    | none |

**Explicitly removed:**

- **`~col` outside a colSpec (old V103): CUT.** `columnBuilders` IS an `atomicExpression` — `~col` is grammar-legal anywhere. Rejecting it is over-rejection (a soundness failure for validate). If a *specific* engine-illegal residue exists (e.g. a dangling `:reduceLambda` with no host colSpec), add it **only** with a corpus-verified fixture; do not reject bare `~col`.
- **empty `.prop()` (old semantic milestone rule): NOT a validate rejection.** It is a legal empty `functionExpressionParameters` (grammatically fine); whether it is a *wrong* 0-arg milestoned nav is **lint's** job (`PUR2001`, model-driven).
- **unknown relation-function (old V110): CUT from validate.** The grammar accepts any `->qualifiedName(...)`; real code calls user helper functions (`->my::lib::fn()`). "Is this a known relation op" is a **semantic** check needing a symbol registry — it belongs in **lint** (optional, model/registry-driven, whitelisting user functions), not in model-free validate. pure-analyzer v1 does **not** warn on unknown relation functions in validate.

validate **NEVER** needs the model. It accepts all three grammar-legal `%latest` arities (0/1/2) — arity correctness is lint's job.

**Exit:** `1` if any Error, else `0`. `--strict` escalates any shape-level warnings to errors.

### 5.2 `lint` — milestoning arity, unknown-property, cardinality

`lint` builds `ModelGraph` once (Arc-shared), then does **one** CST walk collecting navigation-rooted expressions (including those inside lambdas via the TypeEnv, §4.4); each calls `resolve_chain`, emitting findings in a single left-to-right pass. Findings sorted by `(file, span.start, code)`.

**`PUR2001` wrong-milestoning-arity** (the core). For each hop with `kind == Milestoned`: compare `surface` vs `accepted`.

- `surface ∈ accepted` → OK.
- `surface ∉ accepted` → **`PUR2001`**. Message: `navigation .<prop> targets <Ctgt> (<stereotype>) which requires <accepted> milestoning date arg(s), found <surface>`. Primary span = the milestoning-arg node (or the property name if absent); secondary span = the target class stereotype decl.
- **Severity:** `Error` **only** when the class is **closed-world (PMCD)** and the target stereotype is confirmed. Under any under-resolution — PureFile `coverage_gap`, unconfirmed stereotype, unknown QP signature, un-inferrable lambda-param type, missing supertype — **downgrade to Warning with `reason = Recoverable(MODEL_INCOMPLETE)`.** A soundness-first tool never hard-errors under under-resolution.

**`PUR2002` unknown-property** (Error, closed-world only; Recoverable Warning open-world). Searched up the generalization chain and across all property buckets (so inherited / association / generated properties never false-fire). Threading stops after `PUR2002` (no cascade). Fix: nearest-name (edit-distance 1) — **Suggested, never auto-applied.**

**`PUR2003` cardinality/multiplicity misuse** (family; fires ONLY when *both* declared multiplicity and static usage arity are determinate constants — never guessed, never a false Error):

- to-many nav (`[*]`/`[1..*]`) then a consumer requiring upper bound ≤1 (from a small builtin signature table) → Warning; Suggested fix `->toOne()`. Unknown consumer mult → not flagged.
- `->toOne()`/`->toOneMany()` on an already-`[1]` value → Info; MachineApplicable fix (remove).
- `^Class(prop=[a,b,c])` where `prop` is `[0..1]` (both sides constant) → Warning; no fix. Anything needing dataflow → skipped.

**Auto-fix policy (critique-mandated).** In v0.1, **all M/P fixes ship as `Suggested` only (shown, never auto-applied)** until the resolver is corpus-hardened against the user-QP / inheritance / context / date-variable cases. Auto-inserting `(%latest,%latest)` on a mis-diagnosed hop, or on a hop whose correct form used `$businessDate` or the context-gated 0-arg, would corrupt correct source. A `PUR2001` fix may be promoted to `MachineApplicable` only when: target stereotype is confirmed (PMCD), the required arity is a **single** value, **and** no date variable/DATE literal is in scope for that hop. Even then, `--fix` applies only `MachineApplicable` fixes.

**Relation-column refs** (`~col`, `$r.col`) are checked **only** when the root is a concretely-resolvable `#>{db.Table}#` **with a loaded store model** (deferred; §7.4). Otherwise not flagged — recorded at most as `Recoverable(RelationRowTypeUnknown)`.

**Model required.** `lint` without `--model`: default → degrade to syntactic-only, emitting `MODEL_INCOMPLETE` where arity would have applied; with `--require-model` → `PUR901`, exit 3. lint will **not** silently masquerade as validate.

**Exit:** `1` if any Error (deny-escalated arity errors count), else `0`.

### 5.3 `eq` / `diff` — SOUND, INCOMPLETE, 3-valued equivalence

`pure-analyzer eq A.pure B.pure [--model m]` returns one of:

- **`EQUIVALENT`** — proven equal by canonical-normal-form (NF) match.
- **`NOT_EQUIVALENT`** — refuted by a provable output-schema/type difference **or** an interpreter-validated concrete WITNESS over a **model-legal** input.
- **`INDECISIVE(code)`** — neither; carries exactly one stable reason code (FUNDAMENTAL or RECOVERABLE).

`pure-analyzer diff` runs the same engine and always renders an aligned NF tree-diff (the driver view). **`diff`'s always-on structural delta is scoped explicitly** as its own (small) edit-script over the two `CanonRel` DAGs, span-anchored to original A/B source, each divergent node tagged with a reason code. (If the edit-script proves heavy, `diff` may defer the delta rendering to v0.3; the verdict itself always ships.)

#### The sacred soundness invariant

- **(I1)** `EQUIVALENT` iff `NF(A) ≡ NF(B)` by structural equality, where NF is the fixpoint of a rule set every member of which is a **proven meaning-preserving identity under the active equivalence relation**. Any construct without a sound rule is frozen as an **OPAQUE canonical token** so NFs never coincidentally merge.
- **(I2)** `NOT_EQUIVALENT` iff (a) a provable output-schema/type mismatch observable under the active relation, or (b) a concrete **model-legal** witness on which the reference interpreter computes different outputs, where every operator on the witness path is fully interpreted and the divergence does **not** depend on an unspecified tie-break of a non-total order.
- **(I3)** Everything else is `INDECISIVE`. UNKNOWN / timeout / budget-exhaustion / opaque-function / unmodeled-op all map to `INDECISIVE`, never to a commitment.

Mechanical & deterministic: no engine, no solver, no LLM at runtime; witness enumeration is fixed-seed and reproducible.

#### Semantic domain & the active equivalence relation

A Relation value is a **BAG** (multiset) of ROWS over an **ORDERED, NAMED** column schema; `distinct` collapses to a SET; `sort` imposes order making the tail a LIST; `limit/take/slice` truncate it. Default relation = **BAG, ORDERED-SCHEMA**: identical **ordered** `(name,type)` schema AND identical multiset of rows (row order insignificant until a sort is in scope, after which the ordered prefix must match). Flags:

```text
--column-order {significant|insignificant}   default significant  (safest for committing EQUIVALENT)
--multiplicity {bag|set}                      default bag          (SQL-faithful)
```

The relation is fixed **before** any rewrite fires; every rule is proven meaning-preserving under exactly that relation.

**Column order is significant (critical soundness correction).** `Relation<>` is an ordered tuple. `select(~[a,b])` and `select(~[b,a])` produce differently-shaped relations and are **NOT** equivalent under the default. Therefore the normalizer **must NOT** sort `select`/`extend` output column lists into a set. Only **groupBy key sets** and **join equi-condition conjunct sets** are legitimately unordered.

#### Scope (honest)

**IN (decidable core):** `filter`, `select`/`rename` (Project), `extend` (scalar, total), `groupBy`, `join` (INNER/LEFT), `distinct`, `sort`, `limit/take/drop/slice`, `size` — under cosmetic rewrites (alpha-renaming, filter merge, project fusion, AC-normalization of *boolean predicates only*, null-safe constant folding) + structural refutation (wrong key set, wrong nav edge/`%latest` arity, missing distinct, provably-total-order truncation, concretely-separable differences).

**OUT (INDECISIVE, FUNDAMENTAL):** window/OLAP frame equivalence, pareto/top-per-group under ties, multi-step fiscal accumulation, division/ratio, bitemporal as-of equivalence, and any scalar-predicate equivalence outside the interpreted whitelist. `over`/`asOfJoin`/`pivot` are OPAQUE. **No solver in the sound core.**

#### Pipeline & RA-IR (`pure-analyzer-ir`)

```text
parse(A),parse(B) -> CST
lower(CST, model?) -> RA-IR plan (typed relational-algebra tree, spans + resolved schema)
normalize(plan) -> NF                         // confluent, terminating rewrite to fixpoint
if NF(A) ≡ NF(B): EQUIVALENT
else: refute(A,B) -> NOT_EQUIVALENT+witness | classify_indecisive(A,B) -> INDECISIVE(code,bucket)
```

RA-IR nodes (each carries spans + resolved output schema when model present):

```text
Source(id, schema?)                 // #>{db.Table}# or Class.all(); id = canonical path string
Filter(in, pred)
Project(in, [out <- src|expr])      // ORDERED output columns; select+rename fused here (restrict/rename only)
Extend(in, [col = scalarExpr])
Window(in, overSpec, [col=winFn])   // OPAQUE for core
GroupBy(in, keys[], [aggName = (mapLambda, reduceLambda)])
Join(left, right, kind∈{INNER,LEFT}, cond)
Distinct(in)
Sort(in, [(col,dir)])
Limit(in, n) / Slice(in, lo, hi)    // take/drop/slice canonicalized here
AsOfJoin / Pivot                    // OPAQUE for core
```

Lowering resolves each `.prop` via `pure-analyzer-resolve`. **Edge identity** for the wrong-edge refutation is keyed by **`(association-element-id, navigation-direction, source-class, property-name, target-class, milestoning-context)`** — NOT three names. This distinguishes the two directions of one association and inheritance/override cases. If the model is absent, edges/columns stay symbolic → `IND_UNRESOLVED_SCHEMA` where it would have decided.

#### Canonical normal form (the heart) — confluent, terminating

Applied in fixed priority order; each rule is a proven identity under the active relation.

- **2a Alpha-normalization:** rename lambda params and colSpec binders to positional De-Bruijn `$p0,$p1,…` top-down. (Sound: bound-variable renaming.)
- **2b Source canonicalization:** normalize package path + table/class name to a canonical string. (Leaf.)
- **2c Desugar:** `rename→Project`; `take(n)→Limit(n)`; `drop(k)→Slice(k,∞)`; `slice(a,b)→Slice`; `select/~col/~[..]` family → `Project` (**preserving column order**).
- **2d Scalar/predicate canonicalization — RESTRICTED to a null-safe, type-checked, boolean subset:**
  - flatten + sort **AND/OR** conjunct/disjunct **sets** by a total syntactic order; push NOT to leaves via De Morgan + comparison negation; orient comparisons canonically (`a>b ⇒ b<a`; keep `<,<=,==,!=`).
  - **Guards (soundness fences the critiques mandated):** (i) **Do not** treat `==`/`!=` as AC and flatten chained equality — non-boolean equality is not associative. (ii) **Do not** simplify `x==x`→true or `x AND x`→x for possibly-null `x` — under SQL 3VL `x==x` is UNKNOWN when null, so it changes which rows survive. Only apply idempotence/reflexive simplification when the operand is provably non-null (declared `[1..1]` non-null literal/column). (iii) **Constant-fold only** literal-only subterms whose fold matches engine numeric/date semantics exactly (pin integer vs decimal vs float, integer division, overflow, null propagation via the interpreter's pinned semantics, §5.3-interp). Any fold not provably engine-faithful is **not performed** (frozen). Functions outside the interpreted whitelist are frozen OPAQUE and compared structurally only.
- **2e Filter merge:** adjacent `Filter`s → one `Filter` over the sorted conjunction set. (Sound.)
- **2f Sound reordering — CHOOSE MAXIMAL PUSHDOWN (resolving the confluence conflict).** The canonical position for selections is **maximal pushdown toward sources**; the "stage order" is *descriptive documentation*, not a rewrite target, and the termination measure ("selections only move downward") is consistent with it.
  - Filter pushdown below Project **only after back-substituting** the predicate through the projection's `(out←src)` column map (survival alone is insufficient when Project renames).
  - Filter pushdown below Join when the predicate references one side only (INNER: either side; LEFT: only into the **preserved** side).
  - Project fusion (compose column maps, **order-preserving**).
  - Distinct idempotence (`Distinct∘Distinct = Distinct`). **Distinct elimination via a unique key is DROPPED from v1** — PMCD carries no table primary keys / column uniqueness (uniqueness is DB DDL, not the class model), and substituting association multiplicity `[1]` for row-uniqueness is a category error. Without a real uniqueness source, this rule risks a wrong `EQUIVALENT`; it is out of v1.
  - INNER-join operand reordering is applied **only** when it does not change the **resolved final output schema** under `column-order=significant` — evaluated against the whole-downstream resolved schema, **not** the local subtree. If it would reorder output columns, leave as-is → possibly `IND_MISSING_REWRITE` (honest).
  - Any reorder not provably sound is **frozen**, never forced.
- **2g Dead-column elimination — GUARDED.** Drop `Extend`/`Project` outputs never referenced downstream **only when the dropped expression is provably TOTAL** (no division, no partial cast, no throwing function). A dropped *erroring* extend (`~c = x/0`) is meaning-changing if the engine evaluates extends strictly. Non-total → not eliminated (frozen). (Requires the pinned partiality model, §5.3-interp.)
- **2h Serialize** NF to a canonical S-expression / structural key; compare by structural equality.

**Termination:** lexicographic measure (operator-count, total predicate size, pipeline-disorder rank where selections only move downward) strictly decreases per rule; a max-iteration guard trips → `INDECISIVE(IND_MISSING_REWRITE)` rather than risk a bad merge. **Confluence:** rules non-overlapping or joinable; overlaps resolved by fixed priority; result order-independent up to the guard. **Both are ship gates** verified by random-rule-order property tests (§8).

#### Refutation arm (fires after NF mismatch)

- **3a Output-schema refutation (no data):** compute each NF's ordered `(name,type)` output schema via the resolver. If provably different **and observable under the active relation** → `NOT_EQUIVALENT` (SCHEMA witness). Wrong-`%latest`-arity or wrong-edge that changes the output *type* is caught here. **Multiplicity observability is defined:** under the default bag/ordered-schema relation, a `(name,type)` match with differing multiplicity (`[1]` vs `[0..1]`) is **NOT** by itself refutable (multiplicity is not a value-level observable of the bag) — it maps to `IND_UNRESOLVED_SCHEMA`/no-refute, never a commit either way.
- **3b Targeted structural witnesses (construct-then-verify — every candidate CONFIRMED by 3c before emission):** wrong key set; missing distinct; wrong edge (same output type); total-order truncation. Each constructs a small **model-legal** state (see 3c constraints) separating the two plans.
- **3c Reference bag interpreter + BOUNDED WITNESS SEARCH (the general refuter — ships in M4b only after its semantics corpus passes):**
  - **Enabled only** when every scalar function in both plans is in the interpreted whitelist; else DISABLED → `IND_OPAQUE_FUNCTION_IN_WITNESS`.
  - **MODEL-LEGAL enumeration only (the single biggest soundness hole — fixed).** The enumerator must generate only states the model permits: honor `[1..1]` non-null / exactly-one multiplicity, class/relation constraints, and **milestoning disjointness** (per-entity businessDate validity intervals must not overlap; as-of rows disjoint). A witness found on an *illegal* state can refute genuinely-equivalent queries → I2 violation. Enforce constraints **during** generation.
  - **Pinned SQL 3VL / null semantics (mandatory, engine-differential-tested per function).** The interpreter must reproduce engine semantics EXACTLY: `filter` keeps only TRUE (UNKNOWN drops the row); equi-join does not match on null keys (`null ≠ null`); `sum/min/max` ignore null, `count(col) ≠ count(*)`; `distinct`/`groupBy` group all nulls into one group; sort null-ordering per engine; LEFT-join null padding; empty-group `groupBy` behavior; date/string collation. Every whitelisted function's semantics is **pinned by an engine differential test before it may drive a refutation.** Naive two-valued logic is a soundness bug.
  - Value domain `{null,0,1,2,"","a","b", two boundary dates}`, row counts `0..K` (default K=3), fixed deterministic enumeration, budget.
  - If outputs differ on any legal input → `NOT_EQUIVALENT` + that input (rendered as executable Pure Relation literals over the shared source, so a human can paste-and-run against the engine) + the two outputs + the divergence kind (`row_present | multiplicity | schema | value`).
  - **Tie-break soundness fence:** evaluating `Sort/Window/Limit` over a NON-total order marks the ambiguous tail ORDER-AMBIGUOUS; a difference manifesting only under a particular tie-break is **SUPPRESSED** (the engine's tie-break is unspecified). Such pairs fall through to `INDECISIVE(IND_WINDOW / IND_ORDER_UNDERDETERMINED)`.
  - **Reflexivity:** syntactically-identical NF → `EQUIVALENT` even with windows/non-total limits (a query equals itself; where non-deterministic, `EQUIVALENT` denotes equality of the relation *value* as the set-of-possible-outputs).
  - Budget exhausted without separation → `INDECISIVE(IND_WITNESS_BUDGET_EXHAUSTED)` — **NEVER** upgraded to EQUIVALENT (absence of a small witness is not a proof).

#### INDECISIVE reason-code taxonomy

**FUNDAMENTAL (no static procedure / would need data → postpone):**
`IND_WINDOW`, `IND_PARETO`, `IND_MULTISTEP_FISCAL`, `IND_DIVISION_RATIO`, `IND_MILESTONING_ASOF`, `IND_ORDER_UNDERDETERMINED`, `IND_OPAQUE_PREDICATE`, `IND_DIFFERENT_SOURCES`.

**RECOVERABLE (conservative checker / missing sound rewrite → backlog):**
`IND_MISSING_REWRITE`, `IND_UNMODELED_OP` (pivot/asOfJoin/exotic slice), `IND_OPAQUE_FUNCTION_IN_WITNESS`, `IND_UNRESOLVED_SCHEMA`, `IND_WITNESS_BUDGET_EXHAUSTED`, `IND_PREDICATE_NORMAL_FORM_GAP`, `IND_UNPARSEABLE` (a file failed to parse / an island could not be deep-parsed — its own code with an error exit), `MODEL_INCOMPLETE`.

**Re-bucketing note:** the *decidable-with-a-known-unique-key* subset of order-underdetermination is RECOVERABLE, not FUNDAMENTAL — but since v1 has **no uniqueness-key source**, all such cases currently land in `IND_ORDER_UNDERDETERMINED` (FUNDAMENTAL) honestly. Every INDECISIVE carries exactly one primary code + bucket. RECOVERABLE codes are the engineering backlog; FUNDAMENTAL are honestly postponed. This makes pure-analyzer a **driver**, not just a scorer.

**Two files must share one model.** `--model` applies to both A and B. If the two queries legitimately target different sources, `IND_DIFFERENT_SOURCES`; mismatched-source comparisons are flagged up front, and the cross-model column-universe assumption of 3c is only used when both read the same named source.

**Exit:** `0` EQUIVALENT, `1` NOT_EQUIVALENT, `2` INDECISIVE, `3` usage/model-load, `4` internal.

### 5.4 `fmt` — canonical form

**Two modes, cleanly separated (correcting the "falls out free from eq" overstatement).**

- **Default layout mode — a v0.1 freebie built from the lossless CST, ZERO eq dependency.** Pretty-print the CST preserving comments/whitespace semantics: one `->op(...)` per line with aligned arrows; `~[..]` colSpec arrays one column per line beyond N; islands emitted verbatim single-line; canonical spacing around `-> | : ~ @ ^`; canonical `%latest` arg lists. **Semantics-preserving = layout only.** It must **NEVER** reorder colSpecs, sort keys, groupBy columns, or any semantically-significant order — those are meaning-changing. **Idempotent:** `fmt(fmt(x)) == fmt(x)` (fuzz-tested).
- **`--canonical` mode (v0.2) — serialize the eq NF back to Pure.** Semantics-preserving rewrite (alpha-named binders, canonical predicate/operator order) built **only** from the proven §5.3 identities. Comments dropped or best-effort re-attached by span. **Emitter injectivity is a proof obligation:** no two distinct NFs may emit the same bytes (fuzzed), so the oracle holds: for queries inside the decidable core, `eq(A,B)=EQUIVALENT` **iff** `fmt --canonical A` and `fmt --canonical B` are byte-identical.

**Modes/exit:** default = atomic in-place rewrite; `--stdout` print; `--diff` unified diff; `--check` no write, exit 1 if any file would change (CI gate), else 0; write-mode I/O failure → 4.

---

## 6. Diagnostic / Output Format, Exit Codes, Config

### 6.1 One Diagnostic model (four renderings)

`pure-analyzer-diagnostics` owns the single serializable struct; all layers emit only this; renderers live in `pure-analyzer-cli`.

```rust
pub struct Diagnostic {
  pub code: DiagCode,          // stable string id, e.g. "PUR2001"
  pub severity: Severity,      // Error | Warning | Info | Hint
  pub message: String,
  pub primary: Label,          // { file, span:(start,end) byte offsets, note }
  pub secondary: Vec<Label>,
  pub fix: Option<Fix>,        // { title, applicability, edits: Vec<TextEdit{span, new_text}> }
  pub verdict: Option<Verdict>,// eq/diff: Equivalent | NotEquivalent{witness} | Indecisive{reason}
  pub reason: Option<ReasonCode>, // set iff Indecisive; carries bucket
  pub url: Option<String>,     // doc-link into docs/reason-codes/<code>
}
pub enum Severity { Error, Warning, Info, Hint }
pub struct Label { pub file: FileId, pub span: TextRange, pub note: String }
pub enum Applicability { MachineApplicable, Suggested, Unsafe }
pub enum ReasonBucket { Fundamental, Recoverable }
pub struct ReasonCode { pub id: &'static str, pub bucket: ReasonBucket, pub blurb: &'static str }
```

**Renderers retain source text.** Because ariadne carets and the JSON `{line,col}` fields need the original bytes at render time, the driver **keeps a per-file source + line-index table alive for the lifetime of rendering** (codespan `SimpleFiles`-style). This is the corrected memory story: green trees are dropped after their passes, but a lightweight source/line-index map is retained until render. For `--stdin` the in-memory buffer is authoritative (no re-read; avoids TOCTOU).

Renderings by `--format`:

- **human** (default on TTY): `ariadne` — labeled spans, carets, doc-link footer, grouped by file, `N errors, M warnings, K info` summary.
- **json**: single `{"schema_version":1,"files":[...],"diagnostics":[...],"summary":{...}}`; spans as byte `{start,end}` **and** 1-based `{line,col}` for consumers; `#[derive(Serialize)]`.
- **sarif**: SARIF 2.1.0 (`runs[].results[]`, `ruleId=code`, `level` from severity, `region` from span, one `reportingDescriptor` per code with `helpUri=url`). **May be budgeted to v0.2** if v0.1 timeline is tight; human+json are the v0.1 requirement.
- **lsp** (the LSP surface, not a `--format`): `Diagnostic → lsp_types::Diagnostic` (byte offsets → UTF-16 positions at the boundary only; `fix → CodeAction/WorkspaceEdit`).

### 6.2 Unified DiagCode registry (single authoritative namespace — `PUR`)

The **`PUR` scheme is authoritative**; the dotted-string and V/M/P/C schemes from component drafts are superseded.

| Range     | Meaning                                                                              | Examples                                                                                                                                                                      |
| --------- | ------------------------------------------------------------------------------------ | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `PUR0xxx` | lexer / island                                                                       | `PUR0101` unterminated-island; `PUR0102` bad-token                                                                                                                            |
| `PUR1xxx` | parser / grammar (validate), incl. over-admission                                    | `PUR1201` paren-tuple; `PUR1202` illegal-bracket-index; `PUR1204` malformed-milestone-parens; `PUR1210` unknown-joinkind                                                      |
| `PUR2xxx` | lint                                                                                 | `PUR2001` milestoning-arity-wrong; `PUR2002` unknown-property; `PUR2003` cardinality-misuse; `PUR2100` model.derived-qualprop (Info); `PUR2101` resolve.unknown-source (Info) |
| `PUR3xxx` | eq/diff verdict-carrying diagnostics (incl. `IND_*` reason codes as the `reason.id`) | `PUR3001` eq-verdict                                                                                                                                                          |
| `PUR9xxx` | tool / config / model                                                                | `PUR900` model-merge-conflict; `PUR901` model-required-missing                                                                                                                |

`MODEL_INCOMPLETE`, `RelationRowTypeUnknown`, and the `IND_*` set are **ReasonCode ids** (carried in `reason`), orthogonal to the `PUR` DiagCode.

### 6.3 Unified exit codes (single authoritative 5-value scheme)

```text
0  success / nothing actionable
     validate|lint: no Error-severity diagnostics (warnings ok unless --deny/--strict escalates)
     eq: EQUIVALENT ;  fmt --check: no file would change ;  fmt write: OK
1  actionable finding (NOT a tool failure)
     validate|lint: >=1 Error ;  eq: NOT_EQUIVALENT ;  fmt --check: >=1 file would change
2  INDECISIVE (eq/diff only): verdict INDECISIVE with reason code
3  usage / config error (bad flags, model required but absent, unreadable/malformed model)
4  internal error (parser panic caught, I/O failure, bug) — rare; triggers a bug report
```

The two contradictory schemes from the component drafts (2=model-load-fail vs 2=INDECISIVE) are resolved: **model-load failure is 3**, **INDECISIVE is 2**. Severity escalation (`--deny/--warn`, config) is applied **before** exit-code computation. `--quiet` never changes exit codes. Determinism: `--jobs 1` ⇒ byte-reproducible ordering and exit code.

### 6.4 CLI surface (clap derive)

```text
pure-analyzer <SUBCOMMAND> [FILES...] [OPTIONS]

Global:
  --format <human|json|sarif>   default: human on TTY, json otherwise
  --color <auto|always|never>   --quiet/-q   --verbose/-v
  --config <PATH>  |  --no-config
  --jobs <N>                    default num_cpus; N=1 => deterministic order
  --stdin-filename <PATH>

Model:
  --model <PATH>                Pure model file (*.pure) OR PMCD JSON (*.json); auto-detected; repeatable
  --model-format <pure|pmcd>    override auto-detect
  --require-model               error (exit 3) if a subcommand needs a model and none given

FILES: paths/globs; dirs walked (.gitignore honored unless --no-ignore); "-" = one stdin source.

pure-analyzer validate [FILES...]                     # no model needed; --strict escalates shape warnings
pure-analyzer lint     [FILES...] --model <M>
    --select <CODES>  --ignore <CODES>  --deny <CODES>  --warn <CODES>   # glob over PUR codes
    --fix                                       # applies MachineApplicable only
pure-analyzer eq   <LEFT.pure> <RIGHT.pure> --model <M>  [--column-order significant|insignificant]
    [--multiplicity bag|set] [--witness-budget rows,secs] [--explain] [--features smt]  # smt v2+
pure-analyzer diff <LEFT.pure> <RIGHT.pure> --model <M> [--explain]
pure-analyzer fmt  [FILES...] [--check | --stdout | --diff | --canonical]
pure-analyzer explain <CODE>                          # prints docs/reason-codes/<code>.md
pure-analyzer --version  |  pure-analyzer --print-config
```

**Filter vocabulary (unified):** `--select/--ignore/--deny/--warn` all take **DiagCode globs** over the `PUR` namespace (e.g. `PUR2*`, `PUR2001`). Category-prefix filters (M/P/C/V) are dropped.

**Two-file subcommand boundary:** `eq`/`diff` require **exactly two** resolved inputs. A glob/dir expanding to ≠2, or an ambiguous stdin pairing, is `PUR9xx` usage → exit 3.

### 6.5 Config (`.pure-analyzer.toml`)

Discovery (nearest-wins): `--config <path>` › `.pure-analyzer.toml` walked up from each input's dir to workspace root › none. `--no-config` disables discovery. CLI flags override config; config overrides defaults. `--print-config` shows the resolved merge. Determinism requires `BTreeMap`/sorted-before-emit throughout the model graph and render path (no `HashMap` iteration in output).

```toml
[lint]
select = ["PUR2*"]
ignore = ["PUR2003"]
deny   = ["PUR2001"]
[validate]
strict = false
[fmt]
line-width = 100
[model]
paths = ["model/domain.pure"]     # engine-free default model
```

---

## 7. Model Input (staying engine-free)

### 7.1 The decisive framing: Pure-model-file is first-class; PMCD is optional

The single fact milestoning arity needs is the **target class's temporal stereotype**, which is present in **Pure model source** (`<<temporal.X>>`), and generated milestoned qualified properties are **synthesizable** from it. Therefore **full-strength arity linting is achievable WITHOUT running the Legend engine.** pure-analyzer demotes PMCD from "required/authoritative" to an **optional coverage booster** (associations, richer qualified properties, and later store columns). If PMCD were a prerequisite, pure-analyzer's raison d'être — a fast *standalone* alternative to Java/IDE tooling — would collapse, since producing PMCD requires running the exact engine pure-analyzer replaces.

`--model` accepts either; auto-detect by extension + content sniff (`.json` with M3 element shapes → PMCD; `.pure` → Domain-grammar parse). Both normalize into one `ModelGraph`. Multiple `--model` merge (last-wins + `PUR900`).

### 7.2 Domain-grammar dependency is real and budgeted in v0.1

The Pure-model-file path requires pure-analyzer-parser to parse the **Domain** grammar layer (`ClassDef`/`AssociationDef`/`ProfileDef`/stereotype/multiplicity/`extends`), a second parser surface beyond M3 queries. This is **explicitly a v0.1 deliverable** (budgeted), not hand-waved — otherwise lint would silently require PMCD, contradicting the standalone promise. The Domain subset needed for arity is small (class stereotype, supertypes, property targets+multiplicities, association ends+stereotype); exotic Domain constructs (measures, functions, enums) parse to opaque/ignored nodes.

### 7.3 Open-world honesty for Pure files

Per-class provenance is recorded. Where a Pure file omits a stereotype or association pure-analyzer cannot confirm, the class is `coverage_gap = true` and lint runs open-world for it (unknown → `MODEL_INCOMPLETE` Recoverable, never a false Error). Users are steered to add PMCD for association-heavy models. This preserves correct-first while keeping the common case (own-class temporal stereotype + synthesized milestoned ends) fully engine-free.

### 7.4 Store-column resolution deferred

The relational **store/Database** model (table → column types), needed for `#>{db.Table}#` row-type/column linting, is **deferred out of v1**. In v1 the store island yields only `(dbPath, table)` lexically; relation-column checks are off (recoverable). This avoids pulling a second (Relational) grammar and a Database metadata source into the v0.1 critical path — none of which milestoning arity needs.

---

## 8. Test Strategy

Four tiers, all gating CI.

1. **Golden / snapshot (`insta`).** Per subcommand: `input.pure` (+ optional `model.{pure,json}`) → expected serialized `Diagnostic` JSON. Covers stable output shape across formats. `cargo insta review` to update.

2. **Engine-parity differential corpus (validate's ONLY correctness oracle — MANDATORY, v0.1-blocking).** `tests/corpus/{accept,reject}/*.pure`, each reject tagged with its expected `PUR` code. An `xtask corpus-verify` round-trips snippets through the real `legend-engine-language-pure-grammar` jar to (re)generate the accept/reject oracle, **version-tagged to an engine commit**. CI fails if validate accepts an engine-reject or rejects an engine-accept. Seed up front with every over-admission case: paren-tuple, `$x[$i]`/`$x[a+1]`/`$x[]`, malformed milestone parens, `JoinKind.FULL_OUTER`, unbalanced island — **and** the *legal* forms the corrected roster must NOT reject: bare `~col`, `.prop($businessDate)`, `.prop()` (0-arg call). This is a **standing maintenance commitment**, not a one-shot grammar; the jar is a **dev/CI** dependency only (runtime purity intact).

3. **Milestoning-arity fixtures.** Worked chains from the engine's `core_functions_unclassified/milestoning/*.pure` + synthetic models exercising: bi/uni/none targets; the **full truth table** of §2.4; the **same property name on different source classes** (different arity); the **context-gated 0-arg** case (source milestoned compatibly → 0 legal → must NOT flag); the **non-milestoned-intermediate** case (`BiClass.all()->…$x.plainProp.biProp` requires explicit `%latest` — must flag); **inherited** properties and inherited stereotypes (no false `PUR2002`); **user QP returning a milestoned class** (must NOT `%latest`-flag); **in-lambda** navigation (`->filter(x|$x.…)`); **milestoned associations**. Run **both** with a Pure-file model and a PMCD model to assert parity up to `PUR2100` derived-qualprop Info flags.

4. **Fuzz (`cargo-fuzz`).**
   - **Parser:** never panics, never infinite-loops on arbitrary bytes (guards the recovery-loop pitfall — every recovery step consumes ≥1 token or decrements bounded fuel); every node carries a valid span.
   - **fmt idempotence:** `fmt(fmt(x)) == fmt(x)`; `fmt --canonical` emitter injectivity.
   - **eq soundness (the critical harness).** (a) **Metamorphic:** `eq(q,q)=EQUIVALENT`; `eq(q, cosmetic(q))=EQUIVALENT` — **but the mutation generator is restricted to provably order-preserving, column-order-preserving, null-safe rewrites**; a mutation that reorders upstream of an unordered `limit`, reorders `select` columns, or touches null-sensitive predicates must be classified as *possibly-inequivalent* and is therefore **not** asserted EQUIVALENT (else the oracle would certify the very bugs the critiques flagged). (b) **Differential:** a large generated pool of pairs run through BOTH pure-analyzer eq AND the real Legend/H2 engine; assert **no pure-analyzer `EQUIVALENT` contradicts engine "not equal"** and **no pure-analyzer `NOT_EQUIVALENT` contradicts engine "equal"**; every emitted witness is re-verified on the engine (a wrong witness = P0). (c) **INDECISIVE corpus:** window/pareto/fiscal/ratio/as-of pairs must return INDECISIVE with the correct FUNDAMENTAL code (assert the code, not just the verdict). (d) **Confluence/termination:** random rule-order → identical NF; the guard never trips on the corpus.
   - **Interpreter fidelity (M4b gate):** every whitelisted scalar/relational function's null/3VL/aggregation/join/sort semantics is pinned by an engine differential test **before** it may drive a refutation.

**Determinism golden test:** identical inputs run twice → byte-identical output (no `HashMap` in the render/model path).

**Feature-gating CI:** a `--no-default-features` build asserts the sound core (validate+lint+eq-M4a) compiles with **no solver dependency**; a dependency-DAG check asserts no sound-core crate depends on `pure-analyzer-eq-smt`.

---

## 9. Staged Milestones

- **v0.1 — validate + lint + default fmt (the shippable MVP, small/complete/high-value).**
  lexer + syntax + resilient parser + rowan CST + spans (M3 query grammar **and** the Domain-model subset); `validate` with the corrected over-admission roster + the **mandatory** engine-parity corpus; `pure-analyzer-model` (**Pure-file first-class** + PMCD booster) with supertypes/associations/QP classification; `pure-analyzer-resolve` with the local lambda-param/let TypeEnv + generalization walk + the corrected fresh-per-hop context gate + generated-vs-user-QP gate; `lint` (arity core, unknown-property, cardinality; **all fixes Suggested-only**); **default-layout `fmt`** (CST re-emit, idempotent) — a freebie that exercises the re-emitter early; human + json output (sarif optional), unified exit codes, config, `explain`.

- **v0.2 — eq + diff + `fmt --canonical`.**
  `pure-analyzer-ir` (RA-IR + confluent/terminating normalizer, column-order-significant, guarded rewrites); **M4a first:** structural NF match + schema/structural refutation (3a, 3b) — no bag interpreter; **M4b next (only after the interpreter semantics corpus passes):** the model-legal, 3VL-pinned bounded witness search (3c). `diff` verdict + span-anchored delta. `fmt --canonical` from the NF (emitter-injectivity fuzzed). Reason-code taxonomy (FUNDAMENTAL/RECOVERABLE) wired through diagnostics + `docs/reason-codes/`; exit code 2 for INDECISIVE. eq lives in its own `pure-analyzer-eq`/`pure-analyzer-ir` crates.

- **v0.2 (co-shipped) — LSP surface (first-class).**
  `pure-analyzer-lsp` (tower-lsp): diagnostics-on-change, `Fix`→code-actions, `explain`→hover, go-to-definition on navigation via the resolver. Ships alongside eq because every input it needs — `Diagnostic`, structured `Fix`, `explain`, the resolved model — already exists from v0.1; it is an adapter, not new analysis.

- **v0.3 — LSP performance hardening (optional).**
  Add `salsa` incremental recomputation to `pure-analyzer-lsp` **only** if profiling shows single-file re-parse cost matters on large (multi-hundred-file) workspaces; the v0.2 server is already correct without it.

- **v2+ — SMT symbolic eq arm (feature-gated, strictly additive).**
  `pure-analyzer-eq-smt`: `easy-smt` prototype → `z3` for throughput; ships behind `--features smt`; UNKNOWN/timeout maps to INDECISIVE, never a commit; default builds stay solver-free.

---

## 10. Honest Limits & Explicitly Out of Scope

- **The research-grade milestoning-equivalence THEORY is OUT** (of this project, not just v1). The SMT/semiring decision procedure with mechanized bitemporal as-of laws for window/pareto/multi-step-fiscal/division/ratio equivalence is a **separate research project**. pure-analyzer `eq` is honestly `INDECISIVE(FUNDAMENTAL)` on all of it (`IND_WINDOW`, `IND_PARETO`, `IND_MULTISTEP_FISCAL`, `IND_DIVISION_RATIO`, `IND_MILESTONING_ASOF`). No solver in the sound core.

- **validate is convergent, not provably exact.** A hand-written parser cannot be proven acceptance-equivalent to ANTLR4 ALL(*) by a finite corpus. The guarantee is "rejects the enumerated over-admission classes and continuously converges via the versioned differential corpus." Residual divergence is a tracked liability; the corpus is a standing maintenance commitment tied to an engine commit.

- **lint's real-world reach is bounded by local type inference.** Navigation rooted at `Class.all()` inside lambdas is covered; `$var` chains whose type cannot be locally inferred (complex higher-order threading, un-tracked lets) degrade to `PUR2101 unknown-source` (silent, recoverable), not false positives. Full type inference is out of v1.

- **Pure-file models are weaker than PMCD for associations.** Own-class temporal stereotypes and synthesized milestoned ends are fully engine-free; association-heavy or unique-key-dependent analysis benefits from PMCD. Missing/unconfirmed model facts degrade to `MODEL_INCOMPLETE` (Recoverable), never a hard Error.

- **Store-column linting is deferred** (§7.4): relation column refs are unchecked in v1 unless a store model is loaded in a later milestone.

- **eq is INCOMPLETE by design.** Budget exhaustion, opaque functions, unmodeled ops, and unresolved schema all yield RECOVERABLE INDECISIVE codes — a triage-able backlog, never a wrong commit. Soundness (I1/I2/I3) is sacred and asymmetric: one wrong `EQUIVALENT` or `NOT_EQUIVALENT` destroys the tool's value, so **INDECISIVE is always preferred over an uncertain commitment.** Dropped from v1 for soundness: distinct-elimination-via-unique-key (no key provenance in the class model), unguarded dead-column elimination and predicate simplification (guarded by provable totality/non-null instead), and any column-set reordering (column order is significant).

- **fmt never changes meaning.** Default layout mode is whitespace/comment/spacing canonicalization only — it must not inherit any structural reordering from the eq normalizer. `--canonical` is a separate, opt-in, semantics-preserving rewrite that may drop/re-attach comments.
