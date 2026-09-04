//! A weighted-shape `proptest` `Strategy<Value = String>` generating diverse,
//! real M3 query source text over a fixed multi-class model (issue #245) —
//! the relational-IR-level counterpart to `tests/support/lexeme_strategy.rs`
//! (issue #299, `pure-analyzer-parser`) for canonical emission's proof suite.
//!
//! This deliberately renders real Pure *source text*, not hand-built
//! `RelationExpression`/`ScalarExpression` trees: constructing valid IR by
//! hand would have to independently reproduce every invariant
//! `RelationExpression::new` and the normalizer's own preconditions check
//! (unique `ColumnId`s, schema/projection shape agreement, fact provenance).
//! Going through the real parser → lowerer → normalizer gets those for free
//! and exercises the exact pipeline this proof suite is about — so a
//! generated case reaching "supported and emitted" is a genuine, load-bearing
//! signal, not a generator artifact.
//!
//! Coverage is a broad but explicitly bounded foundation, not the full
//! supported-shape space: one base class (`model::Person`), single-level
//! `filter`/`project`/`map`/`distinct`/`distinct(~[...])`/`sort` pipelines,
//! plus a small `join` family. Nested/chained filters, multi-hop navigation,
//! and additional base classes are still uncovered — see the crate's PR body
//! for the follow-up this foundation is scoped against.

use std::fmt::Write as _;

use proptest::prelude::*;
use proptest::sample::select;

use pure_analyzer_model::{ModelGraph, PmcdDocument, load_pmcd_documents};

/// Build the fixed multi-class fixture model every generated source lowers
/// against. `model::Person` carries the field pool [`FIELDS`] plus
/// `personId` (the join key); `model::Manager` exists only to give `Person`
/// a resolvable non-primitive property type (the generator itself never
/// navigates into it); `model::Membership` is the join family's right-hand
/// side.
pub fn model() -> ModelGraph {
    let source = serde_json::json!({
        "_type": "data",
        "elements": [
            {
                "_type": "class",
                "package": "model",
                "name": "Person",
                "stereotypes": [],
                "superTypes": [],
                "properties": [
                    {
                        "name": "name",
                        "genericType": {"rawType": "String", "typeArguments": []},
                        "multiplicity": {"lowerBound": 1, "upperBound": 1}
                    },
                    {
                        "name": "email",
                        "genericType": {"rawType": "String", "typeArguments": []},
                        "multiplicity": {"lowerBound": 1, "upperBound": 1}
                    },
                    {
                        "name": "age",
                        "genericType": {"rawType": "Integer", "typeArguments": []},
                        "multiplicity": {"lowerBound": 1, "upperBound": 1}
                    },
                    {
                        "name": "active",
                        "genericType": {"rawType": "Boolean", "typeArguments": []},
                        "multiplicity": {"lowerBound": 1, "upperBound": 1}
                    },
                    {
                        "name": "personId",
                        "genericType": {"rawType": "String", "typeArguments": []},
                        "multiplicity": {"lowerBound": 1, "upperBound": 1}
                    },
                    {
                        "name": "manager",
                        "genericType": {"rawType": "model::Manager", "typeArguments": []},
                        "multiplicity": {"lowerBound": 1, "upperBound": 1}
                    }
                ],
                "qualifiedProperties": []
            },
            {
                "_type": "class",
                "package": "model",
                "name": "Manager",
                "stereotypes": [],
                "superTypes": [],
                "properties": [],
                "qualifiedProperties": []
            },
            {
                "_type": "class",
                "package": "model",
                "name": "Membership",
                "stereotypes": [],
                "superTypes": [],
                "properties": [
                    {
                        "name": "personId",
                        "genericType": {"rawType": "String", "typeArguments": []},
                        "multiplicity": {"lowerBound": 1, "upperBound": 1}
                    }
                ],
                "qualifiedProperties": []
            }
        ]
    })
    .to_string();
    load_pmcd_documents(&[PmcdDocument::new("canonical-strategy-fixture", &source)])
        .expect("generator fixture model must load")
}

/// One scannable `model::Person` field the single-table generator draws
/// filters and projections from, paired with the Pure literal kind that
/// type-checks against it.
struct Field {
    name: &'static str,
}

const FIELDS: &[Field] = &[
    Field { name: "name" },
    Field { name: "email" },
    Field { name: "age" },
    Field { name: "active" },
];

const STRING_LITERALS: &[&str] = &["Ada", "Grace", "Alan"];
const INTEGER_LITERALS: &[i64] = &[0, 1, 42];

/// Alias overrides deliberately exercised on the first projected column,
/// including `"value"` — the exact literal alias a `->map` result also
/// carries internally, so this specifically stresses the
/// `ProjectionKind`-vs-column-name distinction (issues #263/#264) at
/// generator scale rather than only the two hand-picked regression fixtures.
const ALIAS_OVERRIDES: &[&str] = &["renamed", "Legal Name", "value"];

/// Interesting field-subset/order shapes for `->project(~[...])`: every
/// singleton, several two-field pairs in both orders, and two full-width
/// orders. A curated list rather than every permutation of `FIELDS`,
/// consistent with this module's documented "broad but bounded" scope.
const PROJECT_SHAPES: &[&[usize]] = &[
    &[0],
    &[1],
    &[2],
    &[3],
    &[0, 1],
    &[1, 0],
    &[0, 2],
    &[2, 0],
    &[0, 3],
    &[3, 0],
    &[1, 2],
    &[2, 1],
    &[1, 3],
    &[2, 3],
    &[0, 1, 2],
    &[2, 1, 0],
    &[0, 1, 2, 3],
    &[3, 2, 1, 0],
];

/// One `(field index, rendered literal text)` pair whose literal type-checks
/// against the field.
fn field_and_literal() -> impl Strategy<Value = (usize, String)> {
    prop_oneof![
        select(STRING_LITERALS).prop_map(|value| (0usize, format!("'{value}'"))),
        select(STRING_LITERALS).prop_map(|value| (1usize, format!("'{value}'"))),
        select(INTEGER_LITERALS).prop_map(|value| (2usize, value.to_string())),
        any::<bool>().prop_map(|value| (3usize, value.to_string())),
    ]
}

/// An optional `->filter(x| $x.field <op> literal)` clause.
fn filter_clause() -> impl Strategy<Value = Option<(usize, String, bool)>> {
    prop_oneof![
        3 => Just(None),
        5 => (field_and_literal(), any::<bool>())
            .prop_map(|((field, literal), negate)| Some((field, literal, negate))),
    ]
}

/// The projection stage: no projection, a `->map(f)` over one field, or a
/// `->project(~[...])` over a chosen field subset/order, with the first
/// column's alias occasionally overridden (including to `"value"`).
#[derive(Debug, Clone)]
enum ProjectStage {
    None,
    Map(usize),
    Full(Vec<(usize, String)>),
}

fn alias_for(field_index: usize, override_alias: Option<&'static str>) -> String {
    override_alias.map_or_else(|| FIELDS[field_index].name.to_owned(), str::to_owned)
}

fn project_stage() -> impl Strategy<Value = ProjectStage> {
    prop_oneof![
        2 => Just(ProjectStage::None),
        3 => (0..FIELDS.len()).prop_map(ProjectStage::Map),
        6 => (
            select(PROJECT_SHAPES),
            prop_oneof![
                3 => Just(None),
                1 => select(ALIAS_OVERRIDES).prop_map(Some),
            ],
        )
            .prop_map(|(shape, override_alias)| {
                let pairs = shape
                    .iter()
                    .enumerate()
                    .map(|(position, &field)| {
                        let alias = if position == 0 {
                            alias_for(field, override_alias)
                        } else {
                            alias_for(field, None)
                        };
                        (field, alias)
                    })
                    .collect();
                ProjectStage::Full(pairs)
            }),
    ]
}

/// The `distinct`/`distinct(~[...])`/`sort` tail, valid combinations
/// depending on what the projection stage produced: a `->map`/bare scan or
/// filter stays `Column`-bound (only a bare `->distinct()` applies to it,
/// per `canonical.rs`'s own class-extent-vs-`Relation<>` distinction); a
/// `->project(~[...])` becomes `Row`-bound and additionally accepts
/// `distinct(~[...])`/`sort([...])` selectors.
#[derive(Debug, Clone)]
enum TailStage {
    None,
    Distinct,
    DistinctOn(Vec<usize>),
    Sort(Vec<(usize, bool)>),
    DistinctOnThenSort(Vec<usize>, Vec<(usize, bool)>),
}

fn tail_stage_for(project: &ProjectStage) -> BoxedStrategy<TailStage> {
    match project {
        ProjectStage::Full(pairs) => {
            let count = pairs.len();
            let forward: Vec<usize> = (0..count).collect();
            let reverse: Vec<usize> = (0..count).rev().collect();
            let forward_for_sort = forward.clone();
            let reverse_for_distinct_then_sort = reverse.clone();
            prop_oneof![
                2 => Just(TailStage::None),
                2 => Just(TailStage::Distinct),
                2 => Just(TailStage::DistinctOn(forward.clone())),
                2 => Just(TailStage::DistinctOn(reverse.clone())),
                3 => proptest::collection::vec(any::<bool>(), count..=count).prop_map(
                    move |directions| {
                        TailStage::Sort(
                            forward_for_sort
                                .clone()
                                .into_iter()
                                .zip(directions)
                                .collect(),
                        )
                    }
                ),
                2 => proptest::collection::vec(any::<bool>(), count..=count).prop_map(
                    move |directions| {
                        TailStage::DistinctOnThenSort(
                            reverse_for_distinct_then_sort.clone(),
                            forward.clone().into_iter().zip(directions).collect(),
                        )
                    }
                ),
            ]
            .boxed()
        }
        ProjectStage::None | ProjectStage::Map(_) => prop_oneof![
            3 => Just(TailStage::None),
            2 => Just(TailStage::Distinct),
        ]
        .boxed(),
    }
}

/// Quote an alias exactly as `canonical.rs`'s own `column_name` would.
fn alias_token(alias: &str) -> String {
    let mut characters = alias.chars();
    let is_identifier = characters
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && characters.all(|character| character.is_ascii_alphanumeric() || character == '_');
    if is_identifier {
        alias.to_owned()
    } else {
        format!("'{}'", alias.replace('\'', "''"))
    }
}

fn render_query(
    filter: Option<(usize, String, bool)>,
    project: ProjectStage,
    tail: TailStage,
) -> String {
    let mut text = "model::Person.all()".to_owned();
    if let Some((field, literal, negate)) = filter {
        let operator = if negate { "!=" } else { "==" };
        write!(
            text,
            "->filter(x| $x.{} {operator} {literal})",
            FIELDS[field].name
        )
        .expect("writing to a String never fails");
    }
    let pairs = match project {
        ProjectStage::None => None,
        ProjectStage::Map(field) => {
            write!(text, "->map(x| $x.{})", FIELDS[field].name)
                .expect("writing to a String never fails");
            None
        }
        ProjectStage::Full(pairs) => {
            let specs = pairs
                .iter()
                .map(|(field, alias)| {
                    format!("{}: x | $x.{}", alias_token(alias), FIELDS[*field].name)
                })
                .collect::<Vec<_>>()
                .join(", ");
            write!(text, "->project(~[{specs}])").expect("writing to a String never fails");
            Some(pairs)
        }
    };
    let column_selector = |index: usize| {
        let pairs = pairs
            .as_ref()
            .expect("distinct(~[...])/sort selectors only generated after a Full project");
        alias_token(&pairs[index].1)
    };
    let render_distinct_on = |columns: &[usize]| {
        let names = columns
            .iter()
            .map(|&index| column_selector(index))
            .collect::<Vec<_>>()
            .join(", ");
        format!("->distinct(~[{names}])")
    };
    let render_sort = |keys: &[(usize, bool)]| {
        let parts = keys
            .iter()
            .map(|&(index, descending)| {
                let direction = if descending {
                    "descending"
                } else {
                    "ascending"
                };
                format!("{direction}(~{})", column_selector(index))
            })
            .collect::<Vec<_>>()
            .join(", ");
        format!("->sort([{parts}])")
    };
    match tail {
        TailStage::None => {}
        TailStage::Distinct => text.push_str("->distinct()"),
        TailStage::DistinctOn(columns) => text.push_str(&render_distinct_on(&columns)),
        TailStage::Sort(keys) => text.push_str(&render_sort(&keys)),
        TailStage::DistinctOnThenSort(columns, keys) => {
            text.push_str(&render_distinct_on(&columns));
            text.push_str(&render_sort(&keys));
        }
    }
    text
}

/// A weighted single-table `filter`/`project`/`map`/`distinct`/`sort`
/// pipeline source over `model::Person`.
pub fn arbitrary_single_table_query() -> impl Strategy<Value = String> {
    (filter_clause(), project_stage())
        .prop_flat_map(|(filter, project)| {
            tail_stage_for(&project).prop_map(move |tail| (filter.clone(), project.clone(), tail))
        })
        .prop_map(|(filter, project, tail)| render_query(filter, project, tail))
}

/// Interesting orderings/directions for the join family's two-column
/// `distinct(~[...])`/`sort([...])` tail.
const JOIN_COLUMN_ORDERS: &[[&str; 2]] = &[["Person", "Membership"], ["Membership", "Person"]];

/// A small `model::Person->join(model::Membership, ...)->distinct(~[...])
/// ->sort([...])` family, varying the selector order and sort directions —
/// exercising the emitter's join path (a distinct `RelationOperator` variant
/// the single-table generator above never reaches) alongside it, so the
/// injectivity check spans both.
pub fn arbitrary_join_query_source() -> impl Strategy<Value = String> {
    (
        select(JOIN_COLUMN_ORDERS),
        select(JOIN_COLUMN_ORDERS),
        any::<bool>(),
        any::<bool>(),
    )
        .prop_map(|(distinct_order, sort_order, first_desc, second_desc)| {
            let distinct = distinct_order.join(", ");
            let first_direction = if first_desc {
                "descending"
            } else {
                "ascending"
            };
            let second_direction = if second_desc {
                "descending"
            } else {
                "ascending"
            };
            let first_key = format!("{first_direction}(~{})", sort_order[0]);
            let second_key = format!("{second_direction}(~{})", sort_order[1]);
            format!(
                "model::Person.all()->join(model::Membership.all(), JoinKind.INNER, \
                 {{l, r | $l.personId == $r.personId}})->distinct(~[{distinct}])\
                 ->sort([{first_key}, {second_key}])"
            )
        })
}
