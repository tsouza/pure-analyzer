# Arm C translation context: dog_kennels

## Execution coordinates

- project_id: `spider-schema-bridge`
- workspace: `dog-kennels-1783578357`
- database_path: `spider::dog_kennels::Db`
- autogen_mapping_path (mapping for CLASS-anchored lambda execution): `spider::dog_kennels::model::DbMapping`
- class_runtime_path (runtime for CLASS-anchored lambda execution): `spider::dog_kennels::ClassRt`
- empty_mapping_path (store-level tableToTDS lambdas only): `spider::dog_kennels::EmptyMapping`
- store_runtime_path (store-level tableToTDS lambdas only): `spider::dog_kennels::Rt`
- classes: `spider::dog_kennels::model::default::Breeds`, `spider::dog_kennels::model::default::Charges`, `spider::dog_kennels::model::default::Dogs`, `spider::dog_kennels::model::default::Owners`, `spider::dog_kennels::model::default::Professionals`, `spider::dog_kennels::model::default::Sizes`, `spider::dog_kennels::model::default::TreatmentTypes`, `spider::dog_kennels::model::default::Treatments`
- associations: `spider::dog_kennels::model::fk_0`, `spider::dog_kennels::model::fk_1`, `spider::dog_kennels::model::fk_2`, `spider::dog_kennels::model::fk_3`, `spider::dog_kennels::model::fk_4`, `spider::dog_kennels::model::fk_5`, `spider::dog_kennels::model::fk_6`

## Pure model (autogen classes + associations — what the translator queries)

```pure
Class {meta::pure::profiles::doc.doc = 'Generated Element'} spider::dog_kennels::model::default::Breeds
{
  breedCode: String[1];
  breedName: String[0..1];
}

Class {meta::pure::profiles::doc.doc = 'Generated Element'} spider::dog_kennels::model::default::Charges
{
  chargeId: Integer[1];
  chargeType: String[0..1];
  chargeAmount: Float[0..1];
}

Class {meta::pure::profiles::doc.doc = 'Generated Element'} spider::dog_kennels::model::default::Dogs
{
  dogId: Integer[1];
  ownerId: Integer[0..1];
  abandonedYn: String[0..1];
  breedCode: String[0..1];
  sizeCode: String[0..1];
  name: String[0..1];
  age: String[0..1];
  dateOfBirth: DateTime[0..1];
  gender: String[0..1];
  weight: String[0..1];
  dateArrived: DateTime[0..1];
  dateAdopted: DateTime[0..1];
  dateDeparted: DateTime[0..1];
}

Class {meta::pure::profiles::doc.doc = 'Generated Element'} spider::dog_kennels::model::default::Owners
{
  ownerId: Integer[1];
  firstName: String[0..1];
  lastName: String[0..1];
  street: String[0..1];
  city: String[0..1];
  state: String[0..1];
  zipCode: String[0..1];
  emailAddress: String[0..1];
  homePhone: String[0..1];
  cellNumber: String[0..1];
}

Class {meta::pure::profiles::doc.doc = 'Generated Element'} spider::dog_kennels::model::default::Professionals
{
  professionalId: Integer[1];
  roleCode: String[0..1];
  firstName: String[0..1];
  street: String[0..1];
  city: String[0..1];
  state: String[0..1];
  zipCode: String[0..1];
  lastName: String[0..1];
  emailAddress: String[0..1];
  homePhone: String[0..1];
  cellNumber: String[0..1];
}

Class {meta::pure::profiles::doc.doc = 'Generated Element'} spider::dog_kennels::model::default::Sizes
{
  sizeCode: String[1];
  sizeDescription: String[0..1];
}

Class {meta::pure::profiles::doc.doc = 'Generated Element'} spider::dog_kennels::model::default::TreatmentTypes
{
  treatmentTypeCode: String[1];
  treatmentTypeDescription: String[0..1];
}

Class {meta::pure::profiles::doc.doc = 'Generated Element'} spider::dog_kennels::model::default::Treatments
{
  treatmentId: Integer[1];
  dogId: Integer[0..1];
  professionalId: Integer[0..1];
  treatmentTypeCode: String[0..1];
  dateOfTreatment: DateTime[0..1];
  costOfTreatment: Float[0..1];
}

Association {meta::pure::profiles::doc.doc = 'Generated Element'} spider::dog_kennels::model::fk_0
{
  fk0DefaultDogs: spider::dog_kennels::model::default::Dogs[1..*];
  fk0DefaultOwners: spider::dog_kennels::model::default::Owners[1];
}

Association {meta::pure::profiles::doc.doc = 'Generated Element'} spider::dog_kennels::model::fk_1
{
  fk1DefaultDogs: spider::dog_kennels::model::default::Dogs[1..*];
  fk1DefaultOwners: spider::dog_kennels::model::default::Owners[1];
}

Association {meta::pure::profiles::doc.doc = 'Generated Element'} spider::dog_kennels::model::fk_2
{
  fk2DefaultDogs: spider::dog_kennels::model::default::Dogs[1..*];
  fk2DefaultSizes: spider::dog_kennels::model::default::Sizes[1];
}

Association {meta::pure::profiles::doc.doc = 'Generated Element'} spider::dog_kennels::model::fk_3
{
  fk3DefaultDogs: spider::dog_kennels::model::default::Dogs[1..*];
  fk3DefaultBreeds: spider::dog_kennels::model::default::Breeds[1];
}

Association {meta::pure::profiles::doc.doc = 'Generated Element'} spider::dog_kennels::model::fk_4
{
  fk4DefaultTreatments: spider::dog_kennels::model::default::Treatments[1..*];
  fk4DefaultDogs: spider::dog_kennels::model::default::Dogs[1];
}

Association {meta::pure::profiles::doc.doc = 'Generated Element'} spider::dog_kennels::model::fk_5
{
  fk5DefaultTreatments: spider::dog_kennels::model::default::Treatments[1..*];
  fk5DefaultProfessionals: spider::dog_kennels::model::default::Professionals[1];
}

Association {meta::pure::profiles::doc.doc = 'Generated Element'} spider::dog_kennels::model::fk_6
{
  fk6DefaultTreatments: spider::dog_kennels::model::default::Treatments[1..*];
  fk6DefaultTreatmentTypes: spider::dog_kennels::model::default::TreatmentTypes[1];
}
```

## Glossary (question vocabulary => model identifiers)

```
Glossary for database 'dog_kennels'.
Maps question vocabulary (human-readable names) to model identifiers.

Table 'breeds' => Breeds
  - column 'breed code' => Breeds.breed_code
  - column 'breed name' => Breeds.breed_name
Table 'charges' => Charges
  - column 'charge id' => Charges.charge_id
  - column 'charge type' => Charges.charge_type
  - column 'charge amount' => Charges.charge_amount
Table 'sizes' => Sizes
  - column 'size code' => Sizes.size_code
  - column 'size description' => Sizes.size_description
Table 'treatment types' => Treatment_Types
  - column 'treatment type code' => Treatment_Types.treatment_type_code
  - column 'treatment type description' => Treatment_Types.treatment_type_description
Table 'owners' => Owners
  - column 'owner id' => Owners.owner_id
  - column 'first name' => Owners.first_name
  - column 'last name' => Owners.last_name
  - column 'street' => Owners.street
  - column 'city' => Owners.city
  - column 'state' => Owners.state
  - column 'zip code' => Owners.zip_code
  - column 'email address' => Owners.email_address
  - column 'home phone' => Owners.home_phone
  - column 'cell number' => Owners.cell_number
Table 'dogs' => Dogs
  - column 'dog id' => Dogs.dog_id
  - column 'owner id' => Dogs.owner_id
  - column 'abandoned yes or no' => Dogs.abandoned_yn
  - column 'breed code' => Dogs.breed_code
  - column 'size code' => Dogs.size_code
  - column 'name' => Dogs.name
  - column 'age' => Dogs.age
  - column 'date of birth' => Dogs.date_of_birth
  - column 'gender' => Dogs.gender
  - column 'weight' => Dogs.weight
  - column 'date arrived' => Dogs.date_arrived
  - column 'date adopted' => Dogs.date_adopted
  - column 'date departed' => Dogs.date_departed
Table 'professionals' => Professionals
  - column 'professional id' => Professionals.professional_id
  - column 'role code' => Professionals.role_code
  - column 'first name' => Professionals.first_name
  - column 'street' => Professionals.street
  - column 'city' => Professionals.city
  - column 'state' => Professionals.state
  - column 'zip code' => Professionals.zip_code
  - column 'last name' => Professionals.last_name
  - column 'email address' => Professionals.email_address
  - column 'home phone' => Professionals.home_phone
  - column 'cell number' => Professionals.cell_number
Table 'treatments' => Treatments
  - column 'treatment id' => Treatments.treatment_id
  - column 'dog id' => Treatments.dog_id
  - column 'professional id' => Treatments.professional_id
  - column 'treatment type code' => Treatments.treatment_type_code
  - column 'date of treatment' => Treatments.date_of_treatment
  - column 'cost of treatment' => Treatments.cost_of_treatment
```

## PMCD for compile checks (fetch/assemble)

Two equivalent routes to the PureModelContextData a candidate lambda is compiled against:

1. **Fetch from the workspace (canonical for this probe):**
   `GET http://localhost:6100/api/projects/spider-schema-bridge/workspaces/dog-kennels-1783578357/pureModelContextData`
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
failure. Class-anchored lambdas execute with mapping `spider::dog_kennels::model::DbMapping` + runtime `spider::dog_kennels::ClassRt`.
