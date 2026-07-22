# Arm C translation context: employee_hire_evaluation

## Execution coordinates

- project_id: `spider-schema-bridge`
- workspace: `employee-hire-evaluation-1783545167`
- database_path: `spider::employee_hire_evaluation::Db`
- autogen_mapping_path (mapping for CLASS-anchored lambda execution): `spider::employee_hire_evaluation::model::DbMapping`
- class_runtime_path (runtime for CLASS-anchored lambda execution): `spider::employee_hire_evaluation::ClassRt`
- empty_mapping_path (store-level tableToTDS lambdas only): `spider::employee_hire_evaluation::EmptyMapping`
- store_runtime_path (store-level tableToTDS lambdas only): `spider::employee_hire_evaluation::Rt`
- classes: `spider::employee_hire_evaluation::model::default::Employee`, `spider::employee_hire_evaluation::model::default::Evaluation`, `spider::employee_hire_evaluation::model::default::Hiring`, `spider::employee_hire_evaluation::model::default::Shop`
- associations: `spider::employee_hire_evaluation::model::fk_0`, `spider::employee_hire_evaluation::model::fk_1`, `spider::employee_hire_evaluation::model::fk_2`

## Pure model (autogen classes + associations — what the translator queries)

```pure
Class {meta::pure::profiles::doc.doc = 'Generated Element'} spider::employee_hire_evaluation::model::default::Employee
{
  employeeId: Integer[1];
  name: String[0..1];
  age: Integer[0..1];
  city: String[0..1];
}

Class {meta::pure::profiles::doc.doc = 'Generated Element'} spider::employee_hire_evaluation::model::default::Evaluation
{
  employeeId: String[0..1];
  yearAwarded: String[0..1];
  bonus: Float[0..1];
}

Class {meta::pure::profiles::doc.doc = 'Generated Element'} spider::employee_hire_evaluation::model::default::Hiring
{
  shopId: Integer[0..1];
  employeeId: Integer[1];
  startFrom: String[0..1];
  isFullTime: Boolean[0..1];
}

Class {meta::pure::profiles::doc.doc = 'Generated Element'} spider::employee_hire_evaluation::model::default::Shop
{
  shopId: Integer[1];
  name: String[0..1];
  location: String[0..1];
  district: String[0..1];
  numberProducts: Integer[0..1];
  managerName: String[0..1];
}

Association {meta::pure::profiles::doc.doc = 'Generated Element'} spider::employee_hire_evaluation::model::fk_0
{
  fk0DefaultHiring: spider::employee_hire_evaluation::model::default::Hiring[1];
  fk0DefaultEmployee: spider::employee_hire_evaluation::model::default::Employee[1];
}

Association {meta::pure::profiles::doc.doc = 'Generated Element'} spider::employee_hire_evaluation::model::fk_1
{
  fk1DefaultHiring: spider::employee_hire_evaluation::model::default::Hiring[1..*];
  fk1DefaultShop: spider::employee_hire_evaluation::model::default::Shop[1];
}

Association {meta::pure::profiles::doc.doc = 'Generated Element'} spider::employee_hire_evaluation::model::fk_2
{
  fk2DefaultEvaluation: spider::employee_hire_evaluation::model::default::Evaluation[1..*];
  fk2DefaultEmployee: spider::employee_hire_evaluation::model::default::Employee[1];
}
```

## Glossary (question vocabulary => model identifiers)

```
Glossary for database 'employee_hire_evaluation'.
Maps question vocabulary (human-readable names) to model identifiers.

Table 'employee' => employee
  - column 'employee id' => employee.Employee_ID
  - column 'name' => employee.Name
  - column 'age' => employee.Age
  - column 'city' => employee.City
Table 'shop' => shop
  - column 'shop id' => shop.Shop_ID
  - column 'name' => shop.Name
  - column 'location' => shop.Location
  - column 'district' => shop.District
  - column 'number products' => shop.Number_products
  - column 'manager name' => shop.Manager_name
Table 'hiring' => hiring
  - column 'shop id' => hiring.Shop_ID
  - column 'employee id' => hiring.Employee_ID
  - column 'start from' => hiring.Start_from
  - column 'is full time' => hiring.Is_full_time
Table 'evaluation' => evaluation
  - column 'employee id' => evaluation.Employee_ID
  - column 'year awarded' => evaluation.Year_awarded
  - column 'bonus' => evaluation.Bonus
```
