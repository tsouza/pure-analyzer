# Arm C translation context: concert_singer

## Execution coordinates

- project_id: `spider-schema-bridge`
- workspace: `concert-singer-1783544672`
- database_path: `spider::concert_singer::Db`
- autogen_mapping_path (mapping for CLASS-anchored lambda execution): `spider::concert_singer::model::DbMapping`
- class_runtime_path (runtime for CLASS-anchored lambda execution): `spider::concert_singer::ClassRt`
- empty_mapping_path (store-level tableToTDS lambdas only): `spider::concert_singer::EmptyMapping`
- store_runtime_path (store-level tableToTDS lambdas only): `spider::concert_singer::Rt`
- classes: `spider::concert_singer::model::default::Concert`, `spider::concert_singer::model::default::Singer`, `spider::concert_singer::model::default::SingerInConcert`, `spider::concert_singer::model::default::Stadium`
- associations: `spider::concert_singer::model::fk_0`, `spider::concert_singer::model::fk_1`, `spider::concert_singer::model::fk_2`

## Pure model (autogen classes + associations — what the translator queries)

```pure
Class {meta::pure::profiles::doc.doc = 'Generated Element'} spider::concert_singer::model::default::Concert
{
  concertId: Integer[1];
  concertName: String[0..1];
  theme: String[0..1];
  stadiumId: String[0..1];
  year: String[0..1];
}

Class {meta::pure::profiles::doc.doc = 'Generated Element'} spider::concert_singer::model::default::Singer
{
  singerId: Integer[1];
  name: String[0..1];
  country: String[0..1];
  songName: String[0..1];
  songReleaseYear: String[0..1];
  age: Integer[0..1];
  isMale: Boolean[0..1];
}

Class {meta::pure::profiles::doc.doc = 'Generated Element'} spider::concert_singer::model::default::SingerInConcert
{
  concertId: Integer[0..1];
  singerId: String[0..1];
}

Class {meta::pure::profiles::doc.doc = 'Generated Element'} spider::concert_singer::model::default::Stadium
{
  stadiumId: Integer[1];
  location: String[0..1];
  name: String[0..1];
  capacity: Integer[0..1];
  highest: Integer[0..1];
  lowest: Integer[0..1];
  average: Integer[0..1];
}

Association {meta::pure::profiles::doc.doc = 'Generated Element'} spider::concert_singer::model::fk_0
{
  fk0DefaultConcert: spider::concert_singer::model::default::Concert[1..*];
  fk0DefaultStadium: spider::concert_singer::model::default::Stadium[1];
}

Association {meta::pure::profiles::doc.doc = 'Generated Element'} spider::concert_singer::model::fk_1
{
  fk1DefaultSingerInConcert: spider::concert_singer::model::default::SingerInConcert[1..*];
  fk1DefaultSinger: spider::concert_singer::model::default::Singer[1];
}

Association {meta::pure::profiles::doc.doc = 'Generated Element'} spider::concert_singer::model::fk_2
{
  fk2DefaultSingerInConcert: spider::concert_singer::model::default::SingerInConcert[1..*];
  fk2DefaultConcert: spider::concert_singer::model::default::Concert[1];
}
```

## Glossary (question vocabulary => model identifiers)

```
Glossary for database 'concert_singer'.
Maps question vocabulary (human-readable names) to model identifiers.

Table 'stadium' => stadium
  - column 'stadium id' => stadium.Stadium_ID
  - column 'location' => stadium.Location
  - column 'name' => stadium.Name
  - column 'capacity' => stadium.Capacity
  - column 'highest' => stadium.Highest
  - column 'lowest' => stadium.Lowest
  - column 'average' => stadium.Average
Table 'singer' => singer
  - column 'singer id' => singer.Singer_ID
  - column 'name' => singer.Name
  - column 'country' => singer.Country
  - column 'song name' => singer.Song_Name
  - column 'song release year' => singer.Song_release_year
  - column 'age' => singer.Age
  - column 'is male' => singer.Is_male
Table 'concert' => concert
  - column 'concert id' => concert.concert_ID
  - column 'concert name' => concert.concert_Name
  - column 'theme' => concert.Theme
  - column 'stadium id' => concert.Stadium_ID
  - column 'year' => concert.Year
Table 'singer in concert' => singer_in_concert
  - column 'concert id' => singer_in_concert.concert_ID
  - column 'singer id' => singer_in_concert.Singer_ID
```
