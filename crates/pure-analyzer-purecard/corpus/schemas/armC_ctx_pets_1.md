# Arm C translation context: pets_1

## Execution coordinates

- project_id: `spider-schema-bridge`
- workspace: `pets-1-1783544755`
- database_path: `spider::pets_1::Db`
- autogen_mapping_path (mapping for CLASS-anchored lambda execution): `spider::pets_1::model::DbMapping`
- class_runtime_path (runtime for CLASS-anchored lambda execution): `spider::pets_1::ClassRt`
- empty_mapping_path (store-level tableToTDS lambdas only): `spider::pets_1::EmptyMapping`
- store_runtime_path (store-level tableToTDS lambdas only): `spider::pets_1::Rt`
- classes: `spider::pets_1::model::default::HasPet`, `spider::pets_1::model::default::Pets`, `spider::pets_1::model::default::Student`
- associations: `spider::pets_1::model::fk_0`, `spider::pets_1::model::fk_1`

## Pure model (autogen classes + associations — what the translator queries)

```pure
Class {meta::pure::profiles::doc.doc = 'Generated Element'} spider::pets_1::model::default::HasPet
{
  stuID: Integer[0..1];
  petID: Integer[0..1];
}

Class {meta::pure::profiles::doc.doc = 'Generated Element'} spider::pets_1::model::default::Pets
{
  petID: Integer[1];
  petType: String[0..1];
  petAge: Integer[0..1];
  weight: Float[0..1];
}

Class {meta::pure::profiles::doc.doc = 'Generated Element'} spider::pets_1::model::default::Student
{
  stuID: Integer[1];
  lName: String[0..1];
  fname: String[0..1];
  age: Integer[0..1];
  sex: String[0..1];
  major: Integer[0..1];
  advisor: Integer[0..1];
  cityCode: String[0..1];
}

Association {meta::pure::profiles::doc.doc = 'Generated Element'} spider::pets_1::model::fk_0
{
  fk0DefaultHasPet: spider::pets_1::model::default::HasPet[1..*];
  fk0DefaultStudent: spider::pets_1::model::default::Student[1];
}

Association {meta::pure::profiles::doc.doc = 'Generated Element'} spider::pets_1::model::fk_1
{
  fk1DefaultHasPet: spider::pets_1::model::default::HasPet[1..*];
  fk1DefaultPets: spider::pets_1::model::default::Pets[1];
}
```

## Glossary (question vocabulary => model identifiers)

```
Glossary for database 'pets_1'.
Maps question vocabulary (human-readable names) to model identifiers.

Table 'student' => Student
  - column 'student id' => Student.StuID
  - column 'last name' => Student.LName
  - column 'first name' => Student.Fname
  - column 'age' => Student.Age
  - column 'sex' => Student.Sex
  - column 'major' => Student.Major
  - column 'advisor' => Student.Advisor
  - column 'city code' => Student.city_code
Table 'has pet' => Has_Pet
  - column 'student id' => Has_Pet.StuID
  - column 'pet id' => Has_Pet.PetID
Table 'pets' => Pets
  - column 'pet id' => Pets.PetID
  - column 'pet type' => Pets.PetType
  - column 'pet age' => Pets.pet_age
  - column 'weight' => Pets.weight
```
