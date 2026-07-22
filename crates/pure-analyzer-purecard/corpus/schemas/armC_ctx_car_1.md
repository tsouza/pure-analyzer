# Arm C translation context: car_1

## Execution coordinates

- project_id: `spider-schema-bridge`
- workspace: `car-1-1783545045`
- database_path: `spider::car_1::Db`
- autogen_mapping_path (mapping for CLASS-anchored lambda execution): `spider::car_1::model::DbMapping`
- class_runtime_path (runtime for CLASS-anchored lambda execution): `spider::car_1::ClassRt`
- empty_mapping_path (store-level tableToTDS lambdas only): `spider::car_1::EmptyMapping`
- store_runtime_path (store-level tableToTDS lambdas only): `spider::car_1::Rt`
- classes: `spider::car_1::model::default::CarMakers`, `spider::car_1::model::default::CarNames`, `spider::car_1::model::default::CarsData`, `spider::car_1::model::default::Continents`, `spider::car_1::model::default::Countries`, `spider::car_1::model::default::ModelList`
- associations: `spider::car_1::model::fk_0`, `spider::car_1::model::fk_1`, `spider::car_1::model::fk_2`, `spider::car_1::model::fk_3`, `spider::car_1::model::fk_4`

## Pure model (autogen classes + associations — what the translator queries)

```pure
Class {meta::pure::profiles::doc.doc = 'Generated Element'} spider::car_1::model::default::CarMakers
{
  id: Integer[1];
  maker: String[0..1];
  fullName: String[0..1];
  country: String[0..1];
}

Class {meta::pure::profiles::doc.doc = 'Generated Element'} spider::car_1::model::default::CarNames
{
  makeId: Integer[1];
  model: String[0..1];
  make: String[0..1];
}

Class {meta::pure::profiles::doc.doc = 'Generated Element'} spider::car_1::model::default::CarsData
{
  id: Integer[1];
  mpg: String[0..1];
  cylinders: Integer[0..1];
  edispl: Float[0..1];
  horsepower: String[0..1];
  weight: Integer[0..1];
  accelerate: Float[0..1];
  year: Integer[0..1];
}

Class {meta::pure::profiles::doc.doc = 'Generated Element'} spider::car_1::model::default::Continents
{
  contId: Integer[1];
  continent: String[0..1];
}

Class {meta::pure::profiles::doc.doc = 'Generated Element'} spider::car_1::model::default::Countries
{
  countryId: Integer[1];
  countryName: String[0..1];
  continent: Integer[0..1];
}

Class {meta::pure::profiles::doc.doc = 'Generated Element'} spider::car_1::model::default::ModelList
{
  modelId: Integer[1];
  maker: Integer[0..1];
  model: String[0..1];
}

Association {meta::pure::profiles::doc.doc = 'Generated Element'} spider::car_1::model::fk_0
{
  fk0DefaultCountries: spider::car_1::model::default::Countries[1..*];
  fk0DefaultContinents: spider::car_1::model::default::Continents[1];
}

Association {meta::pure::profiles::doc.doc = 'Generated Element'} spider::car_1::model::fk_1
{
  fk1DefaultCarMakers: spider::car_1::model::default::CarMakers[1..*];
  fk1DefaultCountries: spider::car_1::model::default::Countries[1];
}

Association {meta::pure::profiles::doc.doc = 'Generated Element'} spider::car_1::model::fk_2
{
  fk2DefaultModelList: spider::car_1::model::default::ModelList[1..*];
  fk2DefaultCarMakers: spider::car_1::model::default::CarMakers[1];
}

Association {meta::pure::profiles::doc.doc = 'Generated Element'} spider::car_1::model::fk_3
{
  fk3DefaultCarNames: spider::car_1::model::default::CarNames[1..*];
  fk3DefaultModelList: spider::car_1::model::default::ModelList[1..*];
}

Association {meta::pure::profiles::doc.doc = 'Generated Element'} spider::car_1::model::fk_4
{
  fk4DefaultCarsData: spider::car_1::model::default::CarsData[1];
  fk4DefaultCarNames: spider::car_1::model::default::CarNames[1];
}
```

## Glossary (question vocabulary => model identifiers)

```
Glossary for database 'car_1'.
Maps question vocabulary (human-readable names) to model identifiers.

Table 'continents' => continents
  - column 'cont id' => continents.ContId
  - column 'continent' => continents.Continent
Table 'countries' => countries
  - column 'country id' => countries.CountryId
  - column 'country name' => countries.CountryName
  - column 'continent' => countries.Continent
Table 'car makers' => car_makers
  - column 'id' => car_makers.Id
  - column 'maker' => car_makers.Maker
  - column 'full name' => car_makers.FullName
  - column 'country' => car_makers.Country
Table 'model list' => model_list
  - column 'model id' => model_list.ModelId
  - column 'maker' => model_list.Maker
  - column 'model' => model_list.Model
Table 'car names' => car_names
  - column 'make id' => car_names.MakeId
  - column 'model' => car_names.Model
  - column 'make' => car_names.Make
Table 'cars data' => cars_data
  - column 'id' => cars_data.Id
  - column 'mpg' => cars_data.MPG
  - column 'cylinders' => cars_data.Cylinders
  - column 'edispl' => cars_data.Edispl
  - column 'horsepower' => cars_data.Horsepower
  - column 'weight' => cars_data.Weight
  - column 'accelerate' => cars_data.Accelerate
  - column 'year' => cars_data.Year
```
