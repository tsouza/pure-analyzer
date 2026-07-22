//! The L2 `Schema` data-contract (`docs/spec/schema.md` §6.2): the minimal
//! per-database model a schema-aware decode consults to keep a partial query
//! referencing only real, correctly-typed model elements.
//!
//! The type is **pure data** — no I/O, no network, no Legend call. The host
//! populates it once at session init (§6.3) and hands it to
//! [`DecoderSession::with_schema`](crate::DecoderSession::with_schema). Ingress
//! is JSON ([`Schema::from_json`], §9); `serde` is the sole reason the published
//! core carries `serde`/`serde_json` (the `check-core-deplight` allowlist widen).

use std::collections::HashMap;

use serde::Deserialize;

/// A Pure primitive type name (`docs/spec/schema.md` §6.2.2). The variants that
/// share a [`TypeClass`] compare and aggregate alike.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub(crate) enum PrimName {
    /// `Integer`.
    Integer,
    /// `Float`.
    Float,
    /// `Decimal`.
    Decimal,
    /// `Number`.
    Number,
    /// `String`.
    String,
    /// `Boolean`.
    Boolean,
    /// `Date`.
    Date,
    /// `StrictDate`.
    StrictDate,
    /// `DateTime`.
    DateTime,
}

/// The comparison/operand type-classes primitives collapse into (§6.2.2). L2's
/// type rules (T1) narrow against these, not the raw primitive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum TypeClass {
    /// `Integer`/`Float`/`Decimal`/`Number` — number literals.
    Numeric,
    /// `String` — single-quoted literals.
    Str,
    /// `Boolean` — `true`/`false`.
    Boolean,
    /// `Date`/`StrictDate`/`DateTime` — date literals.
    Temporal,
}

impl PrimName {
    /// The [`TypeClass`] this primitive belongs to (§6.2.2).
    pub(crate) fn type_class(self) -> TypeClass {
        match self {
            PrimName::Integer | PrimName::Float | PrimName::Decimal | PrimName::Number => {
                TypeClass::Numeric
            }
            PrimName::String => TypeClass::Str,
            PrimName::Boolean => TypeClass::Boolean,
            PrimName::Date | PrimName::StrictDate | PrimName::DateTime => TypeClass::Temporal,
        }
    }
}

/// A property's declared type (§6.2.1): the three-way split that decides whether
/// a following `.` continues navigation (`Class`), terminates at a value
/// (`Primitive`), or narrows a comparison RHS to enum values (`Enum`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub(crate) enum PropType {
    /// One of the Pure primitives (§6.2.2).
    Primitive {
        /// The primitive name.
        name: PrimName,
    },
    /// A complex/class-typed property — navigation continues from `path`.
    Class {
        /// The fully-qualified class path the property navigates to.
        path: String,
    },
    /// An enumeration-typed property (reserved; no corpus enum, §6.5 N4).
    Enum {
        /// The fully-qualified enumeration path.
        path: String,
    },
}

/// A `[lower..upper]` multiplicity (§6.2.1). `upper == None` is `*` (unbounded).
///
/// M3 ships the identifier/type rules (N/T) that read a member's *type*, not its
/// multiplicity; the collapse rule T6 that consumes multiplicity is deferred, so
/// the bounds are carried on [`Resolved`] but not consumed by the current rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub(crate) struct Multiplicity {
    /// The lower bound.
    #[allow(dead_code)]
    pub(crate) lower: u32,
    /// The upper bound, or `None` for `*` (unbounded).
    #[allow(dead_code)]
    pub(crate) upper: Option<u32>,
}

/// A stored/regular property (§6.2.1).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct PropertySpec {
    /// The property name (the identifier the model emits).
    pub(crate) name: String,
    /// The declared type.
    #[serde(rename = "type")]
    pub(crate) ty: PropType,
    /// The declared multiplicity.
    pub(crate) mult: Multiplicity,
}

/// A derived (qualified) property (§6.2.1). For MVP identifier narrowing it is a
/// nav step yielding `return_type` — its argument positions are not narrowed.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct QualifiedPropertySpec {
    /// The qualified-property name.
    pub(crate) name: String,
    /// Its declared return type.
    pub(crate) return_type: PropType,
    /// Its declared return multiplicity.
    pub(crate) return_mult: Multiplicity,
}

/// A class definition (§6.2.1).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct ClassInfo {
    /// The class simple-name (the `.all()` head the model emits).
    pub(crate) simple_name: String,
    /// Stored properties, in declared order.
    pub(crate) properties: Vec<PropertySpec>,
    /// Derived (qualified) properties.
    #[serde(default)]
    pub(crate) qualified_properties: Vec<QualifiedPropertySpec>,
    /// Super-type class paths — members resolve transitively.
    #[serde(default)]
    pub(crate) super_types: Vec<String>,
}

/// One end of an association (§6.2.1).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct AssociationEnd {
    /// The navigable property name of this end.
    pub(crate) property_name: String,
    /// The class this end targets.
    pub(crate) target_class: String,
    /// The end's multiplicity.
    pub(crate) mult: Multiplicity,
}

/// An association: exactly two ends (§6.2.1, §6.2.3).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct AssociationSpec {
    /// The association path.
    #[allow(dead_code)]
    pub(crate) path: String,
    /// The two ends.
    pub(crate) ends: [AssociationEnd; 2],
}

/// A precomputed directed navigation step from a class (§6.2.3): navigating
/// `prop` reaches `target` with `mult`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NavStep {
    /// The navigable property name.
    pub(crate) prop: String,
    /// The class reached.
    pub(crate) target: String,
    /// The multiplicity of the navigation.
    pub(crate) mult: Multiplicity,
}

/// The result of resolving an identifier against a class (§6.4 S3): what a
/// following `.` may do and the operand type a comparison sees.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Resolved {
    /// A primitive-typed member — navigation terminates at this value.
    Primitive {
        /// The primitive name (its [`TypeClass`] feeds T1).
        prim: PrimName,
        /// The resolved multiplicity.
        mult: Multiplicity,
    },
    /// A class-typed member or association nav — navigation continues from `path`.
    Class {
        /// The class reached.
        path: String,
        /// The resolved multiplicity.
        mult: Multiplicity,
    },
    /// An enum-typed member (reserved; §6.5 N4).
    Enum {
        /// The enumeration path.
        #[allow(dead_code)]
        path: String,
        /// The resolved multiplicity.
        #[allow(dead_code)]
        mult: Multiplicity,
    },
}

/// The wire form deserialized from JSON before `navigable` is precomputed.
#[derive(Debug, Deserialize)]
struct SchemaData {
    db_id: String,
    db_path: String,
    classes: HashMap<String, ClassInfo>,
    #[serde(default)]
    associations: Vec<AssociationSpec>,
    #[serde(default)]
    enums: HashMap<String, Vec<String>>,
}

/// An error building a [`Schema`] from JSON.
#[derive(Debug, thiserror::Error)]
pub enum SchemaError {
    /// The JSON did not parse into the schema contract (§6.2).
    #[error("schema JSON is not a valid Schema contract: {0}")]
    Json(#[from] serde_json::Error),
}

/// The per-database L2 schema contract (`docs/spec/schema.md` §6.2).
///
/// Built host-side from JSON via [`from_json`](Schema::from_json) and handed to a
/// [`DecoderSession::with_schema`](crate::DecoderSession::with_schema). It is
/// pure data plus the precomputed `navigable` map (§6.2.3) — getting that
/// direction wrong would mask a real association property, so it is derived once
/// at construction and never by ad-hoc lookup.
#[derive(Debug, Clone)]
pub struct Schema {
    db_id: String,
    db_path: String,
    classes: HashMap<String, ClassInfo>,
    enums: HashMap<String, Vec<String>>,
    navigable: HashMap<String, Vec<NavStep>>,
}

impl Schema {
    /// Parse a [`Schema`] from its JSON contract (`docs/spec/schema.md` §6.3,
    /// §9), precomputing the association-navigability map (§6.2.3).
    ///
    /// # Errors
    /// Returns [`SchemaError::Json`] if `json` is not a well-formed schema
    /// contract.
    pub fn from_json(json: &str) -> Result<Self, SchemaError> {
        let data: SchemaData = serde_json::from_str(json)?;
        let navigable = Self::build_navigable(&data.associations);
        Ok(Self {
            db_id: data.db_id,
            db_path: data.db_path,
            classes: data.classes,
            enums: data.enums,
            navigable,
        })
    }

    /// The database id this schema describes.
    #[must_use]
    pub fn db_id(&self) -> &str {
        &self.db_id
    }

    /// Precompute the per-class navigable set (§6.2.3): each association end's
    /// property is navigable **from the class at the other end**.
    fn build_navigable(associations: &[AssociationSpec]) -> HashMap<String, Vec<NavStep>> {
        let mut navigable: HashMap<String, Vec<NavStep>> = HashMap::new();
        for assoc in associations {
            let [e0, e1] = &assoc.ends;
            navigable
                .entry(e0.target_class.clone())
                .or_default()
                .push(NavStep {
                    prop: e1.property_name.clone(),
                    target: e1.target_class.clone(),
                    mult: e1.mult,
                });
            navigable
                .entry(e1.target_class.clone())
                .or_default()
                .push(NavStep {
                    prop: e0.property_name.clone(),
                    target: e0.target_class.clone(),
                    mult: e0.mult,
                });
        }
        navigable
    }

    /// Whether `path` is a real class of this schema (§6.5 N3).
    pub(crate) fn has_class(&self, path: &str) -> bool {
        self.classes.contains_key(path)
    }

    /// Every legal pipeline **source** byte-string (§6.5 N3): each class path plus
    /// the store/`Db` path. The prefix-aware N3 narrower builds its completion
    /// trie from exactly this set (the `let` binder keyword is added by the
    /// narrower, being grammar rather than a schema source).
    pub(crate) fn source_paths(&self) -> impl Iterator<Item = &str> {
        self.classes
            .keys()
            .map(String::as_str)
            .chain(std::iter::once(self.db_path.as_str()))
    }

    /// The full member-name set of a class (§6.5 N1): its stored properties,
    /// qualified properties, and navigable ends, unioned transitively over its
    /// super-types. Nothing else may legally follow `$var.` when bound to it.
    pub(crate) fn member_names(&self, class: &str) -> Vec<String> {
        let mut names = Vec::new();
        let mut visited = Vec::new();
        self.collect_members(class, &mut names, &mut visited);
        names
    }

    fn collect_members(&self, class: &str, names: &mut Vec<String>, visited: &mut Vec<String>) {
        if visited.iter().any(|seen| seen == class) {
            return;
        }
        visited.push(class.to_owned());
        if let Some(info) = self.classes.get(class) {
            for prop in &info.properties {
                names.push(prop.name.clone());
            }
            for qprop in &info.qualified_properties {
                names.push(qprop.name.clone());
            }
            for parent in &info.super_types {
                self.collect_members(parent, names, visited);
            }
        }
        if let Some(steps) = self.navigable.get(class) {
            for step in steps {
                names.push(step.prop.clone());
            }
        }
    }

    /// Resolve `ident` as a member of `class` (§6.4 S3): a stored/qualified
    /// property, or an association navigation, transitively over super-types.
    /// `None` means the identifier is not a member (a phantom / wrong-direction
    /// nav — N1/N2/N5 mask it).
    pub(crate) fn resolve(&self, class: &str, ident: &str) -> Option<Resolved> {
        let mut visited = Vec::new();
        self.resolve_in(class, ident, &mut visited)
    }

    fn resolve_in(&self, class: &str, ident: &str, visited: &mut Vec<String>) -> Option<Resolved> {
        if visited.iter().any(|seen| seen == class) {
            return None;
        }
        visited.push(class.to_owned());
        if let Some(info) = self.classes.get(class) {
            for prop in &info.properties {
                if prop.name == ident {
                    return Some(Self::resolved_from(&prop.ty, prop.mult));
                }
            }
            for qprop in &info.qualified_properties {
                if qprop.name == ident {
                    return Some(Self::resolved_from(&qprop.return_type, qprop.return_mult));
                }
            }
            for parent in &info.super_types {
                if let Some(found) = self.resolve_in(parent, ident, visited) {
                    return Some(found);
                }
            }
        }
        if let Some(steps) = self.navigable.get(class) {
            for step in steps {
                if step.prop == ident {
                    return Some(Resolved::Class {
                        path: step.target.clone(),
                        mult: step.mult,
                    });
                }
            }
        }
        None
    }

    fn resolved_from(ty: &PropType, mult: Multiplicity) -> Resolved {
        match ty {
            PropType::Primitive { name } => Resolved::Primitive { prim: *name, mult },
            PropType::Class { path } => Resolved::Class {
                path: path.clone(),
                mult,
            },
            PropType::Enum { path } => Resolved::Enum {
                path: path.clone(),
                mult,
            },
        }
    }

    /// The declared values of enum `path`, if known (reserved; §6.5 N4).
    #[allow(dead_code)]
    pub(crate) fn enum_values(&self, path: &str) -> Option<&[String]> {
        self.enums.get(path).map(Vec::as_slice)
    }
}

#[cfg(test)]
mod tests {
    use super::{PrimName, Resolved, Schema, TypeClass};

    /// A small schema exercising every contract feature: a primitive property, a
    /// super-type inherited member, a qualified property, an enum, and a two-way
    /// association (`fk` navigable from both ends, §6.2.3).
    const SAMPLE: &str = r#"{
      "db_id": "d",
      "db_path": "spider::d::Db",
      "classes": {
        "A": { "simple_name": "A",
          "properties": [
            {"name": "n", "type": {"kind": "primitive", "name": "Integer"}, "mult": {"lower": 1, "upper": 1}},
            {"name": "label", "type": {"kind": "enum", "path": "E"}, "mult": {"lower": 0, "upper": 1}}
          ],
          "qualified_properties": [
            {"name": "doubled", "return_type": {"kind": "primitive", "name": "String"}, "return_mult": {"lower": 1, "upper": 1}}
          ],
          "super_types": ["Base"] },
        "Base": { "simple_name": "Base",
          "properties": [{"name": "inherited", "type": {"kind": "primitive", "name": "String"}, "mult": {"lower": 0, "upper": 1}}] },
        "B": { "simple_name": "B",
          "properties": [{"name": "m", "type": {"kind": "primitive", "name": "Float"}, "mult": {"lower": 1, "upper": 1}}] }
      },
      "associations": [
        {"path": "fk", "ends": [
          {"property_name": "toB", "target_class": "B", "mult": {"lower": 0, "upper": null}},
          {"property_name": "toA", "target_class": "A", "mult": {"lower": 1, "upper": 1}}
        ]}
      ],
      "enums": { "E": ["ONE", "TWO"] }
    }"#;

    fn sample() -> Schema {
        Schema::from_json(SAMPLE).expect("sample schema parses")
    }

    #[test]
    fn from_json_reports_db_id_and_rejects_garbage() {
        assert_eq!(sample().db_id(), "d");
        assert!(Schema::from_json("{ not json").is_err());
    }

    #[test]
    fn source_paths_are_the_classes_and_the_store_not_phantoms() {
        let s = sample();
        assert!(s.has_class("A") && s.has_class("Base"));
        assert!(!s.has_class("Nope"));
        // A source is a class OR the store path — but never a phantom. The N3
        // trie is built from exactly `source_paths()`.
        let sources: Vec<&str> = s.source_paths().collect();
        assert!(sources.contains(&"A"));
        assert!(sources.contains(&"spider::d::Db"));
        assert!(!sources.contains(&"Nope"));
        assert!(!sources.contains(&"spider::d::Other"));
    }

    #[test]
    fn member_names_union_props_qualified_navigable_and_super_types() {
        let mut members = sample().member_names("A");
        members.sort();
        // own props (n, label) + qualified (doubled) + super-type Base (inherited)
        // + navigable end from A (`toB`, §6.2.3).
        assert_eq!(members, ["doubled", "inherited", "label", "n", "toB"]);
    }

    #[test]
    fn navigable_is_two_way_with_the_opposite_end_direction() {
        let s = sample();
        // From A you navigate `toB`; from B you navigate `toA` — the opposite-end
        // rule (§6.2.3). Getting it backwards would mask a real association prop.
        assert!(s.member_names("A").contains(&"toB".to_owned()));
        assert!(s.member_names("B").contains(&"toA".to_owned()));
        assert!(!s.member_names("A").contains(&"toA".to_owned()));
    }

    #[test]
    fn resolve_classifies_primitive_class_and_phantom() {
        let s = sample();
        assert!(matches!(
            s.resolve("A", "n"),
            Some(Resolved::Primitive {
                prim: PrimName::Integer,
                ..
            })
        ));
        // A qualified property resolves to its return type.
        assert!(matches!(
            s.resolve("A", "doubled"),
            Some(Resolved::Primitive {
                prim: PrimName::String,
                ..
            })
        ));
        // An inherited (super-type) member resolves transitively.
        assert!(matches!(
            s.resolve("A", "inherited"),
            Some(Resolved::Primitive {
                prim: PrimName::String,
                ..
            })
        ));
        // An association nav resolves to the target class (navigation continues).
        assert!(matches!(
            s.resolve("A", "toB"),
            Some(Resolved::Class { path, .. }) if path == "B"
        ));
        assert!(matches!(
            s.resolve("A", "label"),
            Some(Resolved::Enum { .. })
        ));
        assert_eq!(s.resolve("A", "phantom"), None);
        assert_eq!(s.resolve("Nope", "n"), None);
    }

    #[test]
    fn type_class_collapses_primitives_per_section_6_2_2() {
        assert_eq!(PrimName::Integer.type_class(), TypeClass::Numeric);
        assert_eq!(PrimName::Float.type_class(), TypeClass::Numeric);
        assert_eq!(PrimName::Decimal.type_class(), TypeClass::Numeric);
        assert_eq!(PrimName::Number.type_class(), TypeClass::Numeric);
        assert_eq!(PrimName::String.type_class(), TypeClass::Str);
        assert_eq!(PrimName::Boolean.type_class(), TypeClass::Boolean);
        assert_eq!(PrimName::Date.type_class(), TypeClass::Temporal);
        assert_eq!(PrimName::StrictDate.type_class(), TypeClass::Temporal);
        assert_eq!(PrimName::DateTime.type_class(), TypeClass::Temporal);
    }

    #[test]
    fn enum_values_are_exposed_for_the_reserved_n4() {
        assert_eq!(
            sample().enum_values("E"),
            Some(["ONE".to_owned(), "TWO".to_owned()].as_slice())
        );
        assert_eq!(sample().enum_values("Missing"), None);
    }
}
