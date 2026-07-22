//! Generator for the Spider structural corpus (`corpus/spider_schemas/`).
//!
//! The delivered Spider corpus is 100% templated over 158 schemas, so rather than
//! vendor ~50k redundant JSONL lines this module *derives* the cases from the
//! schemas themselves: every case is a pure function of a schema's classes,
//! members, and associations, so a case can never drift from the schema it probes
//! (constitution §4, DRY). Every generated string is one of a handful of templates;
//! the schema supplies the identifiers.
//!
//! The delivered schemas carry only *structure* — every property is typed
//! `String[0..1]` (the real Spider column types were lost upstream) — so this corpus
//! exercises the **structural** rules (N1 member, N2 chained nav, N3 source class)
//! across broad schema variety, plus the one type case that all-`String` still
//! expresses soundly (a numeric literal against a `String` column, T1). The
//! real-typed rules stay on the eight canonical `tests/fixtures/schemas`.
//!
//! Each [`Case`] is a soundness assertion (a real construct that MUST stream /
//! admit) or a precision assertion (a phantom that MUST be masked). A precision
//! case the decoder is known to still leak carries a [`GapKind`] tag; the replay
//! gate asserts leaks ⟺ tags with exact counts, so the allowlist cannot hide an
//! unexpected regression and a fixed gap reddens until its tag is removed.

use std::collections::BTreeSet;
use std::path::Path;

/// The nav-dot phantom every schema rejects: no member begins `zzz…`, so it is a
/// pure phantom (never a prefix of a real member) and MUST always be masked.
const PURE_PHANTOM: &str = "zzzbogus";

/// A source class name absent from every schema (N3). Distinct from any real
/// Spider class, so `…default::<this>.all()` must be masked at the class name.
const PHANTOM_CLASS: &str = "Zzzbogus";

/// The phantom tail appended after an ambiguous association-end in an N2 phantom
/// case (`$x.<end>.<this>`): a member no target class defines.
const NAV_PHANTOM_TAIL: &str = "zzbogus";

/// The lambda binder used in every generated filter/project — its exact spelling is
/// irrelevant to narrowing, so one fixed, un-clashing name keeps the templates DRY.
const BINDER: &str = "x";

/// A raw schema read straight from a delivered JSON file for case generation —
/// deliberately independent of the opaque shipped `Schema`, so the generator sees
/// class/member/association structure the public API does not expose.
#[derive(serde::Deserialize)]
struct SchemaJson {
    db_id: String,
    // `BTreeMap`, not `HashMap`: `generate_for_schema` iterates this map to emit
    // cases, so an ordered map keeps the generated case order deterministic across
    // runs (matching `read_schemas`' filename sort). Counts are order-independent,
    // but first-failure diagnostics must reproduce.
    classes: std::collections::BTreeMap<String, ClassJson>,
    #[serde(default)]
    associations: Vec<AssocJson>,
}

#[derive(serde::Deserialize)]
struct ClassJson {
    simple_name: String,
    properties: Vec<PropJson>,
}

#[derive(serde::Deserialize)]
struct PropJson {
    name: String,
}

#[derive(serde::Deserialize)]
struct AssocJson {
    ends: Vec<EndJson>,
}

#[derive(serde::Deserialize)]
struct EndJson {
    property_name: String,
    target_class: String,
}

/// What a case asserts about the decoder at its decision point.
#[derive(Debug, Clone)]
pub enum Check {
    /// The full `query` must stream token-by-token to an accepting state.
    Streams,
    /// The full `query` must dead-end (a masked/rejected token) before completing.
    DeadEnds,
    /// After driving `query` as a prefix, the probe token must be admitted.
    ProbeAdmitted(Vec<u8>),
    /// After driving `query` as a prefix, the probe token must be masked.
    ProbeMasked(Vec<u8>),
}

impl Check {
    /// Whether this is a soundness assertion (a real construct that must survive).
    #[must_use]
    pub fn is_soundness(&self) -> bool {
        matches!(self, Check::Streams | Check::ProbeAdmitted(_))
    }
}

/// A documented precision gap a case is currently known to leak — an L1
/// over-approximation the decoder does not yet tighten. Each is tracked for a
/// follow-up fix; the replay gate pins the exact count per kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GapKind {
    /// N1 identifier-termination: a phantom that is a strict prefix of a real
    /// member is admitted, because a valid-prefix token cannot be distinguished
    /// from one that will continue to the full member (`$x.concert` vs
    /// `$x.concertName`).
    N1PrefixNotTerminated,
    /// N2 nav off a name that is *both* an association end and a scalar property:
    /// the scalar reading terminates navigation, so the following `.member` is not
    /// narrowed and a phantom tail leaks (`$x.party.zzbogus`).
    N2AmbiguousScalarEnd,
}

/// One generated corpus case.
#[derive(Debug, Clone)]
pub struct Case {
    /// A stable, human-readable id (`<db>/<rule>/<detail>`).
    pub id: String,
    /// The database this case's schema came from.
    pub db: String,
    /// The full query, or the prefix for a probe case.
    pub query: String,
    /// What the decoder must do.
    pub check: Check,
    /// `Some` when this precision case is a known, documented leak.
    pub gap: Option<GapKind>,
}

/// The classpath of a class simple-name under a database, matching the delivered
/// convention (`spider::<db>::model::default::<Class>`).
fn classpath(db: &str, class: &str) -> String {
    format!("spider::{db}::model::default::{class}")
}

/// The class simple-name at the tail of a classpath — the inverse of the
/// `<Class>` slot [`classpath`] fills, factored out so the per-rule generators name
/// it one way.
fn class_of(cp: &str) -> &str {
    cp.rsplit("::").next().unwrap_or(cp)
}

/// Whether a schema identifier is a Pure-legal lexer identifier (starts with a
/// letter or `_`). A digit-leading Spider column (e.g. `1849RatingShare`) is not
/// expressible as a Pure member and is skipped — it is an upstream naming artifact,
/// not a decoder concern.
fn is_pure_ident(name: &str) -> bool {
    matches!(name.bytes().next(), Some(b) if b.is_ascii_alphabetic() || b == b'_')
}

/// Read and parse every `*.json` schema in `dir`, sorted by filename for a stable,
/// deterministic case order (so the pinned counts and ids never depend on
/// directory-walk order).
fn read_schemas(dir: &Path) -> Vec<SchemaJson> {
    let mut paths: Vec<_> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("read schema dir {}: {e}", dir.display()))
        .map(|e| e.expect("dir entry").path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    paths.sort();
    paths
        .into_iter()
        .map(|p| {
            let json =
                std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()));
            serde_json::from_str(&json).unwrap_or_else(|e| panic!("parse {}: {e}", p.display()))
        })
        .collect()
}

/// Generate every structural case for the schemas in `dir`.
pub fn generate(dir: &Path) -> Vec<Case> {
    let mut cases = Vec::new();
    for schema in read_schemas(dir) {
        generate_for_schema(&schema, &mut cases);
    }
    cases
}

/// Every `.`-navigation an association exposes, as `(source_path, via, target_path)`:
/// the end at one side is navigable *from the class the other end targets*. Only
/// 2-end associations whose property name is a Pure identifier are yielded — the one
/// place the association-traversal rule lives, shared by [`navigable_members`] (which
/// wants `source`/`via`) and [`generate_n2`] (which also wants `target`).
fn navigations(schema: &SchemaJson) -> impl Iterator<Item = (&str, &str, &str)> {
    schema
        .associations
        .iter()
        .filter(|a| a.ends.len() == 2)
        .flat_map(|a| {
            (0..2).map(move |from| {
                let to = 1 - from;
                (
                    a.ends[from].target_class.as_str(),
                    a.ends[to].property_name.as_str(),
                    a.ends[to].target_class.as_str(),
                )
            })
        })
        .filter(|(_, via, _)| is_pure_ident(via))
}

/// Every property name navigable from a class via `.`, keyed by class path: its
/// scalar properties **plus** the association ends whose opposite end targets it.
/// This mirrors the decoder's N1 member universe — an association end is a real
/// navigable member, so `.d` for a `domainAuthors` end is legitimately admitted and
/// a phantom must begin no such name.
fn navigable_members(schema: &SchemaJson) -> std::collections::HashMap<&str, Vec<&str>> {
    let mut members: std::collections::HashMap<&str, Vec<&str>> = schema
        .classes
        .iter()
        .map(|(path, class)| {
            let scalars = class
                .properties
                .iter()
                .map(|p| p.name.as_str())
                .filter(|n| is_pure_ident(n))
                .collect();
            (path.as_str(), scalars)
        })
        .collect();
    for (source, via, _target) in navigations(schema) {
        if let Some(list) = members.get_mut(source) {
            list.push(via);
        }
    }
    members
}

fn generate_for_schema(schema: &SchemaJson, out: &mut Vec<Case>) {
    let db = &schema.db_id;
    // N3: a phantom source class is masked at the class name.
    out.push(Case {
        id: format!("{db}/N3/{PHANTOM_CLASS}"),
        db: db.clone(),
        query: format!("|{}.all()->size()", classpath(db, PHANTOM_CLASS)),
        check: Check::DeadEnds,
        gap: None,
    });

    let navigable = navigable_members(schema);
    for (path, class) in &schema.classes {
        let cp = classpath(db, &class.simple_name);
        // Scalar properties anchor the N1/T1 value cases (a real scalar `== 'x'`);
        // the full navigable set (scalars + association ends) is the phantom/fusion
        // universe, matching what the decoder actually admits after `$x.`.
        let scalars: Vec<&str> = class
            .properties
            .iter()
            .map(|p| p.name.as_str())
            .filter(|n| is_pure_ident(n))
            .collect();
        let full: Vec<&str> = navigable.get(path.as_str()).cloned().unwrap_or_default();
        if full.is_empty() {
            continue;
        }
        let full_set: BTreeSet<&str> = full.iter().copied().collect();

        generate_n1(db, &cp, &scalars, &full, &full_set, out);
        generate_n1_fusion(db, &cp, &full, out);
        generate_t1(db, &cp, &scalars, out);
    }

    generate_n2(schema, out);
}

/// N1: every real member streams (soundness); a pure phantom and a strict-prefix
/// phantom are masked (precision — the prefix phantom is a known leak).
fn generate_n1(
    db: &str,
    cp: &str,
    scalars: &[&str],
    members: &[&str],
    member_set: &BTreeSet<&str>,
    out: &mut Vec<Case>,
) {
    let class = class_of(cp);
    // Soundness rides on scalar properties: `<scalar> == 'x'` is a well-formed
    // comparison, whereas an association end is a class collection (its arity is
    // exercised by N2 chained navigation instead).
    for m in scalars {
        out.push(Case {
            id: format!("{db}/N1_soundness/{class}.{m}"),
            db: db.to_owned(),
            query: format!("|{cp}.all()->filter({BINDER}|${BINDER}.{m} == 'x')->size()"),
            check: Check::Streams,
            gap: None,
        });
    }
    // A pure phantom: masked, and never a prefix of any navigable member.
    out.push(Case {
        id: format!("{db}/N1_phantom/{class}.{PURE_PHANTOM}"),
        db: db.to_owned(),
        query: format!("|{cp}.all()->filter({BINDER}|${BINDER}.{PURE_PHANTOM} == 'x')->size()"),
        check: Check::DeadEnds,
        gap: None,
    });
    // A strict-prefix phantom (a proper prefix of some navigable member that is not
    // itself a member): the decoder currently admits it (N1 termination gap).
    if let Some((phantom, member)) = strict_prefix_phantom(members, member_set) {
        out.push(Case {
            id: format!("{db}/N1_prefix/{class}.{phantom}(<{member})"),
            db: db.to_owned(),
            query: format!("|{cp}.all()->filter({BINDER}|${BINDER}.{phantom} == 'x')->size()"),
            check: Check::DeadEnds,
            gap: Some(GapKind::N1PrefixNotTerminated),
        });
    }
}

/// The shortest proper prefix of some member that is not itself a member — the
/// canonical trigger of the identifier-termination gap. `None` when every member's
/// prefixes are themselves members (rare) or there is nothing to truncate.
fn strict_prefix_phantom<'a>(
    members: &[&'a str],
    member_set: &BTreeSet<&'a str>,
) -> Option<(&'a str, &'a str)> {
    members
        .iter()
        .filter_map(|m| {
            (1..m.len())
                .map(|k| &m[..k])
                .find(|p| is_pure_ident(p) && !member_set.contains(p))
                .map(|p| (p, *m))
        })
        .min_by_key(|(p, _)| p.len())
}

/// N1 fusion: at the pre-dot anchor `$x`, the fused `.<char>` token is admitted iff
/// some member begins with that letter, else masked (the PR #45 fix class). Covers
/// `a..=z`; a digit can never begin a Pure member, so every `.<digit>` is masked.
fn generate_n1_fusion(db: &str, cp: &str, members: &[&str], out: &mut Vec<Case>) {
    let class = class_of(cp);
    let legal_first: BTreeSet<u8> = members
        .iter()
        .filter_map(|m| m.bytes().next())
        .map(|b| b.to_ascii_lowercase())
        .collect();
    let prefix = format!("|{cp}.all()->filter({BINDER}|${BINDER}");
    for c in b'a'..=b'z' {
        let probe = vec![b'.', c];
        let admitted = legal_first.contains(&c);
        out.push(Case {
            id: format!("{db}/N1_fusion/{class}.{}", c as char),
            db: db.to_owned(),
            query: prefix.clone(),
            check: if admitted {
                Check::ProbeAdmitted(probe)
            } else {
                Check::ProbeMasked(probe)
            },
            gap: None,
        });
    }
}

/// T1: on an all-`String` schema, a numeric literal against any member is a type
/// mismatch and must be masked at the operand. (The real-typed inverse, a string
/// literal against a numeric column, lives on the canonical fixtures.)
fn generate_t1(db: &str, cp: &str, members: &[&str], out: &mut Vec<Case>) {
    let class = class_of(cp);
    // One representative member per class keeps T1 breadth without 4,503 near-clones.
    let Some(m) = members.first() else { return };
    out.push(Case {
        id: format!("{db}/T1/{class}.{m}"),
        db: db.to_owned(),
        query: format!("|{cp}.all()->filter({BINDER}|${BINDER}.{m} == 5)->size()"),
        check: Check::DeadEnds,
        gap: None,
    });
}

/// N2: for each association end navigable from a class, a real 2-hop navigation to
/// a target member streams (soundness); a phantom tail is masked (precision — a leak
/// when the end name is also a scalar property of the source class).
fn generate_n2(schema: &SchemaJson, out: &mut Vec<Case>) {
    let db = &schema.db_id;
    let by_path: std::collections::HashMap<&str, &ClassJson> = schema
        .classes
        .iter()
        .map(|(p, c)| (p.as_str(), c))
        .collect();
    for (source, via, target) in navigations(schema) {
        let (Some(src_class), Some(tgt_class)) = (by_path.get(source), by_path.get(target)) else {
            continue;
        };
        let cp = classpath(db, &src_class.simple_name);
        let target_member = tgt_class
            .properties
            .iter()
            .map(|p| p.name.as_str())
            .find(|n| is_pure_ident(n));
        let Some(tm) = target_member else { continue };
        // Soundness: real 2-hop nav must stream.
        out.push(Case {
            id: format!("{db}/N2_soundness/{}.{via}.{tm}", src_class.simple_name),
            db: db.clone(),
            query: format!("|{cp}.all()->filter({BINDER}|${BINDER}.{via}.{tm} == 'x')->size()"),
            check: Check::Streams,
            gap: None,
        });
        // Precision: a phantom tail after the nav must be masked. It leaks when
        // `via` is also a scalar property of the source class (the scalar reading
        // terminates navigation).
        let ambiguous = src_class.properties.iter().any(|p| p.name == via);
        out.push(Case {
            id: format!(
                "{db}/N2_phantom/{}.{via}.{NAV_PHANTOM_TAIL}",
                src_class.simple_name
            ),
            db: db.clone(),
            query: format!(
                "|{cp}.all()->filter({BINDER}|${BINDER}.{via}.{NAV_PHANTOM_TAIL} == 'x')->size()"
            ),
            check: Check::DeadEnds,
            gap: ambiguous.then_some(GapKind::N2AmbiguousScalarEnd),
        });
    }
}
