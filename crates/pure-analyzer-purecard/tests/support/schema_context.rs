//! Shared schema-context helpers for the live-Legend lanes: reads a fixture
//! DB's committed `corpus/schemas/*.md` context file and assembles the full
//! compilable Pure model text it needs (issue #55's PR #84 gap; extracted out
//! of `live_legend_schema_walk_compile.rs` so `real_model_legend_compile.rs`
//! (issue #58) reuses the exact same assembly rather than a second copy —
//! constitution §4, DRY).
#![cfg(feature = "legend")]

use std::path::PathBuf;

#[path = "store_grammar.rs"]
mod store_grammar;

/// `db_id`'s full Pure model text: the committed Class/Association grammar
/// (`pure_model_text`) plus the derived Database/Mapping/Connection/Runtime
/// grammar (`store_grammar::store_grammar_text`) arm-A's
/// `Db->tableReference(...)->tableToTDS()` shape needs and a class-anchored
/// query's own execution coordinates (`ClassRt`/`DbMapping`) name. Assembled
/// the same way for every caller, so a PMCD built from it always carries the
/// complete, documented coordinate set.
pub(crate) fn full_model_text(db_id: &str) -> String {
    format!(
        "{}\n{}",
        pure_model_text(db_id),
        store_grammar::store_grammar_text(db_id)
    )
}

/// The `corpus/schemas/*.md` context-file basename for `db_id` — the first
/// five [`crate::fixture_dbs::FIXTURE_DBS`] are arm-C pilot contexts, the last
/// three are out-of-sample (OOS).
pub(crate) fn schema_context_file(db_id: &str) -> PathBuf {
    let prefix = match db_id {
        "dog_kennels" | "student_transcripts_tracking" | "world_1" => "oos_ctx",
        _ => "armC_ctx",
    };
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("corpus/schemas")
        .join(format!("{prefix}_{db_id}.md"))
}

/// Extract the fenced ```pure code block under the `## Pure model` heading of
/// `db_id`'s committed schema-context file — the model text `run_pilot`
/// itself parses via `grammarToJson` (per that file's own "Assemble from
/// grammar" note), reproduced here rather than fetched from a live SDLC
/// workspace so this stays hermetic and CI-reproducible.
pub(crate) fn pure_model_text(db_id: &str) -> String {
    let path = schema_context_file(db_id);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("read schema context {}: {err}", path.display()));
    let heading = "## Pure model";
    let heading_at = text
        .find(heading)
        .unwrap_or_else(|| panic!("{} has no `{heading}` section", path.display()));
    let after_heading = &text[heading_at..];
    let fence_open = after_heading
        .find("```pure\n")
        .unwrap_or_else(|| panic!("{} has no ```pure fence after {heading}", path.display()));
    let body_start = fence_open + "```pure\n".len();
    let body = &after_heading[body_start..];
    let fence_close = body
        .find("```")
        .unwrap_or_else(|| panic!("{} has an unterminated ```pure fence", path.display()));
    body[..fence_close].to_string()
}

/// The first `classes:` entry from `db_id`'s `## Execution coordinates`
/// section — every schema-context file lists at least one, so a bare
/// `Class.all()` lambda is always constructible without per-class knowledge.
///
/// `dead_code`-allowed: only `live_legend_schema_walk_compile.rs`'s
/// compilation unit calls this (its own gold reference is "any class in this
/// DB"); `real_model_legend_compile.rs` needs a *specific* fixture's class
/// instead (carried in its own JSONL input), so its copy of this module never
/// calls it.
#[allow(dead_code)]
pub(crate) fn first_class_path(db_id: &str) -> String {
    let path = schema_context_file(db_id);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("read schema context {}: {err}", path.display()));
    let line = text
        .lines()
        .find(|line| line.starts_with("- classes:"))
        .unwrap_or_else(|| panic!("{} has no `- classes:` line", path.display()));
    let first_backtick = line.find('`').unwrap_or_else(|| {
        panic!(
            "{} `- classes:` line has no backtick-quoted class",
            path.display()
        )
    });
    let rest = &line[first_backtick + 1..];
    let end = rest.find('`').unwrap_or_else(|| {
        panic!(
            "{} `- classes:` line has an unterminated backtick",
            path.display()
        )
    });
    rest[..end].to_string()
}
