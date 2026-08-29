# PureCARD Spec — L2 schema-consistency

_Part of the [PureCARD spec](README.md); see also the [domain model](../domain-model.md)._

## 6. L2 — schema-consistency (the schema-aware constraint level)

L2 is the semantic overlay that L1 cannot express. Given (a) the emitted-Pure L1 grammar and (b) a `Schema` for the target database, L2 defines the additional per-position constraints that keep a partial query referencing only **real, correctly-typed model elements**. It narrows at exactly the positions L1's §7 consistency-contract table enumerates; it never _widens_ what L1 allows.

**Core principle (oracle-driven).** Every rule below is derived from, and testable against, the execution-verified gold corpus **and its schemas**. A rule that masks a token appearing in a gold query for that schema is a soundness bug. Do not invent constraints the corpus does not exercise.

### 6.1 Why L1 cannot do this (the context-sensitivity)

A context-free grammar can enforce that `$x.` is followed by _an identifier_. It cannot enforce that the identifier is one of `{id, maker, fullName, country}` **because that set depends on the class `$x` is bound to**, which depends on the `.all()` source and any intervening association navigation — a context-sensitive fact. L2 threads a small **typed scope** through the parse and, at exactly the identifier and operator positions, intersects L1's terminal set with the schema-legal set for the current scope.

### 6.2 The `Schema` data-contract

The minimal per-database structure a schema-aware decoder consults. It is populated **host-side** (never by the decoder — the decoder never calls Legend) from the PureModelContextData (PMCD) or, equivalently, the MCP reflection tools (§6.3), then handed to the decoder at session init. All names are the **autogen model identifiers** (camelCase properties, PascalCase class simple-names, fully-qualified `spider::db::model::default::Class` paths) exactly as they appear in the ctx brief and gold queries — never the underlying SQL table/column names.

#### 6.2.1 Structure

The structure below is the **JSON contract `Schema::from_json` deserializes** (the serde field names are authoritative — this is what the host must emit):

```
Schema {
  db_id:        string
  db_path:      string                             // the store/database path (REQUIRED). N3 admits it
                                                    // as a legal pipeline source alongside real classes (6.5)
  classes:      Map<ClassPath, ClassInfo>          // keyed by fully-qualified path
  associations: List<AssociationSpec>              // optional (default []); navigability derived, see 6.2.3
  enums:        Map<EnumPath, List<EnumValue>>     // optional (default {}); enumeration path -> its values
}

ClassInfo {                                        // the class path is the Map KEY, not a field
  simple_name:          string                     // "CarMakers" (the .all() head the model emits)
  properties:           List<PropertySpec>         // stored/regular properties, declared order
  qualified_properties: List<QualifiedPropertySpec>// derived properties (optional, default [])
  super_types:          List<ClassPath>            // inherited members resolve transitively (optional, default [])
}

PropertySpec {
  name: string                                     // "horsepower"
  type: PropType                                   // JSON key is "type"
  mult: Multiplicity
}

PropType =                                         // internally tagged on "kind"
  | { kind: "primitive", name: PrimName }          // one of the Pure primitives, 6.2.2
  | { kind: "class",     path: ClassPath }         // a complex/class-typed property (navigation continues)
  | { kind: "enum",      path: EnumPath }          // an enumeration-typed property

Multiplicity { lower: u32, upper: u32 | null }     // upper=null is * (unbounded): [1]->{lower:1,upper:1},
                                                    // 0..1->{0,1}, 1..*->{1,null}

AssociationSpec {
  path: AssociationPath
  ends: [AssociationEnd; 2]                         // exactly two ends (well-formed assoc)
}
AssociationEnd {
  property_name: string                             // "fk0DefaultContinents"
  target_class:  ClassPath                          // Continents
  mult:          Multiplicity                       // [1]
}
// NOTE (Pure semantics, verified): an end's property is navigable FROM the class at the OTHER end
// and yields target_class[mult]. See 6.2.3.

QualifiedPropertySpec {
  name:        string                               // "doubled"
  return_type: PropType                             // its declared return type
  return_mult: Multiplicity
  // parameter list exists in the PMCD but is not needed for identifier narrowing; a decoder MAY
  // ignore args and treat a qualified property as a nav step yielding return_type (MVP), or narrow
  // its argument positions later. Args are rare in the emitted subset.
}

EnumValue = string                                  // the enum literal, e.g. "ACTIVE"
```

`PropType`'s three-way split (`kind: "primitive"` | `"class"` | `"enum"`) is load-bearing: the type determines whether a `.` after this property **continues navigation** (`class`), **terminates at a value** (`primitive`), or **narrows a comparison RHS to enum values** (`enum`). A flat `type: str` is insufficient; a decoder MUST split it.

#### 6.2.2 The primitive type set (from the autogen models)

`PrimName ∈ { Integer, Float, Decimal, Number, String, Boolean, Date, StrictDate, DateTime }`. For the **type rules** (§6.5) primitives collapse into type _classes_:

- **numeric** = { Integer, Float, Decimal, Number } — comparable with `< > <= >=` and number literals; aggregatable with `sum`/`avg`.
- **string** = { String } — comparable with `== !=` and single-quoted literals; string predicates.
- **boolean** = { Boolean } — comparable with `== !=` and `true`/`false` only.
- **temporal** = { Date, StrictDate, DateTime } — comparable with `< > <= >=` and date literals.

(The autogen pilot models are numeric/String/Boolean-heavy; temporal appears in other Spider DBs. Enums are rare in the Spider-derived corpus but MUST be supported for general PMCDs.)

**Declared-type caveat (verified).** Some SQL numeric columns are declared `String` in the autogen model (e.g. car_1's `horsepower`/`mpg`, a TEXT-affinity artifact). `PropType` MUST reflect the **model's declared** type, not the SQL intent: a String-typed numeric column is correctly constrained by L2 as **String**. The model, not the SQL, is L2's ground truth.

#### 6.2.3 Association navigability (the subtle rule)

An `AssociationSpec` with ends `[e0, e1]` yields **two directed navigations**:

- from `e0.target_class`, the property **`e1.property_name`** is navigable and yields `e1.target_class` with `e1.mult`;
- from `e1.target_class`, the property **`e0.property_name`** is navigable and yields `e0.target_class` with `e0.mult`.

Concretely, `fk_0 = { fk0DefaultCountries: Countries[1..*], fk0DefaultContinents: Continents[1] }` means: **from a `Countries`** you may navigate `.fk0DefaultContinents` → `Continents[1]`, and **from a `Continents`** you may navigate `.fk0DefaultCountries` → `Countries[1..*]`. This is exactly what the gold query `Countries.all()->filter(x|$x.continent == $x.fk0DefaultContinents.contId)` does. Getting the direction backwards is a soundness bug (it would mask `fk0DefaultContinents` on `Countries`). A decoder therefore precomputes, per class, its **navigable set** = { each opposite-end property }.

#### 6.2.4 Provenance — how the contract is fed

The decoder never calls Legend; the host builds `Schema` once, at session init, from either source (they are the same PMCD, different access paths). The MCP reflection tools live in the upstream project's `mcp_server` (tool names below are stable API):

| Contract field                                     | MCP tool                                                                                | PMCD field                                                         |
| -------------------------------------------------- | --------------------------------------------------------------------------------------- | ------------------------------------------------------------------ |
| `classes[*].properties` (name, type, multiplicity) | `legend_describe_class` → `properties[]` (`name`, `type`, `lower_bound`, `upper_bound`) | class `properties[].genericType.rawType.fullPath` + `multiplicity` |
| `classes[*].super_types`                           | `legend_describe_class` → `super_types[]`                                               | `superTypes[].path`                                                |
| `classes[*].qualified_properties`                  | `legend_get_derivations` → `derivations[]` (`name`, `return_type`)                      | `qualifiedProperties[]` (`returnGenericType`)                      |
| `associations` (ends, targets, multiplicities)     | `legend_get_associations` → `associations[].properties[]` + `other_end_class`           | `Association.properties[]`                                         |
| `enums`                                            | `legend_list_enums` → `enums[]` (`path`, `values`)                                      | `Enumeration.values[]`                                             |

`legend_describe_class` returns `type` as a full path string; the host classifies it into `PropType`: if it is a primitive path → `Primitive`; if it resolves to a `class` element → `ClassRef`; if to an `enumeration` element → `EnumRef`. Milestoning `target_stereotypes` are ignored by L2 (they affect _arguments_, not name/type resolution).

### 6.3 The `Schema` construction is host-side

How the PMCD / MCP tools are queried to _populate_ the contract is host-side. This spec defines the contract's _shape and semantics_, not the extraction, and the decoder ingests `Schema` from JSON at session init (`Schema::from_json`, §9).

**L2 enforcement is mask-first.** `allowed_mask()` is the _sole_ point that
enforces the supported L2 rules (§6.7 — N1/N2/N3/N5/N6, T2, T3, and part of T1): with a
schema set it intersects the syntactic (L1) mask with that schema-legal set,
clearing tokens illegal under a covered rule. `accept_token`/`accept_byte` enforce
only the **grammar** — a schema-masked token that is grammar-legal is still
accepted (the L2 tracker advances in lockstep so the _next_ mask is correct, but
acceptance is never gated on the narrow). The host contract is therefore: read
`allowed_mask()`, sample only from the admitted set, then commit with
`accept_token`. Do not treat `accept_token` as a schema-validation backstop, nor
`allowed_mask` as a guarantee of full schema validity.

### 6.4 The scope-tracking state machine

L2 maintains a small **scope stack**. The top-of-stack `Scope` determines narrowing. A `Scope` is one of:

- `ClassScope(class_path, var_name?, multiplicity)` — a row is a single instance of `class_path` (multiplicity tracks whether we are on a to-one or to-many path);
- `RelationScope(columns: List<ColName>)` — the pipeline has become a TDS/relation (after `project`/`groupBy`); rows are named columns, not class instances.

#### 6.4.1 Transitions

1. **Source (S1).** On `ClassPath.all()`, the class must exist in `Schema.classes` — or, for the arm-A `Db->tableReference(...)` store source, be the schema's `db_path` (rule N3: `Schema::is_source` accepts a real class key **or** the `db_path`). Set the pipeline scope to `ClassScope(ClassPath, var=None, mult=(1,1))`.
2. **Lambda entry (S2).** On entering a lambda `var | …` (inside `filter`, and inside each `colLambda`/`keyLambda`/`mapLambda`), bind `var` to the _current pipeline element type_: push `ClassScope(current_class, var, (1,1))`. The bound var is the only in-scope row variable inside the lambda body.
3. **Navigation entry (S3).** On `$var.` where `$var` is the bound var, the next identifier is narrowed (N1). After it is consumed:
   - if it is a `Primitive`/`EnumRef` property → the nav expression's _resolved type_ is that primitive/enum; navigation cannot continue (a further `.` is illegal, N-terminal).
   - if it is a `ClassRef` property or an **association navigation** → advance the nav scope's class to the target class and multiply multiplicities (rule S-mult); a further `.` now narrows to the _target_ class's members (N2). This is the chained navigation `$x.fk2DefaultCarMakers.fullName`.
   - if it is a **qualified property** → the resolved type is its `return_type`; if that is a `ClassRef`, navigation may continue from the returned class (MVP: treat like a ClassRef step).
4. **Lambda exit.** On the lambda's closing boundary, pop the lambda `ClassScope`; the pipeline scope is unchanged by `filter` (filter does not change the element type).
5. **project / restrict / olapGroupBy.** `project([colLambdas], [names])`, `restrict([names])`, and `olapGroupBy([partCols], sortSpec, reduceLambda, 'outName')` change the pipeline scope to `RelationScope(names)` — the emitted `names` string-literals (for `olapGroupBy`, the partition columns plus `'outName'`) become the column universe. After this point, class-property narrowing no longer applies; `sort('col', …)` column references and the TDS-column accessors `$r.getInteger('col')` / `getFloat` / `getString` / `getBoolean` (rule N6) and further `restrict` names must be members of the current `RelationScope`. (The `getX('col')` accessor is the post-aggregate HAVING-style read — e.g. `->filter(r|$r.getInteger('cnt') >= 2)` — and is a first-class N6 position; its `strlit` arg is the `colAccess` production in §5.3.)
6. **groupBy.** `groupBy([keyLambdas], [aggs], [names])` also yields `RelationScope(names)`, where `names` are the group-key + aggregate output names. Inside each `keyLambda` and each `agg`'s `mapLambda` the scope is still `ClassScope(source_class, var)` (the lambdas run over the pre-group rows) — so their bodies narrow against the source class, exactly as gold `groupBy([x|$x.fk2DefaultCarMakers.fullName, …], [agg(x|$x.modelId, y|$y->count())], […])`.
7. **agg reduce lambda.** Inside `agg(mapLambda, reduceLambda)` the `reduceLambda` var (`$y`) is bound to the _collection of mapped values_; its element type = the `mapLambda`'s resolved type. This is where aggregation type rules (T3) fire: `$y->sum()` is legal only if that element type is numeric.
8. **sort / take / limit / distinct.** Do not change scope type. `sort` references a column name (N6); `take`/`limit` take an int; `distinct` takes nothing.

#### 6.4.2 Worked example (DB: `car_1`)

Gold query (verified):

```pure
|spider::car_1::model::default::Countries.all()
  ->filter(x|$x.continent == $x.fk0DefaultContinents.contId)
  ->groupBy([x|$x.fk0DefaultContinents.contId, x|$x.fk0DefaultContinents.continent],
            [agg(x|$x.countryId, y|$y->count())],
            ['ContId','Continent','count'])
```

Position-by-position scope + narrowing:

| Position | Scope before | L2 action |
|---|---|---|
| `spider::…::Countries` (source) | — | N3: must be a real class path **or** the store `db_path` (`Schema::is_source`); here it is a class. Set `ClassScope(Countries,(1,1))`. |
| `.all()` | ClassScope(Countries) | pipeline element type = `Countries`. |
| `filter(x\|` | ClassScope(Countries) | S2: bind `x`→`Countries`. |
| `$x.` → `continent` | ClassScope(Countries, x) | N1: `continent` ∈ Countries.properties `{countryId, countryName, continent}` ✓; type `Integer[0..1]` (numeric). |
| `==` | resolved LHS numeric | T1/T6: LHS is `[0..1]` scalar numeric → RHS must be numeric-typed. |
| `$x.fk0DefaultContinents` | ClassScope(Countries, x) | N1+N5: `fk0DefaultContinents` is the navigable end of `fk_0` **from Countries** ✓ → advance to `Continents[1]` (S-mult keeps scalar). |
| `.contId` | (nav) Continents | N2: `contId` ∈ Continents.properties `{contId, continent}` ✓; type `Integer` (numeric) → RHS type-matches LHS. Comparison legal. |
| `groupBy([x\|$x.fk0DefaultContinents.contId, …]` | ClassScope(Countries) per keyLambda | S2 rebinds `x`→Countries; nav narrows as above; both keys resolve. |
| `agg(x\|$x.countryId,` | ClassScope(Countries, x) | mapLambda: `countryId` ∈ Countries ✓ → numeric. |
| `y\|$y->count()` | reduce over numeric collection | T3: `count` legal on any collection ✓. |
| `['ContId','Continent','count']` | → RelationScope | scope becomes `RelationScope({ContId,Continent,count})`; any following `sort`/`restrict` narrows against these names. |

A **counterfactual** the overlay must reject: `Countries.all()->filter(x|$x.maker == 'Ford')` — masked at `maker`, because `maker` is not a Countries property (it is a CarMakers property); and even if a `makerName` existed, `== 'Ford'` on a numeric FK column would be masked by T1. Both are _phantom / type-mismatch_ errors L2 exists to eliminate.

### 6.5 Narrowing rules (identifier positions) — N1–N6

Each rule = "at this position, intersect L1's terminal set with this schema-legal set." All sets are computed from `Schema` and the current `Scope`.

- **N1 — property/first-navigation narrowing.** At `$var.<IDENT>` — and at a `.` taken straight off a class extent (`Class.all().<IDENT>`, whose base is the pipeline's own class: the one navigation position with neither a `$var` nor a prior nav cursor to read a base from, and so the one this rule used to leave wholly unnarrowed) — where the base is bound to `ClassScope(C)`: legal `<IDENT>` = `C.properties[*].name` ∪ `C.qualified_properties[*].name` ∪ `navigable(C)` (the opposite-end property names of every association touching `C`, per §6.2.3) ∪ the same three sets for every class in `C.super_types` (transitively). Nothing else. Pure spells a member either bare or quoted (`$x.name`, `$x.'Gross Credits'`) and both name the same set, so both spellings are narrowed against it and a phantom in either is cleared (issue #55 Phase 4; live: `{|…::Countrylanguage.all().'Capital_T1'}` → "Can't find property 'Capital_T1' in class …::Countrylanguage").
- **N2 — chained-navigation narrowing.** After a ClassRef/association step advanced the scope to target class `T`, a further `.<IDENT>` narrows to `T`'s member set (N1 computed for `T`).
- **N3 — source-class narrowing.** At the pipeline-source `classpath`, the fully-qualified path must be a key of `Schema.classes` **or** the schema's `db_path` (the store, which arm-A relational queries name as their `tableReference` source). (Phantom-class prevention; catches `test::DoesNotExist.all()`.)
  **Classpath continuation (`::`).** The narrowing holds across the whole `::`-joined path, not only its first segment: a `::` is admissible only where the extended path is still a prefix of some legal source path. This matters because `::` does **not** end the identifier lexeme — the byte-PDA keeps a source classpath open across the separator (`InSourceIdent → SourceColon → SourceColon2 → InSourceIdent`), so unlike a `.` or `(` there is no boundary at which L1 re-vets what follows. A trie walk that read the colon as a name-completing boundary therefore released the constraint the moment a real path completed, admitting a fabricated segment glued onto it (`spider::world_1::Db` + `::desc`), live-rejected as "Can't find the packageable element". The rule as stated above always covered this; closing the gap was an implementation fix (issue #55 bucket A), and the boundary predicate is now stated per rule (`NameShape`, `src/schema/trie.rs`) instead of assumed identifier-shaped.
  **Source continuation (N3c).** A whole source path owes exactly one continuation, and which one is decided by the kind of path it is. A **class** path denotes the `Class<T>[1]` metatype, not the `T[*]` extent, so its only legal continuation is its own `.` (which S1 then forces into `all` and its call): every method arrowed straight off it mismatches by construction — live-attested, `|…::Country->groupBy('CountryCode_T2_2')` is rejected with "Can't find a match for function `groupBy(Class<Country>[1],String[1])`". The schema's **store** path is the exact mirror: it denotes a `meta::relational::metamodel::Database`, which has no extent, so `.` is inadmissible (`|…::Db.all()` → "Can't find a match for function `getAll(Database[1])`") and `->` is mandatory. End-of-stream is masked at both, so a bare source path can never end a query. The corpus agrees at both ends and exercises nothing else: across the 5034 gold queries a class source path is followed by `.all` 501 times and a store path by `->tableReference` 8455 times, with zero occurrences of either inverse. (Issue #55 Phase 3; the byte-PDA already rejects whitespace, closers, operators and `(` at a source classpath, so N3c's net effect is exactly the `->`/`.` split.)
  **Store-method narrowing.** The identifier a store path's `->` opens must name a store method, and — like S1's `all` — owes its own `(`. The legal set is read off the corpus, never invented: `tableReference` is the only name that ever follows a store path there. Everything else the vocabulary offers either has no `Database` overload at all (live: `|…::Db->tableToTDS()` → "Can't find a match for function `tableToTDS(Database[1])`") or matches a generic collection builtin that returns the store straight back (`|…::Db->limit(3)` → `Database`), which is never a query.
  **Store-method call shape (N3d).** The store method's own call is fully determined: every parameter is a `String[1]` and it takes exactly two of them, so its argument slot admits only whitespace and a string literal's opening quote, and the position after a completed argument admits only the `,` the call still owes or — once it owes none — its `)`. Masking the closer at an *open* argument slot is the arity half: `->tableReference()` and a one-argument `->tableReference('T')` cannot be walked at all. The engine states the signature in its own rejection (`tableReference(Database[1],String[1],String[1]):Table[1]`, live) and the corpus agrees: all 8455 store-method calls across the 5034 gold queries pass exactly two single-quoted strings, and none passes anything else. (Issue #55 Phase 4; this is the position S1's `SourceMethodArg` already occupies for `all()`'s own call, applied to the store arm.)
  **Class-extent continuation (N3e).** Once the source method's call closes, `Class.all()` has produced the `T[*]` **extent**, and only three things ever follow one: a pipeline step (`->`), a property navigation that maps over it (`.`), or the end of the query. Every binary operator the vocabulary offers is a type mismatch against a collection — live-attested, `{|…::ModelList.all() && 'Year_T1'}` is rejected with "Can't find a match for function `and(ModelList[*],String[1])`" and `{|…::CarMakers.all()*'Accelerate_T2'}` with "Collection element must have a multiplicity [1]". The corpus exercises exactly those three continuations and nothing else: across the 5034 gold queries a closed `.all()` is followed by `->` 438 times, by a `.` property 37 times and by end-of-query 25 times. The `-` of the step arrow is admitted only *as* the arrow — either whole, or as the bare `-` a vocabulary that splits the connector offers, with the very next token then narrowed to the `>` that completes it — so an arithmetic minus cannot be reassembled a byte at a time. N3e is the direct successor of N3c: N3c stops a method being arrowed off the bare `Class<T>[1]` metatype, N3e stops one being *operated on* once `.all()` has produced the extent. (Issue #55 Phase 4.)
  **Extent receiver category (N3f).** N3e admits the step arrow off a closed `Class.all()`; N3f decides what that arrow may open. A `T[*]` class extent is neither a relation nor a primitive scalar, so a method whose *every* overload demands one of those receivers can never match it, whatever arguments follow — the name is dead at the arrow, not at its argument list. The set is read off the engine's own function registry: asked for a name it cannot match, the compiler prints back the whole candidate signature set, and for each denied name not one candidate's receiver parameter admits a `T[*]` class extent. Two categories account for all of them — **relation/store** receivers (`agg`, `join`, `renameColumns`, `restrict`, `tableReference`, `tableToTDS`: `restrict(TabularDataSet[1],String[*])`, `tableReference(Database[1],String[1],String[1])`, `tableToTDS(Table[1])`, …) and **primitive-scalar** receivers (`average`, `between`, `endsWith`, `in`, `pair`, `parseFloat`, `startsWith`, `substring`, `sum`, `toLower`, `toString`, `year`: `average(Number[*])`, `pair(U[1],V[1])`, `toString(Any[1])`, `year(Date[1])`, …).

  Unlike N3c's store arm this is a **deny** set, and deliberately so: there is no closed permit set to state. `at`, `drop`, `slice`, `add`, `init`, `tail`, `first`, `last`, `removeDuplicates`, `reverse` and `fold` all compile on a class extent (live-verified) while appearing in no corpus, so an allow-list built from corpus method names would mask eleven legal collection builtins — the real compile bar is more permissive than a type-theoretic reading of the corpus suggests (the same finding issue #127's spike recorded for `min`/`max` over a to-many navigation). A denied name is cleared at the token that *closes* its lexeme, never at its first byte, so a denied name that is a live prefix of a legal one keeps walking (`in` ⊂ `init`) — the same close discipline N3c needs for `Country` ⊂ `Countrylanguage`. (Issue #55 Phase 5.)
  **Receiver-only call arity (N3g).** N3f decides which names a class extent's arrow may open; N3g decides how long the argument list of one of them may be. For every builtin whose *entire* engine overload set is receiver-only — `count(Any[*])`, `isEmpty(Any[0..1])`/`isEmpty(Any[*])`, `isNotEmpty(Any[0..1])`/`isNotEmpty(Any[*])`, `size(Relation<T>[1])`/`size(Any[*])`, `toOne(T[*])`, each read off the engine's own printed candidate set — an **arrow** call has already supplied that one parameter, so the slot its `(` opens admits nothing but whitespace and its own `)`. This is the exact complement of N3d's arity half: there an opened slot owes an argument and the closer is cleared, here it owes none and every opener is cleared. Live-attested on four receiver categories (a class extent, a `TableTDS`, a primitive collection and a `filter` result), and corpus-attested: across the 5034 gold queries these names are called 3048 times and never with an argument. Stated of the arrow form alone, because the plain-function form spends the same single parameter on its argument — `|count(…::Country.all())` and `|isEmpty($x.name)` both compile live. `distinct` is deliberately excluded (the engine answers `RuntimeException: Not possible!` with no candidate list, so the oracle states nothing to encode) and so is `sort`, which takes a comparator argument in all 1048 of its corpus calls. (Issue #55 Phase 6; bucket D's arity half.)

  **Store-result continuation (N4a).** The store arm's dual of N3e. Once `Db->tableReference('T','S')` closes it has produced a `Table[1]`, which is neither a `Boolean`, a `Number`, a `String` nor a `Date`, so every ordered, arithmetic and logical operator the vocabulary offers mismatches its overload set by construction — live: `and(Table[1],String[1])`, `or(Table[1],String[1])`, `greaterThan(Table[1],String[1])`, `lessThanEqual(Table[1],String[1])`, `divide(Table[1],String[1])`, `plus(Any[2])`, `minus(Any[2])`, `times(Any[2])`. Unlike N3e's permit set this rule is **subtractive**, because the store result really does accept more than the extent does: a bare `|…::Db->tableReference('T','S')` compiles and returns `Table`, `== 'x'`/`!= 'x'` compile through `equal(Any[1],Any[1])`, and `.name` resolves on the metamodel `Table`. So the rule clears exactly the operator family and leaves closers, separators, the navigation dot and the equality comparators alone. The `-` of the step arrow gets N3e's own treatment — admitted only as the arrow, with the next token narrowed to the `>` that completes it — since `->tableToTDS()` is the continuation all 8455 corpus store calls take. (Issue #55 Phase 6; bucket E.)

  **Logical-operand type (N4b).** `and`/`or` have Boolean-only overloads (`and(Boolean[1],Boolean[1])`, `and(Boolean[*])`), so the operand slot a `&&`/`||` opens can never hold a string, numeric or date literal — live on both sides: `'a'&&true`, `true&&'a'`, `true&&1`, `1||true` and `%2020-01-01&&true` all fail, while `true&&true` and `('a'=='b')&&(1<2)` compile. The narrowing is T1's own literal-class predicate applied at a Boolean-typed slot; it is a position of its own rather than a reuse of T1's `ReValue(Boolean)`, which stays deferred, because T1 governs a *comparison*'s operand where `equal(Any[1],Any[1])` keeps a type-mismatched literal legal. Zero occurrences of a literal directly after `&&`/`||` across all three corpora. The arming reads the operator through the same two-byte reassembly `classify_at` uses, so a vocabulary that *splits* `||` into two `|` tokens still arms the rule. **Known limitation, in the other direction:** a vocabulary that *fuses* the operator with its operand into one token (`||'CountryId_T2'`) offers that token at the **preceding** anchor, where this position is not yet active, and it escapes. This is a property of every operand rule in the overlay, T1's `ReValue` included, and its fix is a fused-operand narrower of the kind §6.5's fused nav-dot pass already provides for N1/N2/N6. (Issue #55 Phase 6; bucket E.)

  **String-literal operator (N4c).** N4b's mirror image, read from the completed literal on the operator's left. `minus`, `times` and `divide` have no `String` overload at all — live: `minus(String[2])`, `times(String[2])`, `divide(String[1],String[1])`, and `minus(Any[2])`/`times(Any[2])` once the two operands' classes differ — so `-`, `*` and `/` can never take a string literal as their left operand. The deny set is exactly those three and nothing more: `+` is string concatenation (`plus(String[*])`, live), the ordered comparators have a real `greaterThan(String[1],String[1])` overload, and `&&`/`||` follow a string literal all through the corpus while taking the enclosing *comparison* as their operand — a comparison binds tighter than a conjunction, so the canonical `filter(x|$x.a == 'p' && $x.b == 'q')` must and does stream. The `-` again gets the arrow treatment, and here it matters most: across the three corpora a string literal is followed by `-` 32309 times and every single one opens a `->` (`'FacID'->pair('FacID_T1')`), never an arithmetic minus. Because a string literal is dispatched only once a later token closes it, the rule is also read at the byte-PDA's pending-closing-quote state — the same in-lexeme position N3d needs — and only where no other rule (N6's column, T1's operand) already governs the literal. (Issue #55 Phase 6; bucket E.)
- **N4 — enum-value narrowing.** When a nav expression resolves to `EnumRef(E)` and is compared (`== / !=`), the RHS enum literal `E.value` (or `EnumPath.value` form) is narrowed to `Schema.enums[E]`. Nothing outside that enum's declared values. The emitted L1 grammar has no enum-literal operand position, so this rule is outside the supported overlay. (`SortDirection.ASC/DESC` is a Pure builtin inside `sort`, not a schema enum or an N4 position.)
- **N5 — association navigability direction.** A navigation property is legal from `C` only if it is the _opposite_ end of an association whose other end targets `C` (§6.2.3). This prevents emitting a navigation from the wrong side of the association.
- **N6 — relation-column narrowing.** In `RelationScope(cols)`, every reference to an emitted column name must be a member of `cols` (the names emitted by the preceding `project`/`groupBy`/`olapGroupBy`). Four reference positions occur in the corpus and are all narrowed: (a) a `sort('<COL>', …)` / `asc('<COL>')` / `desc('<COL>')` column string; (b) any `restrict([...])` or later `project` name-reference; (c) the **TDS-column accessor** `$r.get{Integer,Float,String,Boolean}('<COL>')` — the post-aggregate HAVING read (`->filter(r|$r.getInteger('cnt') >= 2)`), which is the single most common relation-column reference (340+ gold occurrences); and (d) the trailing column `<IDENT>` in the `->in(subquery.<IDENT>)` membership form (47 gold), narrowed against the **subquery pipeline's own terminal `RelationScope`** — the subquery is entered as an independent scope (§6.4), so its projected column universe, not the outer pipeline's, is the legal set. This keeps post-projection column references real. (Weaker than N1–N5: column names are string-literals, so this is enforced only where the model references a _previously emitted_ name; it is the relation-side analogue of property narrowing. The `getX` accessor additionally fixes the _type_ of the read — `getInteger` on a numeric column — which L2 MAY check against the aggregate's output type, but the compiler oracle also catches a `getString` on a numeric column, so this is an optional tightening.)

- **N7 — bare value-identifier continuation.** A bare identifier emitted at a **value** position, while its own lexeme is still open, may continue only into one of the shapes that give a bare word a meaning in Pure: a lambda binder (`x|…`, `x: T[1]|…`), a package separator (`::`), a navigation or enum-value selection (`.`), or a function application (`(`). Every other continuation — whitespace, `,`, any closer, any operator — and **end-of-stream** are masked, because they end the word as a standalone expression and a standalone bare word resolves to nothing ("Can't find the packageable element 'pair'", issue #55 bucket B). This narrows no name *set*: a novel binder name must stay admissible, so the rule constrains only what may follow one. Three carve-outs, each corpus-driven: the boolean literals `true`/`false` are complete values and are left alone; an arm-R `~[Col, …]` key is a complete value too (no fixture corpus exercises arm-R, so §4 forbids inventing a continuation set for it); and the rule governs only an *identifier* lexeme — a string/number/date literal opens at the same anchor but is not a bare word, and byte-level BPE fragments it. Whitespace is masked deliberately even though `x |` is legal Pure: admitting it would let one space close the lexeme and drop the rule out of scope, handing the escape straight back (the same reason S1's must-call veto below excludes it).

**S2 as a narrowing — the refVar position.** §6.4.1's S2 supplies scope state (which class a `var` is bound to); the same transition also *constrains* the position it names. At `$<IDENT>` the identifier must be a variable something in the stream has actually bound — Pure resolves `$x` against the lambda and `let` bindings in scope, so an unbound name is not a type error but a missing graph element ("Can't find variable class for variable 'code' in the graph", issue #55 bucket C). The legal set is every name the tracker has seen bound: a lambda binder (`filter(x|…)`, each `colLambda`/`keyLambda`/`mapLambda`/`reduceLambda`), a join brace lambda's typed binders (`{row1: …[1], row2: …[1]|…}`), and a `let` binding (`{|let topStates = …; … ->in($topStates)}` — bound by position, with no pipe to confirm it, and outliving its own statement).

The set is deliberately **monotonic and a superset**: a name is recorded wherever the tracker sees a binder candidate and is never retracted on scope exit. Two reasons. (a) *Soundness* — the scoped bindings do not model every binder form the grammar admits (a join lambda's second typed binder reaches no pipe-binding path at all), and narrowing against them would mask real gold queries; a superset means S2 only ever masks a name **nothing anywhere bound**, which is precisely the failure class it targets. (b) *Cache exactness* — a set that only grows is pinned by its length within a stream, so the rule needs no name fingerprint in its cache key, exactly as N6's emitted-column count pins its set. The precision cost (a name bound in a sibling scope stays admissible) is the same trade N6's emitted-column superset already documents: over-recording only lets more through, never masks.

**Fused nav-dot tokens (N1/N2/N6 under byte-level BPE).** The narrowing above is stated at the identifier position _after_ the navigation `.`, but a byte-level BPE tokenizer (the Qwen/GPT-2 family) routinely packs the leading `.` and the identifier's first byte into a **single** token (`.theme`, `.name`, `.z` are each one token). The per-step mask is read at the anchor _before_ that token, where the member/column position is not yet active, and a token whose first byte is `.` is not an identifier-start, so the ordinary narrow keeps it — letting a phantom whose first character begins no legal member (`$c.maker`) stream unmasked and losing the narrowing entirely once committed. The overlay therefore applies a second, purely subtractive pass over exactly the fused `.`-led tokens: it resolves the class/relation a following `.` would navigate from (mirroring the scope machine's dot transition read-only over the still-open identifier) and clears any `.<ident>` whose post-dot identifier begins no legal name. The candidate gate is identifier-_start_ (a member name always begins with a letter/`_`, which the byte-PDA's post-`.` state requires), so a value-position leading-dot float (`.5`, digit-led) is never touched — the pass can only ever clear a genuine member/column navigation. Bare `.`, quoted members (`.'name'`), operators and non-`.` tokens all pass through, so `L2 ⊆ L1` (G4) still holds and no gold token is masked.

### 6.6 Type rules (operator / operand / reducer positions) — T1–T7

- **T1 — comparison operand-type compatibility.** At `navExpr cmpop operand`, the `operand`'s literal type must match the navExpr's resolved type class (§6.2.2): string prop ↔ single-quoted literal; numeric prop ↔ number literal; boolean prop ↔ `true`/`false`; temporal prop ↔ date literal. (Also admits `navExpr cmpop navExpr` when both resolved types share a type class — e.g. the gold `$x.continent == $x.fk0DefaultContinents.contId`, numeric ↔ numeric.)
- **T2 — ordered-comparator restriction.** `< > <= >=` are legal only when the resolved type is **numeric or temporal**; `== !=` additionally legal for string/boolean/enum. (Masks `boolProp > 3`.)
- **T3 — aggregation-reducer type rule.** In `agg(mapLambda, reduceLambda)`: `->sum()` and `->average()` legal only if the reduce lambda's declared element type is **numeric**; `->min()`/`->max()`/`->count()` are unconstrained. (The gold corpus uses exactly `count/average/min/max/sum`.)
  **Implementation note (2026-08-28, #56):** the reduce lambda's own type
  annotation (`y: Integer[*]|$y->sum()`) is read directly — no cross-lambda
  threading from the map lambda's body is needed. `min`/`max` were originally
  scoped to "numeric or temporal (ordered)" per the pilot survey, but a real
  `car_1` gold query uses `->min()` on a `String[*]` element (lexicographic
  ordering, matching SQL's `MIN`/`MAX`), falsifying that narrower reading
  against the 8 committed `FIXTURE_DBS`. With no counter-evidence to mask any
  type for `min`/`max` instead, they ship unconstrained (§4's corpus-evidence
  discipline: admit rather than invent).
- **T4 — string-predicate type rule.** `->startsWith(…)`, `->endsWith(…)`, `->contains(…)`, `->toLower()`/`->toUpper()` legal only when the receiver's resolved type is **String**.
- **T5 — enum-comparison type rule.** A nav expression resolving to `EnumRef(E)` may be compared only against a value of enum `E` (pairs with N4); comparing it to a string/number literal is masked. Because L1 has no enum-literal operand position, this rule is outside the supported overlay.
- **T6 — multiplicity / collapse rule.** An **ordered comparator** (`< > <= >=`)
  requires the navExpr on its left to be a **scalar primitive**. Those four
  operators dispatch to `lessThan`/`greaterThan`/`lessThanEqual`/
  `greaterThanEqual`, which the engine declares only over scalar primitive
  operands; a navExpr that is not one has no matching overload and the comparator
  is masked. Three navExpr shapes are not one, each live-attested against the
  pinned Legend stack (issue #116):
  - a **collection** — a navigation whose multiplicity has an upper bound other
    than exactly one (`Multiplicity::is_to_one`), at *any* step of the chain.
    `$c.fk1DefaultCountrylanguage` is `Countrylanguage[1..*]` →
    "Can't find a match for function `lessThan(Countrylanguage[1..*],Integer[1])`";
    and multiplicity **propagates**, so a primitive mapped over such a step is a
    collection too even where its own declared multiplicity is `[0..1]`
    (`$c.fk1DefaultCountrylanguage.percentage` → `lessThan(Float[*],Integer[1])`,
    `$x.fk3DefaultCarNames.model` → `lessThan(String[*],String[1])`).
  - a navigation off the `Class.all()` **extent**, which is itself a `T[*]`
    (`Country.all().gnp` → `lessThan(Float[*],Integer[1])`).
  - a **class-typed** navExpr, at any multiplicity — a class is no ordered
    operand even when the association end is `[1..1]`
    (`$c.fk1DefaultCountry` → `lessThan(Country[1],Integer[1])`).

  What the rule does **not** mask:
  - **Equality.** `==` and `!=` stay admissible at every one of those positions.
    Pure's `equal` is `Any[*]`-generic, and all three shapes compile with it live
    (`$c.fk1DefaultCountrylanguage == 'English'` → `Country`). Masking them would
    be a soundness violation, not a precision win.
  - **A `[0..1]` scalar.** An upper bound of exactly one is a scalar whatever the
    lower bound is, which is why `car_1`'s gold `$x.year < 1980` (an
    `Integer[0..1]` primitive, in the 269-query soundness-replay set) stays legal.
    The rule keys on `upper == 1`, never on `lower == 0`.
  - **Any continuation that is not an ordered comparator** — the collapse itself
    first among them. `->isEmpty()`, `->filter(…)`, `->count()`, `->exists(…)`,
    `->toOne()`, a further `.` hop, or the end of the term all pass through
    untouched; the mask clears exactly four tokens and nothing else. This is what
    keeps the corpus's own to-many navigations replayable — `world_1`
    `$c.fk1DefaultCountrylanguage->filter(l|$l.language == 'English')->isEmpty()`
    and `car_1` `$x.fk3DefaultCarNames->exists(c|…)`, both gold.

  **Where the evidence comes from (2026-08-29, #116).** The 8 committed
  `FIXTURE_DBS` carry 34 class-typed to-many navigations and 36 to-one ones:
  `Schema::build_navigable` (§6.2.3) makes each association end navigable from
  the class at the *other* end, so every `fk_n` association is navigable in both
  directions and the reverse direction of a many-to-one FK is a genuine
  `[1..*]` navigation. Across this repository's gold corpus and the wider
  pure-lingua query sets (12,600+ real queries) a to-many navigation occurs 43
  times and is **never** compared scalar-wise: it collapses via `->isEmpty`
  (20), `->filter` (13), `->count` (5) or `->exists` (2), or terminates as an
  `agg` map-lambda body (3). The one measured case of a comparison behind such a
  navigation is `$a.fk3DefaultAssetParts.partId->count() == 2` — legal only
  *after* the `->count()`.

  This supersedes the 2026-08-27 research note that recorded T6 as
  unimplementable against the committed fixtures. That note's blocking claim —
  "every FK association across all 8 fixtures is a strict many-to-one pattern"
  — was wrong: it read each association in one direction only.

- **T7 — projection/key lambda return-shape.** `colLambda`/`keyLambda` bodies must resolve to a **scalar** (`upper == 1`) primitive/enum value (a TDS column is scalar); a body left at a class or a to-many collection is masked. (Prevents `project([x|$x.fk0DefaultCountries], …)` — projecting a whole to-many navigation instead of one of its columns.)

### 6.7 Rule count

The scope state machine of §6.4 has **6 scope-transition rules** (S1/source,
S2/lambda-bind, S3/nav-advance, plus project/groupBy/agg/sort re-typing,
consolidated). The narrowing/type taxonomy has **14 rules**: **7 narrowing**
(N1–N7) and **7 type** (T1–T7). Scope transitions supply state to constraints;
they are not constraints themselves.

Two scope transitions additionally *constrain* the position they name rather
than only supplying state: **S1** narrows the identifier after a source
classpath's own `.` to exactly `all`, and **S2** narrows a `$<IDENT>` refVar to
the names the stream has bound (§6.5).

**S1's must-call veto.** `all` is a niladic *call* (`source = classpath
".all()"`), so once the name is whole the only legal continuation is its own
`(` — not another hop, not end-of-stream, and not whitespace (a space would
close the lexeme, drop S1 out of scope, and hand the escape back). Live-attested:
a bare `Class.all` is rejected with "Can't find property 'all' in class
'meta::pure::metamodel::type::Class'", exactly as `Db->tableToTDS` without its
`()` is.

**Mask-aware completion.** L1 acceptance is a lookahead fact — "would a
value-boundary byte from here reach a value-terminal state?" — and an identifier
has no self-terminating byte, so *every* partial name satisfies it: a stream
could stop mid-identifier (`Class.a`, live-rejected the same way) and call
itself complete. The overlay therefore also decides where a query may **end**:
the reserved EOS bit is cleared at a trie cursor that has reached only a strict
prefix of a legal name, after a whole name whose call is still owed, and at an
N7-constrained bare word — and `DecoderSession::is_complete` reads that same
verdict, so it and `allowed_mask`'s published EOS bit agree by construction.

The schema overlay constrains **N3** (source class/store, including its `::`
continuation, its class-vs-store continuation split, and the store-method set),
**N1/N2**
(property/navigation), **N5** (association direction through N1 member lookup),
**N6** (relation columns), **N7** (bare value-identifier continuation), the
numeric/string portion of **T1** (comparison
operand type), **T2** (ordered-comparator restriction, numeric/temporal only —
boolean/string/enum operands mask `< > <= >=` and keep `== !=`), and **T3**
(aggregation-reducer type — `sum`/`average` numeric-only; `min`/`max`/`count`
unconstrained). The other named categories are outside the supported overlay and
pass through without schema narrowing. `src/schema/narrow.rs` is authoritative
for the executable boundary.

---

## 7. The L1↔L2 consistency-contract table

L1 and L2 share a **single position vocabulary**: every place L2 narrows must be a specific, unambiguous grammar position L1 defines, and every L1 identifier/literal position that L2 references must exist in the grammar. The table below is the cross-check spine — L1 productions and L2 narrowing positions MUST stay in lockstep. A drift on either side is a bug.

| L2 rule (§6)                                           | L1 position (§5)                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                   |
| ------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| **N3** source-class narrowing                          | `source = classpath ".all()"` — the `classpath` before `.all()` (§5.2, §5.4); N3c additionally governs the position right after it, the byte-PDA's `InSourceIdent`, where `.` (class) and `->` (store) are the only continuations L1 offers; N3f governs the method-name `ident` the extent's own `->` opens, the byte-PDA's `AfterArrow`; N3g governs the value slot inside a receiver-only arrow call, and N4a the position right after a store method's call closes (`AfterValue`/`AfterName`, plus the `SawDash` of a split step arrow)                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                       |
| **N1** property / first-navigation                     | `navExpr = refVar { "." ident }` — the **first** `ident` after `$var .` (§5.3)                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                     |
| **N2** chained navigation                              | `navExpr` — each **subsequent** `ident` after a `.` (§5.3)                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                         |
| **N4** enum-value narrowing                            | No emitted L1 position; this category is outside the supported schema overlay. `SortDirection.ASC/DESC` in `sort` is a Pure builtin, not an N4 position.                                                                                                                                                                                                                                                                                                                                                                                                                                                           |
| **N5** association navigability direction              | same `ident` position as N1/N2 (L1 does not distinguish assoc from prop)                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                           |
| **N6** relation-column narrowing                       | the **reference** positions narrowed against a `RelationScope`: the `strlit` of `sort`/`asc`/`desc`, of `restrict`, of a later `project` name-reference, the `strlit` argument of `colAccess` (`$r.getInteger('col')`), **and** the trailing `ident` of the `->in(pipeline "." ident)` subquery-membership form (narrowed against the subquery pipeline's OWN terminal `RelationScope` — L2 enters each pipeline independently), §5.2–§5.3. (The `project`/`groupBy`/`olapGroupBy` name-lists _emit/define_ the column universe — they establish the scope, they are not themselves narrowed against a prior one.) |
| **N7** bare value-identifier continuation               | the `ident` a `valueExpr` may open with — the lambda `binderVar`, a value-position `classpath` segment, an enum-path head, and a function name — at every anchor that opens one (`ExpectValue`/`ExpectValueReq`, and the `\|`/`<`/`>`/`-` intermediate states an operator or lambda arrow leaves behind), §5.3                                                                                                                                                                                                                                                                                                              |
| **T1/T2** comparison operand type & ordered-comparator | `cmp = valueExpr cmpop valueExpr` — the `cmpop` + operand positions (§5.3)                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                         |
| **N4b/N4c** logical-operand and string-literal operator | the operand a `&&`/`\|\|` opens, and the operator position right after a completed `strlit` — both the `valueExpr … op … valueExpr` positions of §5.3, the second additionally read at the byte-PDA's pending-closing-quote state                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                         |
| **T3** aggregation-reducer type                        | `reduceExpr = refVar "->" reducer "()"` — the `reducer` position (§5.3)                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                            |
| **T4** string-predicate / string-transform type        | two L1 positions: `valueExpr "->" boolPred` for the predicates `contains`/`startsWith`/`endsWith`, **and** `valueExpr "->" fn` for the transforms `toLower`/`toUpper` (which are `fn`, not `boolPred`, in §5.3) (§5.3)                                                                                                                                                                                                                                                                                                                                                                                             |
| **T5** enum-comparison type                            | No emitted L1 position; this category is outside the supported schema overlay.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                     |
| **T6** multiplicity / collapse                         | the `cmpop` position right after a `navExpr` that is not a scalar primitive — a collection (upper bound ≠ 1 at any step), an extent navigation, or a class-typed step (§5.3). The collapse operators the rule leaves open (`->toOne()`, `exists`/`isEmpty`/`isNotEmpty`, the aggregates) are what turn such a `navExpr` back into a scalar.                                                                                                                                                                                                                                                                          |
| **T7** projection/key lambda return-shape              | the `valueExpr` body of `colLambda`/`keyLambda` must be scalar (§5.3)                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                              |

**Two contract points L1 explicitly provides for L2:**

1. **The relation-column access form.** `colAccess = refVar "." tdsGetter "(" strlit ")"` (e.g. `$r.getInteger('cnt')`) is the post-`groupBy`/`olapGroupBy` HAVING-style column read (`getInteger` alone 310, all four `getX` ≈ 340+ gold occurrences — matching §6.5 N6). Its `strlit` is an **N6 position** — it references a name emitted by the preceding `project`/`groupBy`, and L2 narrows it to the current `RelationScope(cols)`. Without this production L1 could not even reach the position, so the two levels would silently disagree.
2. **The `->toOne()` collapse operator.** `collapse` is the primary mechanism by which a `[0..1]` navigation becomes a `[1]` scalar so a scalar `cmp`/`fn`/arithmetic is legal (206 gold occurrences); it is one of the T6 collapse operators (alongside `exists` and the aggregates). L2's T6 references it by name.

---
