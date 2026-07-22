# Arm C translation context: world_1

## Execution coordinates

- project_id: `spider-schema-bridge`
- workspace: `world-1-1783578508`
- database_path: `spider::world_1::Db`
- autogen_mapping_path (mapping for CLASS-anchored lambda execution): `spider::world_1::model::DbMapping`
- class_runtime_path (runtime for CLASS-anchored lambda execution): `spider::world_1::ClassRt`
- empty_mapping_path (store-level tableToTDS lambdas only): `spider::world_1::EmptyMapping`
- store_runtime_path (store-level tableToTDS lambdas only): `spider::world_1::Rt`
- classes: `spider::world_1::model::default::City`, `spider::world_1::model::default::Country`, `spider::world_1::model::default::Countrylanguage`
- associations: `spider::world_1::model::fk_0`, `spider::world_1::model::fk_1`

## Pure model (autogen classes + associations — what the translator queries)

```pure
Class {meta::pure::profiles::doc.doc = 'Generated Element'} spider::world_1::model::default::City
{
  id: Integer[1];
  name: String[0..1];
  countryCode: String[0..1];
  district: String[0..1];
  population: Integer[0..1];
}

Class {meta::pure::profiles::doc.doc = 'Generated Element'} spider::world_1::model::default::Country
{
  code: String[1];
  name: String[0..1];
  continent: String[0..1];
  region: String[0..1];
  surfaceArea: Float[0..1];
  indepYear: Integer[0..1];
  population: Integer[0..1];
  lifeExpectancy: Float[0..1];
  gnp: Float[0..1];
  gNPOld: Float[0..1];
  localName: String[0..1];
  governmentForm: String[0..1];
  headOfState: String[0..1];
  capital: Integer[0..1];
  code2: String[0..1];
}

Class {meta::pure::profiles::doc.doc = 'Generated Element'} spider::world_1::model::default::Countrylanguage
{
  countryCode: String[0..1];
  language: String[0..1];
  isOfficial: String[0..1];
  percentage: Float[0..1];
}

Association {meta::pure::profiles::doc.doc = 'Generated Element'} spider::world_1::model::fk_0
{
  fk0DefaultCity: spider::world_1::model::default::City[1..*];
  fk0DefaultCountry: spider::world_1::model::default::Country[1];
}

Association {meta::pure::profiles::doc.doc = 'Generated Element'} spider::world_1::model::fk_1
{
  fk1DefaultCountrylanguage: spider::world_1::model::default::Countrylanguage[1..*];
  fk1DefaultCountry: spider::world_1::model::default::Country[1];
}
```

## Glossary (question vocabulary => model identifiers)

```
Glossary for database 'world_1'.
Maps question vocabulary (human-readable names) to model identifiers.

Table 'city' => city
  - column 'id' => city.ID
  - column 'name' => city.Name
  - column 'country code' => city.CountryCode
  - column 'district' => city.District
  - column 'population' => city.Population
Table 'sqlite sequence' => sqlite_sequence
  - column 'name' => sqlite_sequence.name
  - column 'seq' => sqlite_sequence.seq
Table 'country' => country
  - column 'code' => country.Code
  - column 'name' => country.Name
  - column 'continent' => country.Continent
  - column 'region' => country.Region
  - column 'surface area' => country.SurfaceArea
  - column 'indepdent year' => country.IndepYear
  - column 'population' => country.Population
  - column 'life expectancy' => country.LifeExpectancy
  - column 'gnp' => country.GNP
  - column 'gnp old' => country.GNPOld
  - column 'local name' => country.LocalName
  - column 'government form' => country.GovernmentForm
  - column 'head of state' => country.HeadOfState
  - column 'capital' => country.Capital
  - column 'code2' => country.Code2
Table 'countrylanguage' => countrylanguage
  - column 'countrycode' => countrylanguage.CountryCode
  - column 'language' => countrylanguage.Language
  - column 'is official' => countrylanguage.IsOfficial
  - column 'percentage' => countrylanguage.Percentage
```

## PMCD for compile checks (fetch/assemble)

Two equivalent routes to the PureModelContextData a candidate lambda is compiled against:

1. **Fetch from the workspace (canonical for this probe):**
   `GET http://localhost:6100/api/projects/spider-schema-bridge/workspaces/world-1-1783578508/pureModelContextData`
   then drop every element with `_type == "sectionIndex"`. VERIFY presence of all required
   paths (database, DbMapping, ClassRt, Rt, EmptyMapping, Conn, classes, associations) --
   fs-SDLC silently drops elements its bundled protocol can't deserialize (gate0-findings).

2. **Assemble from grammar (what run_pilot does per instance):** concatenate the store
   grammar + connection grammar + autogen class/association/mapping grammar and parse via
   `POST http://localhost:6300/api/pure/v1/grammar/grammarToJson` (see `assemble_pmcd` in
   `src/pure_lingua/schema_bridge/model_bootstrap.py`; cached by grammar hash).

**Compile check:** `POST http://localhost:6300/api/pure/v1/compilation/lambdaReturnType` with body exactly
`{"lambda": <lambda json>, "model": <pmcd>}` -- no `clientVersion` field (the endpoint
rejects it outright). Success = HTTP 200 with a `returnType`; anything else is a compile
failure. Class-anchored lambdas execute with mapping `spider::world_1::model::DbMapping` + runtime `spider::world_1::ClassRt`.
