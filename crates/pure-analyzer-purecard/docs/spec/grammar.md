# PureCARD Spec — L1 Grammar

_[Spec index](README.md) · [domain model](../domain-model.md)_

## 5. L1 — the emitted-Pure grammar (syntactic constraint level)

L1 is the context-free grammar of the _emitted subset_ of Legend Pure that the trained model actually produces. The corpus exercises **two idioms** the grammar must both admit: an **arm-A relational envelope** (`|Db->tableReference(...)->tableToTDS()->…`, the TDS/table-function pipeline, 92.2% of gold) and an **arm-C class-navigation** form (`|Class.all()->…`, class-anchored relation pipelines, 7.8%). Both are single Pure lambdas; they diverge only at the `source` production and in a handful of relational leaf steps. L1 makes the output _parse_; L2 (§6) makes the identifiers/types _resolve against a model_; L3 (faithfulness) is out of scope for both. The both-arms scope is recorded in ADR-0004. A third construct family — **arm-R**, the modern Relation/Function API (`~`-column constructs) the fine-tuned model also emits — is additive, oracle'd by a separate seed corpus, and specified in §5.9 (ADR-0007, ADR-0008).

**Target Legend version.** L1 targets the Legend Pure grammar of **engine 4.113.0** (SDLC 0.195.0) — the pinned compile-oracle stack every gold query was execution-verified against (`corpus/legend-stack/`, `docs/spec/testing.md`). The differential gate (§5.10) labels its corpus against a running engine and asserts that pin (`scripts/label-differential.mjs`), so a grammar comparison never silently runs against a different Legend version. Moving off 4.113.0 is a deliberate, corpus-re-validating change, not a routine bump.

**Core principle (oracle-driven).** Every production below is derived from, and testable against, the execution-verified gold Pure queries the upstream pipeline already produced (see §8 for corpus locations). The verified corpus **is** the spec: a grammar that masks a token appearing in a gold query is a soundness bug. Do **not** invent productions the corpus does not exercise, and do **not** omit ones it does. The construct inventory in §5.7 is the empirical evidence — counts over the full **5,034-query** corpus (`corpus/gold_queries.jsonl`: 4,639 arm-A + 395 arm-C), one per query containing the construct.

### 5.1 Query envelope (two observed top-level forms)

The final-query span PureCARD constrains is a Pure lambda. Two envelopes occur in the corpus:

```ebnf
query        = simpleQuery | blockQuery ;
simpleQuery  = "|" pipeline ;                          (* the common case: 98.6% of gold *)
blockQuery   = "{|" { letBinding ";" } pipeline "}" ;  (* let-scoped block: 69 gold (1.4%) *)
letBinding   = "let" ident "=" pipeline ;              (* a named sub-pipeline, referenced as $ident *)
```

`blockQuery` binds one or more sub-pipelines with `let` and returns a final `pipeline`; a `let`-bound name is referenced later as a `$ident` row/scalar value (e.g. the `->at(0).getString('mps')` scalar-extraction pattern, §5.7). L2's scope machine (§6.4) enters each `pipeline` independently.

### 5.2 Pipeline and steps

The two idioms branch here — at `source` and in the relational leaf steps — then
re-converge on the shared lambda/expression productions of §5.3.

```ebnf
pipeline   = source , { "->" step } ;
source     = classNavSource | relationalSource ;
classNavSource   = classpath , ".all()" ;              (* arm-C: N3 position, classpath must be a real class; 395 gold *)
relationalSource = classpath "->" "tableReference" "(" strlit "," strlit ")"
                             "->" "tableToTDS" "(" ")" ;    (* arm-A: Db->table function envelope; 4,639 gold *)
step       = (* shared + arm-C *)
             filter | project | groupBy | olapGroupBy | restrict
           | sort | take | distinct
             (* arm-A relational steps *)
           | relGroupBy | relAgg | renameColumns | extend | join | limit ;

(* --- arm-C class-navigation steps (unchanged) --- *)
filter     = "filter" "(" ( lambda | tdsLambda ) ")" ;  (* bare binder (arm-C) or typed binder (arm-A, §5.3) *)
project    = "project" "(" "[" colLambda { "," colLambda } "]"
                       "," "[" strlit    { "," strlit    } "]" ")" ;
groupBy    = "groupBy" "(" "[" { keyLambda { "," keyLambda } } "]"   (* key list MAY be empty: [] *)
                       "," "[" agg       { "," agg       } "]"
                       "," "[" strlit    { "," strlit    } "]" ")" ;
olapGroupBy= "olapGroupBy" "(" "[" strlit { "," strlit } "]"          (* partition columns *)
                       "," sortSpec                                    (* window order, e.g. desc('MaxRevenue') *)
                       "," reduceLambda                                (* e.g. y|$y->rowNumber() *)
                       "," strlit ")" ;                                (* output column name *)
sort       = "sort" "(" ( strlit "," sortdir | sortSpec { "," sortSpec } ) ")" ;  (* chainable multi-key *)
sortSpec   = ( "asc" | "desc" ) "(" strlit ")" ;       (* olap/sortBy helper form *)
sortdir    = "SortDirection.ASC" | "SortDirection.DESC" ;
take       = "take" "(" int ")" ;
distinct   = "distinct" "(" ")" ;

(* --- arm-A relational (TDS) steps, corpus-derived --- *)
restrict   = "restrict" "(" strOrList ")" ;            (* string-or-list; restrict('Rank') AND restrict(['a','b']) *)
relGroupBy = "groupBy" "(" strOrList "," relAgg { "," relAgg } ")" ;  (* key col(s) then agg(s); key MAY be [] *)
relAgg     = "agg" "(" strlit "," tdsMapLambda "," tdsReduceLambda ")" ;  (* 3-arg: 'COUNT()', map, reduce *)
renameColumns = "renameColumns" "(" renameArg ")" ;
renameArg  = colRename | "[" colRename { "," colRename } "]" ;  (* string-or-list *)
colRename  = strlit "->" "pair" "(" strlit ")" ;      (* 'FacID'->pair('FacID_T1') *)
extend     = "extend" "(" extendArg ")" ;
extendArg  = colDef | "[" colDef { "," colDef } "]" ;          (* string-or-list *)
colDef     = "col" "(" tdsColLambda "," strlit ")" ; (* col( row: …[1]|$row.getString('c'), '_c0' ) *)
join       = "join" "(" relationalSubPipeline "," joinType "," braceLambda ")" ;
relationalSubPipeline = relationalSource , { "->" step } ;    (* a full Db->tableReference…tableToTDS pipeline *)
joinType   = classpath "." ( "INNER" | "LEFT_OUTER" ) ;  (* meta::relational::metamodel::join::JoinType.INNER *)
limit      = "limit" "(" int ")" ;
```

`groupBy`/`relGroupBy` with an empty key list `[]` is the aggregate-over-all form
(verified: the `count(*)` gold). The arm-C `groupBy`/`restrict` take bracketed
lists; the arm-A `relGroupBy`/`restrict` accept a bare `strlit` _or_ a list
(`strOrList`, §5.4) — the single-column shorthand the relational emitter uses
(`restrict('Rank')`, `groupBy('FacID_T1', …)`). `limit`/`extend` **are** observed
in arm-A (665 / 446 gold) — they were absent only from the arm-C slice; `take`
remains the arm-C row-limiter. `join` embeds a full relational sub-pipeline as its
first argument (`tableReference` occurs 8,455× across 4,639 queries — more than
once per query — precisely because joins nest source pipelines), so the PDA must
recurse `source`/`step` under a sub-pipeline frame.

### 5.3 Lambdas and expressions (the L2 narrowing surface)

```ebnf
lambda       = binderVar "|" boolExpr ;                (* filter predicate *)
colLambda    = binderVar "|" valueExpr ;               (* project column *)
keyLambda    = binderVar "|" valueExpr ;               (* groupBy key *)
mapLambda    = binderVar "|" valueExpr ;               (* agg map *)
reduceLambda = binderVar "|" reduceExpr ;              (* agg reduce, e.g. y|$y->sum() / ->count() *)
agg          = "agg" "(" mapLambda "," reduceLambda ")" ;   (* arm-C 2-arg agg *)

(* --- arm-A typed-multiplicity binders (relational lambdas) --- *)
typedBinder  = ident ":" classpath "[" mult "]" ;      (* row: meta::pure::tds::TDSRow[1] *)
mult         = "1" | "*" | int ;                       (* corpus exercises 1 and * only; int reserved (§5.6) *)
tdsLambda    = typedBinder "|" boolExpr ;              (* filter row predicate *)
tdsColLambda = typedBinder "|" valueExpr ;             (* extend/col value *)
tdsMapLambda = typedBinder "|" valueExpr ;             (* relAgg map,    row: …[1]|$row *)
tdsReduceLambda = typedBinder "|" reduceExpr ;         (* relAgg reduce, y: …[*]|$y->count() *)
braceLambda  = "{" typedBinder { "," typedBinder } "|" boolExpr "}" ;  (* join key predicate over ≥2 binders *)

boolExpr   = cmp { ("&&" | "||") cmp }
           | "(" boolExpr ")" { ("&&" | "||") cmp } ;
cmp        = valueExpr cmpop valueExpr                 (* T1/T2/T6 operand-type + multiplicity position *)
           | valueExpr "->" boolPred                   (* T4/T6 predicate position *)
           | navExpr "->" "in" "(" pipeline "." ident ")" ;  (* subquery membership *)
cmpop      = "==" | "!=" | ">" | "<" | ">=" | "<=" ;

reduceExpr = refVar "->" reducer "(" ")" ;             (* T3 reducer-type position; body use is $-prefixed *)
reducer    = "count" | "sum" | "average" | "min" | "max" | "size" | "rowNumber" ;

boolPred   = ( "exists" | "contains" | "startsWith" | "endsWith"
             | "isEmpty" | "isNotEmpty" ) "(" [ predArg ] ")"
           | "between" "(" valueExpr "," valueExpr ")" ;  (* arm-A range predicate; 35 gold *)
predArg    = lambda | valueExpr ;                      (* exists takes a lambda; contains/startsWith take a value *)

(* valueExpr is any scalar-valued expression usable as an operand, a projected column, or a key. *)
valueExpr  = term { arithop term } ;
arithop    = "+" | "-" | "*" | "/" ;
term       = navExpr { "->" collapse } { "->" fn ( "(" [ fnArgs ] ")" ) }
           | ifExpr | literal | colAccess | "(" valueExpr ")" ;
collapse   = "toOne" "(" ")" ;                         (* [0..1]/[*] -> [1]; the T6 collapse operator *)
fn         = "parseFloat" | "parseInteger" | "toString" | "toLower" | "toUpper"
           | "substring" | "year" | "at" | "cast" | "first" | "concatenate" ;
fnArgs     = valueExpr { "," valueExpr } ;
ifExpr     = "if" "(" boolExpr "," "|" valueExpr "," "|" valueExpr ")" ;  (* zero-arg then/else lambdas *)

navExpr    = refVar { "." ident } ;                    (* body use is $-prefixed; N1 (first ident) + N2 (chained idents) *)
colAccess  = refVar "." tdsGetter "(" strlit ")" ;     (* N6 relation-column access, e.g. $r.getInteger('cnt') *)
tdsGetter  = "getInteger" | "getFloat" | "getString" | "getBoolean" ;
```

**Note on `navExpr` = the whole L2 narrowing spine.** `navExpr = refVar { "." ident }` is intentionally one production covering _all_ of: a plain property (`$x.name`), an association navigation (`$x.fk0DefaultContinents`), a qualified/derived property, and a chained navigation (`$x.fk0DefaultContinents.contId`). L1 cannot and must not distinguish them — that is exactly what L2's N1/N2/N5 narrow. The grammar's only job is to fix that a `.` after a `var` (or after a prior `ident`) is followed by an `ident`; L2 decides _which_ ident.

### 5.4 Terminals and identifiers (lexis)

```ebnf
classpath  = ident { "::" ident } ;                    (* e.g. spider::car_1::model::default::Countries *)
binderVar  = ident ;                                   (* lambda HEADER only: the bare "x" in  x|...       *)
refVar     = "$" ident ;                               (* expression BODY only: the "$x" in  $x.name       *)
literal    = strlit | number | boollit | dateLit | milestoneLit ;
strlit     = "'" { schar | "''" } "'" ;                (* SINGLE quotes only; embedded quote doubled ''   *)
number     = [ "-" ] ( digit { digit } [ frac ] | frac ) ;   (* int, "1.5", leading-dot ".5", "-.5"     *)
frac       = "." digit { digit } [ exp ] ;             (* exponent only AFTER a fractional part (§5.5)     *)
exp        = ( "e" | "E" ) [ "+" | "-" ] digit { digit } ;   (* scientific: "1.5e3", "1.5e-3"; NOT "1e3"  *)
boollit    = "true" | "false" ;
dateLit    = "%" dateChar { dateChar | "." } ;         (* numeric date/time: %2018-03-17[T07:13:53[.000]]  *)
dateChar   = digit | "-" | "T" | ":" ;
milestoneLit = "%" lower { lower } ;                   (* symbolic milestoning: %latest / %latestdate      *)
lower      = "a".."z" ;
int        = digit { digit } ;
strOrList  = strlit | "[" [ strlit { "," strlit } ] "]" ;  (* single string OR bracketed list (MAY be []); arm-A restrict/groupBy keys *)
ident      = alpha { alnum | "_" } ;                   (* camelCase props, PascalCase classes, snake cols *)
schar      = <any character except a single quote> ;
alpha      = "a".."z" | "A".."Z" ;
alnum      = alpha | digit ;
digit      = "0".."9" ;
```

### 5.5 Verified lexical quirks (corpus-confirmed)

- **Single-quote strings only.** Double quotes never appear; an embedded quote is written `''` (15 gold queries exercise the doubling). A grammar admitting `"..."` is a compile-unsound over-approximation — keep `strlit` single-quote-only.
- **`SortDirection.ASC` / `SortDirection.DESC`** are the only enum-shaped literals in the pilot corpus (36 occurrences), and they occur **only inside `sort`** (via `sortdir`), never as a comparison operand. They are a _Pure builtin_, not a schema enumeration, so they are **not** an L2 N4/N5 position — L1 fixes their `EnumPath "." IDENT` shape as a fixed terminal in `sortdir`, and L2 does not narrow them. **Schema-enum comparison** (`$x.status == SomeEnum.ACTIVE`, an `EnumRef` property vs an enum value) is what L2's N4/T5 target; it does **not** occur in the current Spider-derived corpus, so per §5 the emitted grammar carries **no** enum-literal _operand_ production yet. That operand production (`enumLit = classpath "." ident`, feeding `term`) is **reserved**: add it on the first gold query that compares an enum, at which point L2's N4/T5 narrow its RHS. See §7 (the N4/T5 contract rows) and §6.5 N4 / §6.6 T5, which mark the same rules forward-looking.
- **`binderVar` vs `refVar`.** The lambda _header_ names the variable bare (`x|`); every _use_ in the body is `$`-prefixed (`$x.`). L1 keeps them distinct so a stray bare `x.name` or `$x|` is rejected; L2 binds the header name and resolves `$`-uses against it (§6.4, transition S2).
- **Two kinds of `%`-literal.** A `%` opens either a _numeric_ date/time literal (`dateLit`, `%2018-03-17[T07:13:53]`) or a _symbolic_ milestoning literal (`milestoneLit`, `%latest` / `%latestdate`). They are disjoint at the first byte after `%`: a `dateChar` (digit / `-` / `T` / `:`) opens `dateLit`, a lowercase letter opens `milestoneLit`, and a bare `%` (or any other byte) is a dead state. `%latest` is not in the Spider-derived gold corpus; it is oracle'd by the **modern-dialect seed corpus** (§5.8) — the fine-tuned model emits it in `Class.all(%latest)`, bitemporal `Class.all(%latest, %latest)`, milestoned `.PROP(%latest, %latest)`, and comparison-operand positions (gap report §5/G2). Like `dateLit`, `milestoneLit` is a `Lexeme::Date` L2 pass-through — no schema narrowing.

### 5.6 Deliberate over-approximations (oracle-driven tightening)

The grammar over-approximates validity where a CFG cannot cheaply enforce a constraint the compiler oracle already catches. Do **not** tighten these speculatively; tighten only where §8 differential compile testing finds a real invalid escape:

- **Projected-column-count == name-count.** `project`/`groupBy`/`olapGroupBy` do not enforce that the lambda-list length equals the name-list length. The compiler catches a mismatch.
- **Arithmetic/`if` type coherence.** `valueExpr` allows any `arithop` between any two `term`s; L2's type rules (T1–T2) and the compiler reject numeric/string mixing.
- **Collapse necessity.** L1 allows `navExpr` scalar comparisons without a `->toOne()`; whether a `[0..1]`/`[*]` navigation _must_ be collapsed first is L2's T6, not L1's.
- **Predicate arity.** `boolPred` arguments are loosely typed (`predArg`); the exact arg shape per predicate (lambda vs value) is left to L2/compiler.
- **Typed-binder multiplicity.** `mult` admits `int` as well as `1`/`*`; the corpus exercises only `1` and `*` (`TDSRow[1]`, `TDSRow[*]`). The `int` alternative is a deliberate, sound widening (it admits more, never less); an integer multiplicity a model emits is caught by the compiler, not L1.
- **`restrict`/`groupBy` string-or-list.** The arm-A relational steps accept a bare `strlit` _or_ a bracketed list (`strOrList`); L1 does not require the list form even where a single column would suffice.
- **Symbolic milestoning literal shape.** `milestoneLit = "%" lower { lower }` admits any `%`-prefixed lowercase run, not only the two known symbols `%latest` / `%latestdate`. This mirrors how the machine already admits _any_ identifier where a reducer/step/property name is expected: L1 fixes the `% <lowercase>+` shape and the compiler/L2 reject an unknown milestone symbol. Uppercase and digit boundaries stay dead (`tests/precision_reject.rs`), so the widening cannot silently grow to `%<anything>`.

### 5.7 Observed construct inventory (the empirical spec)

Counts in the **Queries** column are **distinct queries containing the construct
at least once** — _not_ raw occurrence totals — over the full **5,034-query**
corpus (`corpus/gold_queries.jsonl`: 4,639 arm-A + 395 arm-C), recomputed this
session. This is deliberately a different measure from the _total occurrences_
quoted in prose (§5.2): a construct that repeats
within one query (`pair` appears 32,308 times but in 2,378 queries; `tableReference`
8,455 times in 4,639 queries) has a higher occurrence total than its
queries-containing count, while a once-per-query construct (`limit`, `between`)
has equal counts. The queries-containing figures here are the authoritative
inventory the grammar is locked against. Every construct here MUST parse; anything
absent here is _not yet_ in the grammar (add on first gold occurrence, per §5's
core principle).

**Arm-A relational envelope and steps** (the 92.2% majority idiom):

| Construct | Queries | Grammar production |
|---|---:|---|
| `tableReference(...)` / `tableToTDS()` | 4639 / 4639 | `relationalSource` (arm-A envelope) |
| `meta::pure::tds::TDSRow[…]` typed binder | 4057 | `typedBinder` / `mult` |
| `restrict(...)` | 3540 | `restrict` (string-or-list) |
| `filter(row: …[1]\|…)` | 3105 | `filter` / `tdsLambda` |
| `renameColumns(...)` / `->pair(...)` | 2378 / 2378 | `renameColumns` / `colRename` |
| `join(...)` | 2378 | `join` / `relationalSubPipeline` |
| `JoinType.INNER` / `JoinType.LEFT_OUTER` | 2196 / 272 | `joinType` |
| `groupBy(strOrList, agg…)` / `agg('N',…)` | 2335 / 2335 | `relGroupBy` / `relAgg` (3-arg) |
| `getInteger`/`getString`/`getFloat`/`getBoolean` | 2622 / 2391 / 543 / 4 | `colAccess` / `tdsGetter` |
| `limit(int)` | 665 | `limit` |
| `extend(...)` / `col(...)` | 446 / 725 | `extend` / `colDef` |
| `between(...)` | 35 | `boolPred` (range predicate) |

**Shared expression / lambda constructs** (both arms):

| Construct | Queries | Grammar production |
|---|---:|---|
| `->count()` | 1691 | `reducer` |
| `distinct()` | 1185 | `distinct` |
| `sort(...)` | 1048 | `sort` / `sortdir` / `sortSpec` |
| `&&` / `\|\|` boolean connectives | 945 / 560 | `boolExpr` |
| `desc(...)` / `asc(...)` sort helpers | 665 / 352 | `sortSpec` |
| `isEmpty()` / `isNotEmpty()` | 441 / 60 | `boolPred` |
| `->average()` / `->max()` / `->sum()` / `->min()` | 292 / 238 / 180 / 140 | `reducer` |
| `->contains(...)` | 69 | `boolPred` (String, T4) |
| `->toOne()` | 41 | `collapse` (T6 collapse operator) |
| `concatenate` / `between`-arg literals | 35 | `fn` |
| `if(...)` | 25 | `ifExpr` |
| `->in(subquery.col)` | 18 | `cmp` subquery-membership form |
| `parseFloat` / `startsWith` | 15 / 15 | `fn` / `boolPred` |
| `->size()` / `->exists(...)` | 10 / 14 | `reducer` / `boolPred` |
| `toLower` / `->map(...)` / `->year()` | 6 / 6 / 5 | `fn` |
| `==` `!=` `>` `<` `>=` `<=` | (all present) | `cmpop` |

**Arm-C class-navigation constructs** (the 7.8% minority idiom):

| Construct                            | Queries | Grammar production              |
| ------------------------------------ | ------: | ------------------------------- |
| `.all()`                             | 395     | `classNavSource` (arm-C source) |
| `project(...)`                       | 527     | `project`                       |
| `take(int)`                          | 22      | `take`                          |
| `olapGroupBy(...)` / `->rowNumber()` | 3 / 3   | `olapGroupBy` / `reducer`       |
| `let … = …` block form               | 69      | `blockQuery` / `letBinding`     |

### 5.8 Modern-dialect seed corpus (a second oracle)

The Spider-derived `corpus/gold_queries.jsonl` (§5.7) is frozen at 5,034 queries;
it never exercised some **modern Legend Pure** constructs the fine-tuned model
also emits. Those are seeded in a _separate_, provenance-distinct file,
`corpus/modern_dialect_seeds.jsonl`, so the 5,034-query gold corpus and every doc
citation of its count stay untouched. `tests/modern_dialect_soundness.rs` replays
each seed through the real byte-PDA with the same killer property as §8.1 (never
dead, ends accepting) and classifies it to its declared envelope. The seed corpus
is the oracle for anything added here — do **not** add a production without a seed.

| Construct                             | Seeds | Grammar production       | Gap report |
| ------------------------------------- | ----: | ------------------------ | ---------- |
| `%latest` / `%latestdate` milestoning | 5     | `milestoneLit` (§5.4)    | G2         |
| `~` Relation/Function API (arm-R)     | 11    | arm-R productions (§5.9) | G1         |

### 5.9 Arm-R — the Relation/Function API (`~`-column constructs)

Modern Legend Pure's Relation/Function API
(`meta::pure::functions::relation::*`) is a third construct family, **arm-R**,
distinguished by the `~` column sigil. It is class-nav-sourced in the seed corpus
(`Class.all()->project(~[…])`), so `Envelope::classify` bins any `~`-bearing query
as `RelationApi` (the `~` is checked before the `.all(` / `tableReference`
markers). These productions are **additive** — they widen the grammar and never
touch arm-A/arm-C — and are oracle'd by the arm-R seeds in
`corpus/modern_dialect_seeds.jsonl` (§5.8), not the frozen Spider gold. Every
referenced sub-production (`colLambda`, `mapLambda`, `reduceLambda`, `binderVar`,
`valueExpr`, `strlit`, `reducer`) already exists in §5.3.

```ebnf
(* --- add to the step alternation (§5.2) --- *)
step =+ relProject | relFnGroupBy | relSort | relExtendWindow | relRename ;

relProject      = "project" "(" "~" "[" relColSpec { "," relColSpec } "]" ")" ;
relColSpec      = colName ":" colLambda ;                       (* Week: x|$x.a *)

relFnGroupBy    = "groupBy" "(" "~" "[" [ colName { "," colName } ] "]"   (* keys; MAY be ~[] *)
                            "," relAggSpec { "," relAggSpec } ")" ;
relAggSpec      = "~" colName ":" mapLambda ":" reduceLambda ;  (* ~'Gross Credits': x|$x.GC : y|$y->sum() *)

relSort         = "sort" "(" "[" relSortKey { "," relSortKey } "]" ")" ;
relSortKey      = ( "ascending" | "descending" ) "(" colRef ")" ;

relExtendWindow = "extend" "(" windowSpec "," "~" "[" winAggSpec { "," winAggSpec } "]" ")" ;
windowSpec      = "over" "(" colRef { "," colRef } ")" ;        (* over(~desk) — window partition *)
winAggSpec      = colName ":" frameLambda ":" reduceLambda ;    (* agg: {p,w,r|$r.notional} : y|$y->sum() *)
frameLambda     = "{" binderVar { "," binderVar } "|" valueExpr "}" ;  (* window frame, ≥1 binder *)

relRename       = "rename" "(" colRef "," colRef ")" ;          (* rename(~old, ~new) *)

(* --- new shared lexis --- *)
colRef          = "~" colName ;                                 (* ~Week / ~'Gross Credits' *)
colName         = ident | strlit ;                             (* bare ident OR single-quoted (spaces allowed) *)
```

**How the byte-PDA admits it (the residual over-approximation, §5.6).** The
machine adds exactly one state, `SawTilde` (reached from the value hubs on `~`),
transitioning on `[` (a relation column-set), a bare identifier, or a quoted
string (a column reference); a `:` may additionally open a `{`-brace frame lambda
(`agg:{p,w,r|…}`). Everything else in arm-R — the `:` column-to-lambda separators,
the `over(~…)` partition, the `{p,w,r|…}` frames, the reducers, the bracket
nesting — reuses the shared value-hub / lambda / bracket machinery of §5.2–§5.3.
So the grammar admits a superset of the strict productions above (e.g. it does not
enforce that a `winAggSpec` colon is bare while a `relAggSpec` colon carries a
leading `~`); the compiler oracle catches the residue, exactly as §5.6 sanctions.
Like every other L1 identifier, a `~`-column name is an **L2 pass-through** (it
opens at the `SawTilde` anchor, whose rule is `None`), so arm-R never masks the
model's emitted column names.

### 5.10 Differential gate (L1 vs. the Legend engine)

The oracle behind §5.6's "tighten only where a real invalid escape is found" is
made mechanical here. `corpus/differential_l1.jsonl` holds ~200 diverse query
strings, each labeled with the Legend engine's grammar verdict (`parse_ok` /
`parse_fail`) frozen offline by `just label-differential` — which POSTs to a
running engine and **asserts the engine version matches the 4.113.0 pin** before
labeling, so a comparison never runs against a different grammar. CI replays the
frozen corpus against L1 only (`tests/differential_l1.rs`); the pure core never
calls the engine.

The load-bearing property is **soundness**: L1 admits every query the engine
parses (`parse_ok ⟹ L1 accepts`), except a small, documented `KNOWN_DIVERGENCES`
allowlist where L1 is _deliberately stricter_ than the permissive engine grammar.
The engine's `grammarToJson` is grammar-permissive — it parses `5abc` / `1_000` as
element references (`packageableElementPtr`), deferring existence-checks — and a
constrained decoder should not admit that residue where a value belongs; those
cases are the allowlist, not a match target. This is the training-side decision to
target the _intended query dialect_: a bare / number-shaped element-reference
operand is the hallucination class constrained decoding exists to catch, so L1
rejects it as out-of-dialect residue rather than mirror the engine's permissive
over-parse. A qualified or dotted **enum-ref** operand
(`== Type.Meeting`, `== pkg::E.VALUE`) is by contrast legal Pure and stays
admitted — pinned by `enum_ref_operands_stay_admitted` so tightening the residue
cannot silently break it. A _new_ `parse_ok` query that L1 rejects and is not
allowlisted reddens the gate — the class that let the source-position `|X.'name'`
regression slip past review before this gate existed.
