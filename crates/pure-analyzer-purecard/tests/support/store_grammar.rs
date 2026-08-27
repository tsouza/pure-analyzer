//! Derives the Database/Mapping/Connection/Runtime Pure grammar a fixture
//! DB's arm-A store-level queries need to compile — the gap issue #55's PR #84
//! found: each `corpus/schemas/*.md` context file's committed Pure model text
//! carries only the Class/Association grammar (`###Pure`), not the
//! `###Relational`/`###Mapping`/`###Connection`/`###Runtime` blocks arm-A's
//! `Db->tableReference(...)->tableToTDS()` shape (and a class-anchored
//! `Class.all()`'s own execution coordinates) also need.
//!
//! Design decision (fold vs. fetch): the `## PMCD for compile checks` section
//! of every context file offers two "equivalent routes" — fetch the assembled
//! PMCD from a live SDLC workspace, or assemble it from grammar text via
//! `grammarToJson`. The first is not actually available here: the referenced
//! workspaces (e.g. `world-1-1783578508`) belong to the external system that
//! authored these context files, not to this repo's own Legend stack (its
//! SDLC container has no workspaces at all — confirmed live against
//! `corpus/legend-stack/docker-compose.yml`'s bundled instance). The second
//! route needs nothing this repo doesn't already have, matching the precedent
//! `live_legend_schema_walk_compile.rs::pure_model_text` already set for the
//! Class/Association grammar: derive it once from the same committed schema
//! JSON `tests/fixtures/schemas/*.json` already parses (`Schema::from_json`'s
//! own input), so this stays hermetic and CI-reproducible, no live-workspace
//! dependency.
//!
//! This is a compile-only proof (issue #55's own scope: "100% compile rate,"
//! not execution or faithfulness — that is #56/#58's separate, model-in-the-
//! loop concern), so the generated `RelationalDatabaseConnection` seeds no
//! data (`testDataSetupSqls: []`): `lambdaReturnType` type-checks a lambda
//! against a `PureModelContextData`'s structure, it does not execute it.
//! Column/table naming only needs to be internally self-consistent — the
//! `Database`'s own tables/columns, the `Mapping`'s references to them, and
//! the pre-existing `Class`/`Association` grammar's property names — not a
//! faithful reproduction of the original Spider database's real DDL, since no
//! query here ever touches real rows.
#![cfg(feature = "legend")]

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde_json::Value;

/// Parse `db_id`'s committed schema fixture as raw JSON (not through
/// `purecard::Schema`, whose public surface does not expose per-class
/// property/association iteration to `pub(crate)` test code — going straight
/// to the JSON `Schema::from_json` itself parses keeps this self-contained).
fn schema_json(db_id: &str) -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/schemas")
        .join(format!("{db_id}.json"));
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("read schema fixture {}: {err}", path.display()));
    serde_json::from_str(&text)
        .unwrap_or_else(|err| panic!("parse schema fixture {}: {err}", path.display()))
}

/// The Legend Relational column type for a schema primitive — verified live
/// against the engine's `grammarToJson/model` (`BOOLEAN` is rejected with
/// `Unsupported column data type 'BOOLEAN'`; `BIT` is the accepted spelling).
fn sql_type(primitive: &str) -> &'static str {
    match primitive {
        "Integer" => "INTEGER",
        "Float" => "FLOAT",
        "String" => "VARCHAR(4000)",
        "Boolean" => "BIT",
        "DateTime" | "Date" | "StrictDate" => "TIMESTAMP",
        other => panic!("store_grammar: unmapped primitive type {other:?}"),
    }
}

/// The Relational-mapping class identifier Legend's `[Class[id]]` bracket
/// syntax uses — the fully-qualified class path with `::` collapsed to `_`,
/// matching the convention real generated Legend domains use (e.g.
/// `retail::calendar::model::default::Store` → `retail_calendar_model_default_Store`).
fn mapping_id(class_path: &str) -> String {
    class_path.replace("::", "_")
}

/// The property, of `properties`, chosen as a class's Relational primary key:
/// the first single-valued (`mult.upper == 1`) property, falling back to the
/// first property outright. Every fixture class has at least one `mult.upper
/// == 1` property (verified across all 8 `FIXTURE_DBS` schemas), so the
/// fallback is defensive, not exercised.
fn primary_key_name(properties: &[Value]) -> &str {
    properties
        .iter()
        .find(|p| p["mult"]["upper"] == 1)
        .or_else(|| properties.first())
        .and_then(|p| p["name"].as_str())
        .expect("every class has at least one property")
}

/// One association's derived shape: which end is the FK-holding "many" side,
/// which is the referenced "one" side, and the short join/property-bracket
/// name (the association path's last `::`-segment, e.g. `fk_0`).
struct AssocShape<'a> {
    short: &'a str,
    many_class: &'a str,
    many_property: &'a str,
    one_class: &'a str,
    one_property: &'a str,
}

/// Split an association JSON value into its many/one ends: the end with
/// `mult.upper: 1` is "one" (the FK's referenced side), the other is "many"
/// (the FK column's own side) — the common auto-generated FK shape. Two rarer
/// shapes exist in the committed fixtures (car_1's `fk_4`, one-to-one, both
/// ends `upper: 1`; car_1's `fk_3`, many-to-many, neither end `upper: 1`;
/// employee_hire_evaluation's `fk_0`, one-to-one): for those, `position`
/// falls back to the first end as "one" — an arbitrary but deterministic
/// direction choice. This is a compile-only proof (`store_grammar.rs`'s own
/// module doc comment): no query here ever executes against real data, so a
/// many-to-many relationship modeled as a simple FK (losing its true
/// multiplicity) is a documented simplification, not a soundness bug — the
/// alternative (a real link/join table with chained joins) buys no proof
/// value neither #55 nor #59 needs.
fn assoc_shape(assoc: &Value) -> AssocShape<'_> {
    let short = assoc["path"]
        .as_str()
        .and_then(|p| p.rsplit("::").next())
        .expect("association has a path");
    let ends = assoc["ends"].as_array().expect("association has ends");
    assert_eq!(
        ends.len(),
        2,
        "association {short} does not have exactly two ends"
    );
    let one_index = ends
        .iter()
        .position(|e| e["mult"]["upper"] == 1)
        .unwrap_or(0);
    let one = &ends[one_index];
    let many = &ends[1 - one_index];
    AssocShape {
        short,
        many_class: many["target_class"].as_str().expect("end has target_class"),
        many_property: many["property_name"]
            .as_str()
            .expect("end has property_name"),
        one_class: one["target_class"].as_str().expect("end has target_class"),
        one_property: one["property_name"]
            .as_str()
            .expect("end has property_name"),
    }
}

/// The synthetic FK column name an association contributes to its many-side
/// table — never a real property name, so it can never collide with one.
fn fk_column_name(short: &str, one_simple: &str) -> String {
    format!("{short}_{one_simple}Ref")
}

/// The `###Relational`/`###Mapping`/`###Connection`/`###Runtime` grammar text
/// for `db_id`, to concatenate after its existing `###Pure` Class/Association
/// model text before calling `grammarToJson/model`. Deterministic (classes and
/// associations are visited in their committed JSON order, which `serde_json`
/// preserves for objects and arrays).
pub(crate) fn store_grammar_text(db_id: &str) -> String {
    store_grammar_from_schema(&schema_json(db_id))
}

/// The pure core of [`store_grammar_text`], taking the already-parsed schema
/// JSON directly — split out so unit tests can exercise it against a small
/// synthetic schema without a fixture file on disk.
fn store_grammar_from_schema(schema: &Value) -> String {
    let db_path = schema["db_path"].as_str().expect("schema has db_path");
    let ns = db_path
        .strip_suffix("::Db")
        .unwrap_or_else(|| panic!("db_path {db_path:?} does not end in ::Db"));
    let classes = schema["classes"].as_object().expect("schema has classes");
    let associations = schema["associations"]
        .as_array()
        .expect("schema has associations");

    let simple_name_of = |class_path: &str| -> &str {
        classes[class_path]["simple_name"]
            .as_str()
            .unwrap_or_else(|| panic!("class {class_path} has no simple_name"))
    };
    let primary_key_of = |class_path: &str| -> &str {
        let props = classes[class_path]["properties"]
            .as_array()
            .unwrap_or_else(|| panic!("class {class_path} has no properties"));
        primary_key_name(props)
    };

    // Pass 1: derive each association's FK column and collect it against its
    // many-side class, plus the join and mapping-bracket lines it contributes.
    let mut extra_columns: BTreeMap<&str, Vec<(String, &'static str)>> = BTreeMap::new();
    let mut joins = String::new();
    let mut assoc_mapping_blocks = String::new();
    for assoc in associations {
        let shape = assoc_shape(assoc);
        let many_simple = simple_name_of(shape.many_class);
        let one_simple = simple_name_of(shape.one_class);
        let fk_col = fk_column_name(shape.short, one_simple);
        let one_pk = primary_key_of(shape.one_class);
        extra_columns
            .entry(shape.many_class)
            .or_default()
            .push((fk_col.clone(), "INTEGER"));
        joins.push_str(&format!(
            "  Join {}({many_simple}.{fk_col} = {one_simple}.{one_pk})\n",
            shape.short
        ));
        let many_id = mapping_id(shape.many_class);
        let one_id = mapping_id(shape.one_class);
        assoc_mapping_blocks.push_str(&format!(
            "  {}: Relational\n  {{\n    AssociationMapping\n    (\n      {}[{one_id},{many_id}]: [{ns}::Db]@{},\n      {}[{many_id},{one_id}]: [{ns}::Db]@{}\n    )\n  }}\n",
            assoc["path"].as_str().expect("association has a path"),
            shape.many_property,
            shape.short,
            shape.one_property,
            shape.short,
        ));
    }

    // Pass 2: emit one Table (with any FK columns pass 1 collected) and one
    // Relational class-mapping block per class, in committed schema order.
    let mut tables = String::new();
    let mut class_mapping_blocks = String::new();
    for (class_path, class) in classes {
        let simple = class["simple_name"]
            .as_str()
            .unwrap_or_else(|| panic!("class {class_path} has no simple_name"));
        let properties = class["properties"]
            .as_array()
            .unwrap_or_else(|| panic!("class {class_path} has no properties"));
        let pk = primary_key_name(properties);

        let mut columns: Vec<(String, &'static str, bool)> = properties
            .iter()
            .map(|p| {
                let name = p["name"].as_str().expect("property has a name").to_owned();
                let prim = p["type"]["name"]
                    .as_str()
                    .expect("property has a type name");
                let is_pk = name == pk;
                (name, sql_type(prim), is_pk)
            })
            .collect();
        for (col, sql) in extra_columns.get(class_path.as_str()).into_iter().flatten() {
            columns.push((col.clone(), sql, false));
        }

        tables.push_str(&format!("  Table {simple}\n  (\n"));
        for (index, (name, sql, is_pk)) in columns.iter().enumerate() {
            let pk_suffix = if *is_pk { " PRIMARY KEY" } else { "" };
            let comma = if index + 1 < columns.len() { "," } else { "" };
            tables.push_str(&format!("    {name} {sql}{pk_suffix}{comma}\n"));
        }
        tables.push_str("  )\n");

        let id = mapping_id(class_path);
        class_mapping_blocks.push_str(&format!(
            "  *{class_path}[{id}]: Relational\n  {{\n    ~primaryKey\n    (\n      [{ns}::Db]{simple}.{pk}\n    )\n    ~mainTable [{ns}::Db]{simple}\n"
        ));
        for (index, p) in properties.iter().enumerate() {
            let name = p["name"].as_str().expect("property has a name");
            let comma = if index + 1 < properties.len() {
                ","
            } else {
                ""
            };
            class_mapping_blocks
                .push_str(&format!("    {name}: [{ns}::Db]{simple}.{name}{comma}\n"));
        }
        class_mapping_blocks.push_str("  }\n");
    }

    format!(
        "###Relational\nDatabase {ns}::Db\n(\n{tables}\n{joins})\n\n\
         ###Mapping\nMapping {ns}::model::DbMapping\n(\n{class_mapping_blocks}\n{assoc_mapping_blocks})\n\n\
         Mapping {ns}::EmptyMapping\n(\n)\n\n\
         ###Connection\nRelationalDatabaseConnection {ns}::Conn\n{{\n  store: {ns}::Db;\n  type: H2;\n  specification: LocalH2 {{ testDataSetupSqls: []; }};\n  auth: DefaultH2;\n}}\n\n\
         ###Runtime\nRuntime {ns}::ClassRt\n{{\n  mappings: [ {ns}::model::DbMapping ];\n  connections: [\n    {ns}::Db: [ c: {ns}::Conn ]\n  ];\n}}\n\
         Runtime {ns}::Rt\n{{\n  mappings: [ {ns}::EmptyMapping ];\n  connections: [\n    {ns}::Db: [ c: {ns}::Conn ]\n  ];\n}}\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Value {
        serde_json::from_str(text).expect("valid test json")
    }

    #[test]
    fn sql_type_maps_every_fixture_primitive() {
        assert_eq!(sql_type("Integer"), "INTEGER");
        assert_eq!(sql_type("Float"), "FLOAT");
        assert_eq!(sql_type("String"), "VARCHAR(4000)");
        assert_eq!(sql_type("Boolean"), "BIT");
        assert_eq!(sql_type("DateTime"), "TIMESTAMP");
    }

    #[test]
    #[should_panic(expected = "unmapped primitive type")]
    fn sql_type_panics_on_an_unmapped_primitive() {
        sql_type("Currency");
    }

    #[test]
    fn mapping_id_collapses_namespace_separators() {
        assert_eq!(
            mapping_id("spider::world_1::model::default::City"),
            "spider_world_1_model_default_City"
        );
    }

    #[test]
    fn fk_column_name_is_never_a_real_property_shape() {
        assert_eq!(fk_column_name("fk_0", "Country"), "fk_0_CountryRef");
    }

    /// Build a two-end association JSON value; `upper_a`/`upper_b` are raw
    /// JSON literal text (`"1"` or `"null"`) for each end's `mult.upper`.
    fn assoc(short: &str, class_a: &str, upper_a: &str, class_b: &str, upper_b: &str) -> Value {
        parse(&format!(
            r#"{{
                "path": "spider::x::model::{short}",
                "ends": [
                    {{"property_name": "a", "target_class": "{class_a}", "mult": {{"lower": 1, "upper": {upper_a}}}}},
                    {{"property_name": "b", "target_class": "{class_b}", "mult": {{"lower": 1, "upper": {upper_b}}}}}
                ]
            }}"#
        ))
    }

    #[test]
    fn assoc_shape_picks_the_single_multiplicity_end_as_one() {
        let value = assoc("fk_0", "City", "null", "Country", "1");
        let shape = assoc_shape(&value);
        assert_eq!(shape.short, "fk_0");
        assert_eq!(shape.many_class, "City");
        assert_eq!(shape.one_class, "Country");
    }

    #[test]
    fn assoc_shape_order_is_independent_of_which_end_is_listed_first() {
        let value = assoc("fk_0", "Country", "1", "City", "null");
        let shape = assoc_shape(&value);
        assert_eq!(shape.many_class, "City");
        assert_eq!(shape.one_class, "Country");
    }

    #[test]
    fn assoc_shape_falls_back_to_the_first_end_as_one_for_a_one_to_one_association() {
        // car_1's fk_4 and employee_hire_evaluation's fk_0: both ends are
        // `mult.upper: 1`, so no end uniquely identifies the FK's referenced
        // side. The fallback (first end is "one") is an arbitrary but
        // deterministic direction choice, documented on `assoc_shape` — this
        // pins that it never panics and picks a consistent direction.
        let value = assoc("fk_4", "CarsData", "1", "CarNames", "1");
        let shape = assoc_shape(&value);
        assert_eq!(shape.one_class, "CarsData");
        assert_eq!(shape.many_class, "CarNames");
    }

    #[test]
    fn assoc_shape_falls_back_to_the_first_end_as_one_for_a_many_to_many_association() {
        // car_1's fk_3: neither end is `mult.upper: 1`. Same fallback as the
        // one-to-one case — pins it never panics and picks a consistent
        // direction, at the documented cost of losing the true multiplicity.
        let value = assoc("fk_3", "CarNames", "null", "ModelList", "null");
        let shape = assoc_shape(&value);
        assert_eq!(shape.one_class, "CarNames");
        assert_eq!(shape.many_class, "ModelList");
    }

    const TINY_SCHEMA: &str = r#"{
        "db_id": "d",
        "db_path": "test::d::Db",
        "classes": {
            "test::d::model::default::A": {
                "simple_name": "A",
                "properties": [
                    {"name": "id", "type": {"kind": "primitive", "name": "Integer"}, "mult": {"lower": 1, "upper": 1}},
                    {"name": "name", "type": {"kind": "primitive", "name": "String"}, "mult": {"lower": 0, "upper": 1}}
                ]
            },
            "test::d::model::default::B": {
                "simple_name": "B",
                "properties": [
                    {"name": "id", "type": {"kind": "primitive", "name": "Integer"}, "mult": {"lower": 1, "upper": 1}}
                ]
            }
        },
        "associations": [
            {
                "path": "test::d::model::fk_0",
                "ends": [
                    {"property_name": "fk0DefaultB", "target_class": "test::d::model::default::B", "mult": {"lower": 1, "upper": null}},
                    {"property_name": "fk0DefaultA", "target_class": "test::d::model::default::A", "mult": {"lower": 1, "upper": 1}}
                ]
            }
        ]
    }"#;

    fn tiny_schema() -> Value {
        parse(TINY_SCHEMA)
    }

    #[test]
    fn store_grammar_from_schema_emits_the_database_and_join() {
        let text = store_grammar_from_schema(&tiny_schema());
        assert!(text.contains("###Relational\nDatabase test::d::Db"));
        assert!(text.contains("Table A"));
        assert!(text.contains("Table B"));
        // A's own properties, plus fk_0's synthetic FK column landing on B
        // (the many side, referencing A's primary key `id`).
        assert!(text.contains("id INTEGER PRIMARY KEY"));
        assert!(text.contains("name VARCHAR(4000)"));
        assert!(text.contains("fk_0_ARef INTEGER"));
        assert!(text.contains("Join fk_0(B.fk_0_ARef = A.id)"));
    }

    #[test]
    fn store_grammar_from_schema_emits_the_class_and_association_mapping() {
        let text = store_grammar_from_schema(&tiny_schema());
        assert!(text.contains("###Mapping\nMapping test::d::model::DbMapping"));
        assert!(text.contains("*test::d::model::default::A[test_d_model_default_A]: Relational"));
        assert!(text.contains("~mainTable [test::d::Db]A"));
        assert!(text.contains(
            "fk0DefaultB[test_d_model_default_A,test_d_model_default_B]: [test::d::Db]@fk_0"
        ));
        assert!(text.contains(
            "fk0DefaultA[test_d_model_default_B,test_d_model_default_A]: [test::d::Db]@fk_0"
        ));
        assert!(text.contains("Mapping test::d::EmptyMapping\n(\n)"));
    }

    #[test]
    fn store_grammar_from_schema_emits_the_connection_and_runtimes() {
        let text = store_grammar_from_schema(&tiny_schema());
        assert!(text.contains("###Connection\nRelationalDatabaseConnection test::d::Conn"));
        assert!(text.contains("store: test::d::Db;"));
        assert!(text.contains("###Runtime\nRuntime test::d::ClassRt"));
        assert!(text.contains("mappings: [ test::d::model::DbMapping ];"));
        assert!(text.contains("Runtime test::d::Rt"));
        assert!(text.contains("mappings: [ test::d::EmptyMapping ];"));
    }

    #[test]
    fn store_grammar_from_schema_is_deterministic() {
        let schema = tiny_schema();
        assert_eq!(
            store_grammar_from_schema(&schema),
            store_grammar_from_schema(&schema)
        );
    }
}
