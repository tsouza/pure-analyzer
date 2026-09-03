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
letBinding   = "let" ident "=" ( pipeline | scalarExpr ) ;  (* a named sub-pipeline or scalar, referenced as $ident *)
scalarExpr   = scalarCall | dateLit | milestoneLit ;   (* issue #352: a bound scalar, typically a milestoning date *)
scalarCall   = ident "(" ")" ;                         (* a bare, unqualified, zero-argument call: today(), now() *)
```

`blockQuery` binds one or more sub-pipelines or scalars with `let` and returns a
final `pipeline`; a `let`-bound name is referenced later as a `$ident` row/scalar
value (e.g. the `->at(0).getString('mps')` scalar-extraction pattern, §5.7).
`scalarExpr` was added by issue #352 for the common query shape that binds a
business date once and threads it through milestoned navigation
(`{|let d = today(); T.all($d)->…}`) — a zero-arg call and a date literal were
already admitted as a milestoning *argument* (`.all(today())`,
`.all(%2024-01-01)`); the fix is admitting the same two values as the
initializer too, not a new kind of expression. `scalarCall` is deliberately
arity-**zero**-only and unqualified: a multi-argument or qualified call
(`today(1)`, `ns::today()`), and a bare `$ident` initializer referencing an
outer binding — all real Legend Pure — are outside issue #352's evidenced
scope and stay unadmitted (`tests/precision_reject.rs`). `milestoneLit`
(`%latest`) also streams here, but only as the pre-existing residual
over-approximation §5.6 already documents at every other value position — the
pinned engine itself rejects a bare `let` value of `%latest`
("Unexpected token '%latest'"), unlike the live-attested `scalarCall`/`dateLit`
forms (`corpus/modern_dialect_seeds.jsonl`'s `issue-352/let-scalar:*` rows).
L2's scope machine (§6.4) enters each `pipeline` independently.

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
typedBinder  = ident ":" classpath [ "[" mult "]" ] "|" ; (* row: meta::pure::tds::TDSRow[1]| — the pipe is required *)
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
dateLit    = "%" digit { dateChar | "." } ;            (* numeric date/time: %2018-03-17[T07:13:53[.000]]  *)
dateChar   = digit | "-" | "T" | ":" ;
milestoneLit = "%latest" ;                             (* the engine's one symbolic milestoning symbol     *)
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
- **`SortDirection.ASC` / `SortDirection.DESC`** are the only enum-shaped literals in the pilot corpus (36 occurrences), and they occur **only inside `sort`** (via `sortdir`), never as a comparison operand. They are a _Pure builtin_, not a schema enumeration, so they are **not** an L2 N4/N5 position — L1 fixes their `EnumPath "." IDENT` shape as a fixed terminal in `sortdir`, and L2 does not narrow them. Schema-enum comparison is outside the emitted grammar and supported schema overlay.
- **`binderVar` vs `refVar`.** The lambda _header_ names the variable bare (`x|`); every _use_ in the body is `$`-prefixed (`$x.`). L1 keeps them distinct so a stray bare `x.name` or `$x|` is rejected; L2 binds the header name and resolves `$`-uses against it (§6.4, transition S2).
- **Two kinds of `%`-literal, disjoint at the byte after the sigil.** A `%` opens either a _numeric_ date/time literal (`dateLit`, `%2018-03-17[T07:13:53]`) or the _symbolic_ milestoning literal (`milestoneLit`, `%latest`). A **digit** opens `dateLit`; an **`l`** opens the `%latest` keyword; every other byte — a bare `%`, an uppercase letter, and the `-`/`T`/`:` date *separators*, which are interior bytes only — is a dead state. Both boundaries are live-attested against the pinned engine (issue #55 Phase 7): `%-`, `%T`, `%:`, `%foo`, `%late` and `%latestdate` are each "no viable alternative at input '…%'", while `%1`, `%2018-03-17T07:13:53.000` and `%latest` all parse. `%latest` is not in the Spider-derived gold corpus; it is oracle'd by the **modern-dialect seed corpus** (§5.8) — the fine-tuned model emits it in `Class.all(%latest)`, bitemporal `Class.all(%latest, %latest)` and milestoned `.PROP(%latest[, %latest])`. Like `dateLit`, `milestoneLit` is a `Lexeme::Date` L2 pass-through — no schema narrowing.

### 5.6 Deliberate over-approximations (oracle-driven tightening)

The grammar over-approximates validity where a CFG cannot cheaply enforce a constraint the compiler oracle already catches. Do **not** tighten these speculatively; tighten only where §8 differential compile testing finds a real invalid escape:

- **Projected-column-count == name-count.** `project`/`groupBy`/`olapGroupBy` do not enforce that the lambda-list length equals the name-list length. The compiler catches a mismatch.
- **Arithmetic/`if` type coherence.** `valueExpr` allows any `arithop` between any two `term`s; L2's type rules (T1–T2) and the compiler reject numeric/string mixing.
- **Collapse necessity.** L1 allows `navExpr` scalar comparisons without a `->toOne()`; whether a `[0..1]`/`[*]` navigation _must_ be collapsed first is L2's T6, not L1's.
- **Predicate arity.** `boolPred` arguments are loosely typed (`predArg`); the exact arg shape per predicate (lambda vs value) is left to L2/compiler.
- **Typed-binder multiplicity.** `mult` admits `int` as well as `1`/`*`; the corpus exercises only `1` and `*` (`TDSRow[1]`, `TDSRow[*]`). The `int` alternative is a deliberate, sound widening (it admits more, never less); an integer multiplicity a model emits is caught by the compiler, not L1.
- **`restrict`/`groupBy` string-or-list.** The arm-A relational steps accept a bare `strlit` _or_ a bracketed list (`strOrList`); L1 does not require the list form even where a single column would suffice.


**Tightened in issue #55 Phase 4 (removed from this list).** Four shapes L1 used
to over-admit are now dead states, each live-attested against the pinned engine
and each with its rejecting byte pinned in `tests/precision_reject.rs`:

- **A `,` needs an element list to separate.** It is legal inside a call/group
  (`Paren`), a collection or multiplicity bracket (`Bracket`) or a brace lambda's
  binder list (`BraceLambda`) — never at a block query's statement level, whose
  statements are `;`-separated ("Unexpected token ','").
- **A lambda binder pipe needs an argument slot.** The same three frames. At a
  block query's statement level, or on an empty stack, the query's own body is
  already open, so a second, bodiless pipe is a dead state ("Unexpected token
  '|'"). A boolean `||` stays legal everywhere.
- **A typed-binder `:` needs a binder slot.** Likewise the same three frames. The
  `::` classpath separator is decided *before* the frame test and stays legal
  wherever a classpath is, so `meta::relational::…::JoinType` in a
  block-statement-level value position is unaffected ("Unexpected token ':'").
- **A binder colon's multiplicity is optional, and `%latest`'s position is not
  fixed.** Two residuals the Phase 7 tightening deliberately stops short of.
  (1) `typedBinder` requires the pipe but not the `[mult]`, because the arm-R
  column binding legitimately has none (`~'Total': y|$y->sum()`, `~[Week: x|…]`)
  and the byte machine cannot see the `~` sigil that distinguishes it from a
  typed lambda parameter — the engine *does* require the multiplicity for the
  latter ("Unexpected token '|'. Valid alternatives: \['[', '(', '<'\]").
  (2) `milestoneLit` is admitted wherever a literal is, though the engine takes
  `%latest` only in a milestoning argument slot (`.all(…)` / `.PROP(…)`, one or
  two arguments); a comparison operand is rejected, and so — live-attested by
  issue #352 — is a bare `letBinding` value (`{|let d = %latest; …}`,
  "Unexpected token '%latest'"). Both want a position/sigil phase the current
  per-byte machine does not track.

- **A call's `(` and a multiplicity `[` bind to a name.** Both are admitted from
  `AfterName` — the state an identifier's completion (past any trailing
  whitespace) lands in — and not from the generic `AfterValue`, so a juxtaposed
  application off a call's result, a string or a number dies ("Unexpected token
  '('"), as does a bracket off anything but a type name ("Bracket operation is
  not supported" — the engine has no positional index at all). Whitespace keeps
  the position a *name* position, so `filter (x|…)` and `TDSRow [1]`, both
  engine-legal, still stream.

**Tightened in issue #55 Phase 7 (also removed from the over-approximation
list).** Three more shapes are now dead states, each live-attested and each with
its rejecting byte pinned in `tests/precision_reject.rs`:

- **The milestone literal is the `%latest` keyword**, spelled one state per byte
  (`MilestoneL`…`InMilestoneLit`) exactly as `LetL`/`LetLe`/`LetLet` spell `let`.
  `%latestdate` — which the seed corpus used to assert L1 accepts — is on the
  rejected side; see §5.8.
- **A date literal opens on a digit.** `-`/`T`/`:` are date *interior* bytes; a
  literal that starts on one (`%->…`, `%T`, `%:`) is dead. Issue #55 Phase 8
  finished the shape at the other end — see below.
- **A typed binder's right-hand side is a classpath, then its multiplicity, then
  exactly one pipe.** Only `::` (contiguous), `[`, and `|` may follow the type
  name; the multiplicity bracket holds a `mult` and nothing else (`row['europe']`
  is dead); and once it closes, only the pipe — or, inside a `join` brace
  lambda's binder list, the `,` that opens the next binder — may follow. A
  second `|` is dead in the body a binder colon opens: the binder is not an
  operand, so that `||` is never a boolean one.

**Tightened in issue #55 Phase 8 (also removed from the over-approximation
list).** Four more shapes are now dead states, each live-attested against the
pinned engine on the branch and each with its rejecting byte pinned in
`tests/precision_reject.rs`:

- **A date literal also *ends* on a digit, and its `.` is fractional seconds.**
  Every `-`/`T`/`:` owes a following digit, so a literal can neither end nor
  branch on a separator; the `.` opens the fraction and is legal only in the time
  half, past at least one `:`. The two halves also differ in *which* separators
  they take: a `T` hands over from the date half to the time half and so may open
  a field only in the first, while a `-` opens a date field in one and a timezone
  offset in the other. Live: `%2018-`, `%2018-03-17T`, `%2018-03-17T07:`,
  `%1974.`, `%1974.5`, `%0.0`, `%2018-03-17.000`,
  `%2018-03-17T07:13:53.000.111`, `%2018-03-17T07:13:53T1` and `%20:18T3` are
  each "no viable alternative at input", while `%1`, `%1974`, `%1974-1-1`,
  `%2018-03-17T07:13:53.000`, `%2018-03-17T07:13:53-0500` and `%20:18-3` all
  parse.
- **A `(` at a value position is a parenthesised *group*, not an argument list.**
  A group holds one expression, so it has no `,` to separate: `->limit((1,2))`,
  `->limit(('a','b'))`, `->limit(1+(2,3))` and `->extend(('MPG_T2',extend))` are
  each "no viable alternative at input", while `->limit((1))` and `->limit([1,2])`
  parse. It carries its own stack frame, distinct from the call `(` that binds to
  a name. A group still opens a **lambda** and a **typed-binder** slot
  (`->limit((x|1))` and `->limit((a:b[1]|1))` both parse), so only the comma
  moved.
- **A lambda binder pipe binds to a name or a string literal.** A binder is named
  by an identifier, so a pipe off any other completed term is only ever the second
  byte of a boolean `||`: `->filter(f()|1)`, `->filter(1|1)`, `->filter($x.a|1)`,
  `->filter(x.y|1)`, `->filter([1]|1)` and `->filter(%2018-01-01|1)` are each "no
  viable alternative at input '…|'", while `->filter(x|1)`, `->filter('a'|1)` and
  `->filter(a&&b|1)` parse — the last because `b` is itself a bare name in operand
  position.
- **A binder type that has taken a `::` owes its multiplicity.** The `::` settles
  the one ambiguity a bare binder type carries: it names a package path, never
  arm-R's bare column-binding variable, so the multiplicity Legend requires of a
  typed lambda parameter is no longer optional and the pipe may not follow the
  type directly (`->filter(row: meta::pure::tds::TDSRow|1)` and
  `->extend(a:b::c|1)` both die; the same walks with `[1]` in front of the pipe
  parse). The residual over-approximation — a *bare* binder type's multiplicity
  is still optional — stays, because arm-R's `~'Total': y|$y->sum()` legitimately
  has none and the byte machine cannot see the `~`.

**Tightened in issue #55 Phase 9 (also removed from the over-approximation
list).** The rule Phase 8 worked out, attested and escalated rather than merged:

- **A `::` binds to a term-start name or a string literal.** A `::` names a
  package path, and a package path is spelled from a bare word or a quoted one.
  Live-attested both ways: `…!=mpg::getInteger`, `…!=meta::pure::tds::TDSRow`,
  `…!='europe'::makeId` and `…!=mpg ::getInteger` parse, while the same `::` off
  a call's `)`, a `]`, a number, a date literal, a `$`-variable, a `.property` or
  a `->`-called name is each "no viable alternative at input '…::'". A name and a
  string literal route their own `:` to the existing `AfterColon`, which keeps
  the separator; every other completed term routes to the new `AfterValueColon`,
  which does not. The typed-binder arms are unchanged in both, because arm-R's
  second column colon legitimately follows a *completed* term
  (`~'Agg': x|$x.v : y|$y->sum()`, `~[agg:{p,w,r|$r.v}:y|…]`). Where no binder
  slot is open at all the colon has no reading left and dies on the colon itself,
  which is also where the engine points ("Unexpected token ':'").

  Shipping it needs a maintainer call, and that call is issue #55's "Decision
  1": it moves the criterion arm **+5** and the generalization guard **−8**,
  breaching the guard's floor. The −8 was proven to be a reshuffle of the walk
  sample rather than a precision loss — a second implementation accepting the
  *byte-identical* language swung the same arm by −2 — but lowering the guard's
  baseline is a §3/§7 move reserved to a human, and
  `tests/live_legend_schema_walk_compile.rs` records it as such rather than as a
  ratchet.

**One tightening Phase 8 worked out and deliberately did not ship**, recorded
here so a later phase does not re-derive it:

- **A `;`-continued block query owes its final `;`.** `{|A;B}` and `{|A;B;C}` are
  "Unexpected token", while `{|A;B;}`, `{|A;}` and `{|A}` parse. Enforcing the
  engine's actual rule — a `}` may close bare only if no `;` preceded it — needs
  *mutable per-frame* state, which neither `Step` nor the declarative spec's
  `Action` (ADR-0010, schema V1) can express. The reachable approximation,
  requiring the `;` of every block query, would make L1 deliberately stricter than
  the engine on a structural production for the first time; that belongs in
  `differential_l1.rs`'s `KNOWN_DIVERGENCES` with its own decision, not folded into
  a precision phase.

**Tightened in issue #369 (also removed from the over-approximation list).**
Every `->` in the grammar introduces a function application — `pipeline`'s own
`step`, `term`'s `collapse`/`fn`, `reduceExpr`'s `reducer`, `cmp`'s `boolPred`,
`colRename`'s `pair` — and every one of those productions spells `name "(" …
")"`. There is no production anywhere in §5 where a `->`-introduced name stands
on its own. L1 used to admit one anyway: a name reached through `AfterArrow`
closed into the same completed-name hub (`AfterMemberName`) a `.`/`$`-reached
member name closes into, and that hub's own repertoire — a further `->`, `.`,
`::`, an operator, a separator, a closer — has nothing arm-specific about it, so
a *second* `->` streamed right off the first name's bytes with no call in
between. Live-verified against the pinned engine (4.113.0): `|t::A->b->c()`,
`|t::A.all()->x->project([p|$p.a],['a'])`,
`|t::A::p->w->m->A.all()->project([p|$p.a],['a'])` and
`|t::Db->model->A(%latest)->project([p|$p.a],['a'])` are each "no viable
alternative at input '->…->'" right at the second arrow, against a `200` control
(`|t::A.all()->project([p|$p.a],['a'])`) that differs only in giving every step
its call. Empirically this was the *dominant* rejection shape under live
sampling — 58 of 81 engine-rejected, decoder-complete candidates in one run
shared exactly this fault, ahead of every other precision gap combined.

- **A `->`-introduced name is call-required.** `AfterArrow`'s own identifier now
  closes into a new state, `AfterArrowName`, distinct from `AfterMemberName`: an
  identifier reached from `.`/`$` still closes into the permissive
  `AfterMemberName` (a `.`-navigated property legitimately takes a further `->`
  with no call of its own, e.g. `$x.a->toOne()` — that half of the grammar is
  unchanged), but one reached from `->` closes into `AfterArrowName`, whose only
  live continuation — besides its own trailing whitespace — is the call's own
  `(`. Everything else, including a further `->`, is a dead state. `AfterArrowName`
  is deliberately *not* a `completes_a_term` hub either: an uncalled arrow-step
  name is not a complete query, so a stream may not end on one — before this fix,
  `Pda::is_accepting`'s own end-of-stream widening (`src/grammar/pda.rs`, the
  value-boundary probe)
  made `|X.all()->name` a false-positive completion with no engine counterpart.
  Both the per-byte mask and end-of-stream now agree with the live grammar on
  every witness above.
- **Scope, precisely.** The two receiver-category L2 rules (N3f/N3i, `docs/spec/schema.md` §6.5) had
  their own `admits_eos` mechanism clearing end-of-stream on a *denied* extent or
  scalar method name specifically because L1 gave every arrow-step name — denied
  or not — a false completion to clear. That L2 mechanism is unchanged and still
  correct (it also denies the call-opening `(` a denied name may never reach,
  independent of this fix); it is simply no longer the only line of defense
  against a bare arrow-step name reaching end-of-stream, since L1 now refuses
  that universally, before any L2 overlay is even consulted.

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
absent here is outside the emitted grammar.

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

The 69-query count above is exactly the frozen Spider-derived gold corpus's
`let`-block figure, and every one of those 69 binds a **pipeline** — the
`scalarExpr` alternative §5.1 added (issue #352) is not corpus-derived and
contributes 0 to it; it is oracled separately by the modern-dialect seed
corpus below (`issue-352/let-scalar:*`), the same second-oracle mechanism
`%latest` (G2) and arm-R (G1) already use for constructs the frozen corpus
never exercised.

### 5.8 Modern-dialect seed corpus (a second oracle)

The Spider-derived `corpus/gold_queries.jsonl` (§5.7) is frozen at 5,034 queries;
it never exercised some **modern Legend Pure** constructs the fine-tuned model
also emits. Those are seeded in a _separate_, provenance-distinct file,
`corpus/modern_dialect_seeds.jsonl`, so the 5,034-query gold corpus and every doc
citation of its count stay untouched. `tests/modern_dialect_soundness.rs` replays
each seed through the real byte-PDA with the same killer property as §8.1 (never
dead, ends accepting) and classifies it to its declared envelope. The seed corpus
is the oracle for anything added here — do **not** add a production without a seed.

**A seed is only an oracle if it is real Legend Pure.** Two of the G2 rows were
not: `gap-report/g2-latest:4` claimed `Class.all(%latestdate)` and `:5` claimed
`%latest` as a comparison operand, and the pinned engine rejects both outright.
Nothing caught it — L1 accepting more than the engine is exactly §5.10's
documented over-approximation, so the soundness lane stayed green while the
oracle was wrong, and `milestoneLit` had been widened to `%<lowercase>+` to
admit a string that was never in the language. Issue #55 Phase 7 corrects both
rows to live-attested shapes (single-argument property milestoning, and a chained
milestoned navigation; `issue-55/g2-latest-corrected:4`/`:5`) and adds the gate
that closes the class: `every_modern_dialect_seed_parses_against_the_pinned_engine`
sends every seed through the engine's own `grammarToJson/lambda`, so a seed that
is not real Pure cannot be committed again.

| Construct                             | Seeds | Grammar production               | Gap report |
| ------------------------------------- | ----: | -------------------------------- | ---------- |
| `%latest` milestoning                 | 5     | `milestoneLit` (§5.4)            | G2         |
| `~` Relation/Function API (arm-R)     | 14    | arm-R productions (§5.9)         | G1         |
| `letBinding` scalarExpr initializers  | 3     | `letBinding`/`scalarExpr` (§5.1) | issue #352 |

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
machine adds `SawTilde` (reached from the value hubs on `~`), transitioning on
`[` (a relation column-set, `Frame::RelColBracket`), a bare identifier, or a
quoted string (a column reference) — each routed to its own colName states
(`InRelColIdent`/`InRelColStrLit`, closing at `AfterRelColName`), *not* the
generic identifier/string-literal/value-hub states arm-A/arm-C terms use. That
separation is deliberate: a `~`-column position is a disjoint sub-grammar from
the generic value/expression hub, not a value a `|` can attach to, so
`AfterRelColName` admits a `:` (opening `AfterRelColColon`, arm-R's own
binder-colon position) and everything else a completed value admits, but
never a `|` — live-attested, `over(~a|$a.b)`, `rename(~old|$x.a, ~new)`,
`sort([ascending(~a|$a.b)])` and `groupBy(~[a|$a.b], …)` are each "no viable
alternative" right at the `|`. A `~[…]` bracket item *past a comma* reopens
the same restricted position (`ExpectRelColSpecReq`), not the generic one,
since a second item has no fresh `~` in front of it to route through
`SawTilde` again — without that, only a bracket's *first* item would have been
narrowed (issue #361).

`AfterRelColColon` is arm-R's own colon continuation, distinct from the
generic typed-binder `AfterColon` every other Pure lambda binder's colon
reuses (`row: Person[1]|…`). Every arm-R lambda's `binderVar` is a *bare*
identifier (`colLambda`/`mapLambda`/`frameLambda`, §5.3) — never a typed
one — so this position admits only a bare binder identifier
(`InRelColLambdaBinder`, closing at `AfterRelColLambdaBinder`, which admits
only whitespace or its own binder pipe) or the `winAggSpec`/`relAggSpec`
aggregation form's brace lambda (`agg: {p,w,r|…}`) — never a typed binder's
`::` classpath or `[` multiplicity. Live-attested: `over(~[k: t::A[*]|$k.k])`
and the same shape with no `$` on the body variable are each "no viable
alternative" right past the colon, exactly like the untyped bare-lambda form
issue #361 closed off `AfterRelColName` itself (issue #368).

Everything else in arm-R — the `:` column-to-lambda separators past the first,
the `over(~…)` partition otherwise, the `{p,w,r|…}` frame bodies, the
reducers, the bracket nesting — still reuses the shared value-hub / lambda /
bracket machinery of §5.2–§5.3. So the grammar still admits a superset of the
strict productions above (e.g. it does not enforce that a `winAggSpec` colon
is bare while a `relAggSpec` colon carries a leading `~`, nor that `over(…)`'s
argument is only ever a `colRef` list rather than the general bracketed
colSpec form every other arm-R construct shares, nor that a *second*
lambda-to-lambda colon inside `relAggSpec`/`winAggSpec`
(`mapLambda ":" reduceLambda`) keeps its own `reduceLambda` binder bare —
that second colon is reached off a *completed value*, not `AfterRelColName`,
so it still opens the generic `AfterColon` typed-binder path and is
out of this issue's scope); the compiler oracle catches that residue, exactly
as §5.6 sanctions — but a *lambda* standing in for a column name, at any of
these positions, is no longer part of it. Like every other L1 identifier, a
`~`-column name is an **L2 pass-through** (it opens at the `SawTilde`/
`ExpectRelColSpec`/`ExpectRelColSpecReq` anchors, whose rule is `None`), so
arm-R never masks the model's emitted column names.

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
