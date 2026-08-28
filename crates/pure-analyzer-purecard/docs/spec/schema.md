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

- **N1 — property/first-navigation narrowing.** At `$var.<IDENT>` where `$var` is bound to `ClassScope(C)`: legal `<IDENT>` = `C.properties[*].name` ∪ `C.qualified_properties[*].name` ∪ `navigable(C)` (the opposite-end property names of every association touching `C`, per §6.2.3) ∪ the same three sets for every class in `C.super_types` (transitively). Nothing else.
- **N2 — chained-navigation narrowing.** After a ClassRef/association step advanced the scope to target class `T`, a further `.<IDENT>` narrows to `T`'s member set (N1 computed for `T`).
- **N3 — source-class narrowing.** At the pipeline-source `classpath`, the fully-qualified path must be a key of `Schema.classes` **or** the schema's `db_path` (the store, which arm-A relational queries name as their `tableReference` source). (Phantom-class prevention; catches `test::DoesNotExist.all()`.)
- **N4 — enum-value narrowing.** When a nav expression resolves to `EnumRef(E)` and is compared (`== / !=`), the RHS enum literal `E.value` (or `EnumPath.value` form) is narrowed to `Schema.enums[E]`. Nothing outside that enum's declared values. The emitted L1 grammar has no enum-literal operand position, so this rule is outside the supported overlay. (`SortDirection.ASC/DESC` is a Pure builtin inside `sort`, not a schema enum or an N4 position.)
- **N5 — association navigability direction.** A navigation property is legal from `C` only if it is the _opposite_ end of an association whose other end targets `C` (§6.2.3). This prevents emitting a navigation from the wrong side of the association.
- **N6 — relation-column narrowing.** In `RelationScope(cols)`, every reference to an emitted column name must be a member of `cols` (the names emitted by the preceding `project`/`groupBy`/`olapGroupBy`). Four reference positions occur in the corpus and are all narrowed: (a) a `sort('<COL>', …)` / `asc('<COL>')` / `desc('<COL>')` column string; (b) any `restrict([...])` or later `project` name-reference; (c) the **TDS-column accessor** `$r.get{Integer,Float,String,Boolean}('<COL>')` — the post-aggregate HAVING read (`->filter(r|$r.getInteger('cnt') >= 2)`), which is the single most common relation-column reference (340+ gold occurrences); and (d) the trailing column `<IDENT>` in the `->in(subquery.<IDENT>)` membership form (47 gold), narrowed against the **subquery pipeline's own terminal `RelationScope`** — the subquery is entered as an independent scope (§6.4), so its projected column universe, not the outer pipeline's, is the legal set. This keeps post-projection column references real. (Weaker than N1–N5: column names are string-literals, so this is enforced only where the model references a _previously emitted_ name; it is the relation-side analogue of property narrowing. The `getX` accessor additionally fixes the _type_ of the read — `getInteger` on a numeric column — which L2 MAY check against the aggregate's output type, but the compiler oracle also catches a `getString` on a numeric column, so this is an optional tightening.)

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
- **T6 — multiplicity / collapse rule.** A scalar comparison (`navExpr cmpop operand`), a scalar string/temporal `fn`, or scalar arithmetic requires the navExpr's resolved multiplicity to be **to-one** (`upper == 1`). A navigation whose resolved multiplicity is `[0..1]` or that crosses a to-many association end (e.g. from `Continents` via `fk0DefaultCountries` → `Countries[1..*]`) yields a _non-scalar_; using it scalar-wise is illegal — it must be **collapsed to `[1]` first**. The corpus-attested collapse operators are, in order of frequency: **`->toOne()`** (206 gold occurrences — the canonical `[0..1] → [1]` collapse, e.g. `$x.note->toOne()->contains('East')` and `$x.balance->toOne() + …`), an **aggregate** (`->sum()`/`->count()`/… inside `agg`), or an **existence predicate** (`->exists(lambda)` / `->isEmpty()` / `->isNotEmpty()`, which consume a to-many collection and return a scalar Boolean). L2 treats a `navExpr` immediately followed by any of these as scalar at the enclosing operator position. A scalar comparison applied to an _un-collapsed_ `[0..1]`/`[*]` navExpr is masked. (Optional-to-one `[0..1]` FK navigations DO occur in the pilot corpus and are collapsed with `->toOne()`; strictly-to-one `[1]` ends need no collapse.)
  **Implementation caveat (2026-08-27, #56 research):** the 206-occurrence
  `->toOne()` evidence and the `[0..1]` FK claim above are drawn from the
  broader pilot survey, not the 8 committed `FIXTURE_DBS` schemas L2 rules are
  actually implemented and soundness-verified against. Against those 8, this
  rule's literal reading — masking scalar `cmp` on **any** non-to-one navExpr —
  is **falsified** by the in-scope gold corpus itself: `car_1`'s
  `$x.year < 1980` is a direct, uncollapsed ordered comparison on a `[0..1]`
  **primitive** (`year: Integer`) and is part of the 269-query soundness-replay
  set (`l2_soundness.rs`), so masking it would be a soundness regression. Every
  `->toOne()` occurrence actually reachable in the 8 fixtures is on a
  **primitive** property (`horsepower`, `isFullTime`, …), never on a
  class-typed FK navigation — and every FK association across all 8 fixtures
  is a strict many-to-one pattern (`[1,1]` in the traversed direction; the
  reverse, to-many, end is never directly compared scalar-wise in the corpus).
  So neither reading is implementable-and-verifiable today: the
  any-multiplicity reading is directly falsified, and the
  class-typed-navigation-only reading (the one consistent with `year < 1980`
  staying legal) has zero exercisable positive-or-negative evidence in the
  current fixture set. Implementing either without a new fixture schema
  containing a genuinely optional/to-many-only FK association — or new
  corpus cases sourced from beyond the current 8 — would violate the
  corpus-evidence discipline (constitution §4: "do not invent constraints the
  corpus does not exercise").
- **T7 — projection/key lambda return-shape.** `colLambda`/`keyLambda` bodies must resolve to a **scalar** (`upper == 1`) primitive/enum value (a TDS column is scalar); a body left at a class or a to-many collection is masked. (Prevents `project([x|$x.fk0DefaultCountries], …)` — projecting a whole to-many navigation instead of one of its columns.)

### 6.7 Rule count

The scope state machine of §6.4 has **6 scope-transition rules** (S1/source,
S2/lambda-bind, S3/nav-advance, plus project/groupBy/agg/sort re-typing,
consolidated). The narrowing/type taxonomy has **13 rules**: **6 narrowing**
(N1–N6) and **7 type** (T1–T7). Scope transitions supply state to constraints;
they are not constraints themselves.

The schema overlay constrains **N3** (source class/store), **N1/N2**
(property/navigation), **N5** (association direction through N1 member lookup),
**N6** (relation columns), the numeric/string portion of **T1** (comparison
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
| **N3** source-class narrowing                          | `source = classpath ".all()"` — the `classpath` before `.all()` (§5.2, §5.4)                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                       |
| **N1** property / first-navigation                     | `navExpr = refVar { "." ident }` — the **first** `ident` after `$var .` (§5.3)                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                     |
| **N2** chained navigation                              | `navExpr` — each **subsequent** `ident` after a `.` (§5.3)                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                         |
| **N4** enum-value narrowing                            | No emitted L1 position; this category is outside the supported schema overlay. `SortDirection.ASC/DESC` in `sort` is a Pure builtin, not an N4 position.                                                                                                                                                                                                                                                                                                                                                                                                                                                           |
| **N5** association navigability direction              | same `ident` position as N1/N2 (L1 does not distinguish assoc from prop)                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                           |
| **N6** relation-column narrowing                       | the **reference** positions narrowed against a `RelationScope`: the `strlit` of `sort`/`asc`/`desc`, of `restrict`, of a later `project` name-reference, the `strlit` argument of `colAccess` (`$r.getInteger('col')`), **and** the trailing `ident` of the `->in(pipeline "." ident)` subquery-membership form (narrowed against the subquery pipeline's OWN terminal `RelationScope` — L2 enters each pipeline independently), §5.2–§5.3. (The `project`/`groupBy`/`olapGroupBy` name-lists _emit/define_ the column universe — they establish the scope, they are not themselves narrowed against a prior one.) |
| **T1/T2** comparison operand type & ordered-comparator | `cmp = valueExpr cmpop valueExpr` — the `cmpop` + operand positions (§5.3)                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                         |
| **T3** aggregation-reducer type                        | `reduceExpr = refVar "->" reducer "()"` — the `reducer` position (§5.3)                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                            |
| **T4** string-predicate / string-transform type        | two L1 positions: `valueExpr "->" boolPred` for the predicates `contains`/`startsWith`/`endsWith`, **and** `valueExpr "->" fn` for the transforms `toLower`/`toUpper` (which are `fn`, not `boolPred`, in §5.3) (§5.3)                                                                                                                                                                                                                                                                                                                                                                                             |
| **T5** enum-comparison type                            | No emitted L1 position; this category is outside the supported schema overlay.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                     |
| **T6** multiplicity / collapse                         | the `collapse` (`->toOne()`), `boolPred` `exists`/`isEmpty`/`isNotEmpty`, and `agg` positions that turn a `[0..1]`/`[*]` `navExpr` into a scalar before a scalar `cmp` (§5.3)                                                                                                                                                                                                                                                                                                                                                                                                                                      |
| **T7** projection/key lambda return-shape              | the `valueExpr` body of `colLambda`/`keyLambda` must be scalar (§5.3)                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                              |

**Two contract points L1 explicitly provides for L2:**

1. **The relation-column access form.** `colAccess = refVar "." tdsGetter "(" strlit ")"` (e.g. `$r.getInteger('cnt')`) is the post-`groupBy`/`olapGroupBy` HAVING-style column read (`getInteger` alone 310, all four `getX` ≈ 340+ gold occurrences — matching §6.5 N6). Its `strlit` is an **N6 position** — it references a name emitted by the preceding `project`/`groupBy`, and L2 narrows it to the current `RelationScope(cols)`. Without this production L1 could not even reach the position, so the two levels would silently disagree.
2. **The `->toOne()` collapse operator.** `collapse` is the primary mechanism by which a `[0..1]` navigation becomes a `[1]` scalar so a scalar `cmp`/`fn`/arithmetic is legal (206 gold occurrences); it is one of the T6 collapse operators (alongside `exists` and the aggregates). L2's T6 references it by name.

---
