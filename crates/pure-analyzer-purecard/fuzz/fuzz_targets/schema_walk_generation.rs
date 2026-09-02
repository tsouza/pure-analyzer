//! Fuzz the schema-aware accepting-walk generator (issue #59) against
//! arbitrary-but-valid schema shapes.
//!
//! `schema_walk_completeness.rs`/`schema_walk_properties.rs` already prove
//! `generate_schema_walks`/`generate_first_complete_schema_walks` never
//! panic and always replay soundly over the 8 committed `FIXTURE_DBS`
//! schemas — real, but hand-authored and structurally similar to each other.
//! This target explores schema *shapes* that corpus doesn't cover (empty
//! classes, many properties, name collisions with `all`/structural bytes,
//! unusual primitive-type mixes) by deriving a schema from arbitrary fuzzer
//! bytes rather than fixed fixtures. `schema_from_json.rs` already fuzzes
//! whether arbitrary bytes parse as a `Schema` at all; this target fuzzes
//! what the *walker* does once a schema, however oddly shaped, did parse.
#![no_main]

use std::collections::HashSet;

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;
use purecard::{CompiledGrammar, DecoderSession, Schema, Vocab};
use schema_walker::{generate_first_complete_schema_walks, generate_schema_walks};

/// Caps a fuzzed schema's class/property counts so a single input can't blow
/// up generation time — shape diversity is the target here, not scale.
const MAX_CLASSES: usize = 4;
const MAX_PROPERTIES: usize = 5;
/// Caps a sanitized identifier's length for the same reason.
const MAX_NAME_LEN: usize = 12;

#[derive(Arbitrary, Debug)]
struct RawSchema {
    classes: Vec<RawClass>,
}

#[derive(Arbitrary, Debug)]
struct RawClass {
    name: String,
    properties: Vec<RawProperty>,
}

#[derive(Arbitrary, Debug)]
struct RawProperty {
    name: String,
    primitive: u8,
    required: bool,
}

/// Sanitize `raw` into a non-empty, ASCII-alphanumeric Pure identifier: any
/// non-identifier byte is dropped (not replaced), the result is capped to
/// [`MAX_NAME_LEN`], and `fallback` (already identifier-safe and unique per
/// caller) is prepended whenever sanitizing alone doesn't leave a
/// letter-led, non-empty name — collapsing every unusable fuzzer string onto
/// a small, still-distinct set of real identifiers rather than discarding
/// the input.
fn sanitize_ident(raw: &str, fallback: &str) -> String {
    let cleaned: String = raw
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(MAX_NAME_LEN)
        .collect();
    if cleaned
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic())
    {
        cleaned
    } else {
        format!("{fallback}{cleaned}")
    }
}

fn primitive_name(tag: u8) -> &'static str {
    match tag % 5 {
        0 => "Integer",
        1 => "Float",
        2 => "String",
        3 => "Boolean",
        _ => "DateTime",
    }
}

/// The structural bytes every fixture-DB vocab in the real test suite also
/// offers as their own single-byte token, so a walk can explore beyond bare
/// identifiers.
const STRUCTURAL_BYTES: &[u8] = b"abXY1_ |{}()[].,;:$%'-><=!&+*/";

fuzz_target!(|raw: RawSchema| {
    let mut vocab_tokens: Vec<Vec<u8>> = STRUCTURAL_BYTES.iter().map(|&b| vec![b]).collect();
    vocab_tokens.push(b"all".to_vec());

    let mut seen_classes: HashSet<String> = HashSet::new();
    let mut class_entries: Vec<String> = Vec::new();
    for (i, class) in raw.classes.iter().take(MAX_CLASSES).enumerate() {
        let cname = sanitize_ident(&class.name, &format!("C{i}"));
        if !seen_classes.insert(cname.clone()) {
            continue;
        }
        vocab_tokens.push(cname.as_bytes().to_vec());
        vocab_tokens.push(format!("spider::fuzz::model::default::{cname}").into_bytes());

        let mut seen_props: HashSet<String> = HashSet::new();
        let mut prop_entries: Vec<String> = Vec::new();
        for (j, prop) in class.properties.iter().take(MAX_PROPERTIES).enumerate() {
            let pname = sanitize_ident(&prop.name, &format!("p{j}"));
            if !seen_props.insert(pname.clone()) {
                continue;
            }
            vocab_tokens.push(pname.as_bytes().to_vec());
            let prim = primitive_name(prop.primitive);
            let upper = if prop.required { "1" } else { "1" };
            let lower = u8::from(prop.required);
            prop_entries.push(format!(
                r#"{{"name": "{pname}", "type": {{"kind": "primitive", "name": "{prim}"}}, "mult": {{"lower": {lower}, "upper": {upper}}}}}"#
            ));
        }
        class_entries.push(format!(
            r#""spider::fuzz::model::default::{cname}": {{"simple_name": "{cname}", "properties": [{}], "qualified_properties": [], "super_types": []}}"#,
            prop_entries.join(",")
        ));
    }
    if class_entries.is_empty() {
        // No source classpath to build a `.all()` pipeline from at all.
        return;
    }

    let schema_json = format!(
        r#"{{"db_id": "fuzz", "db_path": "spider::fuzz::Db", "classes": {{{}}}, "associations": [], "enums": {{}}}}"#,
        class_entries.join(",")
    );
    let Ok(schema) = Schema::from_json(&schema_json) else {
        // Sanitization is best-effort; a schema that still fails to parse
        // isn't this target's concern (schema_from_json.rs already fuzzes
        // that surface directly).
        return;
    };

    vocab_tokens.sort();
    vocab_tokens.dedup();
    let eos = vocab_tokens.len() as u32;
    let vocab = Vocab::from_byte_tokens(vocab_tokens);
    let grammar = CompiledGrammar::compile(vocab);

    // Never panic (generate_schema_walks/generate_first_complete_schema_walks
    // panic internally if they fall short of WALK_COUNT — a genuine finding,
    // not a false positive, since a real caller relies on that guarantee),
    // and every returned walk must replay cleanly through a fresh session —
    // the same invariant schema_walk_completeness.rs proves over the fixed
    // 8-DB corpus, checked here against schema shapes that corpus doesn't
    // cover.
    for walk in generate_schema_walks(&grammar, &schema) {
        let mut session = DecoderSession::with_schema(&grammar, schema.clone())
            .expect("a fixed-engine grammar always accepts a schema overlay");
        for id in walk {
            session
                .accept_token(id)
                .expect("a generated walk's own token must always be admissible");
        }
        assert!(
            session.is_complete(),
            "a walk generate_schema_walks returned did not replay to completion"
        );
    }
    for walk in generate_first_complete_schema_walks(&grammar, &schema) {
        let mut session = DecoderSession::with_schema(&grammar, schema.clone())
            .expect("a fixed-engine grammar always accepts a schema overlay");
        for id in walk {
            session
                .accept_token(id)
                .expect("a generated walk's own token must always be admissible");
        }
        assert!(
            session.is_complete(),
            "a walk generate_first_complete_schema_walks returned did not replay to completion"
        );
    }
});
