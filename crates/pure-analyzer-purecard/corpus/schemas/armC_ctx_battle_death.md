# Arm C translation context: battle_death

## Execution coordinates

- project_id: `spider-schema-bridge`
- workspace: `battle-death-1783544875`
- database_path: `spider::battle_death::Db`
- autogen_mapping_path (mapping for CLASS-anchored lambda execution): `spider::battle_death::model::DbMapping`
- class_runtime_path (runtime for CLASS-anchored lambda execution): `spider::battle_death::ClassRt`
- empty_mapping_path (store-level tableToTDS lambdas only): `spider::battle_death::EmptyMapping`
- store_runtime_path (store-level tableToTDS lambdas only): `spider::battle_death::Rt`
- classes: `spider::battle_death::model::default::Battle`, `spider::battle_death::model::default::Death`, `spider::battle_death::model::default::Ship`
- associations: `spider::battle_death::model::fk_0`, `spider::battle_death::model::fk_1`

## Pure model (autogen classes + associations — what the translator queries)

```pure
Class {meta::pure::profiles::doc.doc = 'Generated Element'} spider::battle_death::model::default::Battle
{
  id: Integer[1];
  name: String[0..1];
  date: String[0..1];
  bulgarianCommander: String[0..1];
  latinCommander: String[0..1];
  result: String[0..1];
}

Class {meta::pure::profiles::doc.doc = 'Generated Element'} spider::battle_death::model::default::Death
{
  causedByShipId: Integer[0..1];
  id: Integer[1];
  note: String[0..1];
  killed: Integer[0..1];
  injured: Integer[0..1];
}

Class {meta::pure::profiles::doc.doc = 'Generated Element'} spider::battle_death::model::default::Ship
{
  lostInBattle: Integer[0..1];
  id: Integer[1];
  name: String[0..1];
  tonnage: String[0..1];
  shipType: String[0..1];
  location: String[0..1];
  dispositionOfShip: String[0..1];
}

Association {meta::pure::profiles::doc.doc = 'Generated Element'} spider::battle_death::model::fk_0
{
  fk0DefaultShip: spider::battle_death::model::default::Ship[1..*];
  fk0DefaultBattle: spider::battle_death::model::default::Battle[1];
}

Association {meta::pure::profiles::doc.doc = 'Generated Element'} spider::battle_death::model::fk_1
{
  fk1DefaultDeath: spider::battle_death::model::default::Death[1..*];
  fk1DefaultShip: spider::battle_death::model::default::Ship[1];
}
```

## Glossary (question vocabulary => model identifiers)

```
Glossary for database 'battle_death'.
Maps question vocabulary (human-readable names) to model identifiers.

Table 'battle' => battle
  - column 'id' => battle.id
  - column 'name' => battle.name
  - column 'date' => battle.date
  - column 'bulgarian commander' => battle.bulgarian_commander
  - column 'latin commander' => battle.latin_commander
  - column 'result' => battle.result
Table 'ship' => ship
  - column 'lost in battle' => ship.lost_in_battle
  - column 'id' => ship.id
  - column 'name' => ship.name
  - column 'tonnage' => ship.tonnage
  - column 'ship type' => ship.ship_type
  - column 'location' => ship.location
  - column 'disposition of ship' => ship.disposition_of_ship
Table 'death' => death
  - column 'caused by ship id' => death.caused_by_ship_id
  - column 'id' => death.id
  - column 'note' => death.note
  - column 'killed' => death.killed
  - column 'injured' => death.injured
```
