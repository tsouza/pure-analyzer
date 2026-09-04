//! Building blocks shared by every versioned JSON envelope this crate emits.
//!
//! `json.rs`, `comparison_json.rs`, and `canonical_emission_json.rs` each
//! serialize a different outcome, but they agree on one schema version and on
//! how a source file, a byte range, and an indecision reason are represented.
//! Keeping that agreement in one module means bumping the version, or fixing
//! a field, only ever has one place to happen.

use libpure::{LineColumn, SourceFile, SourceOrigin};
use pure_analyzer_diagnostics::{TextRange, TextSize};
use serde::Serialize;

/// Schema version shared by every JSON envelope this crate emits.
pub(crate) const JSON_SCHEMA_VERSION: &str = "1.0";

/// One retained source file, identified the same way in every JSON envelope.
#[derive(Serialize)]
pub(crate) struct JsonFile<'a> {
    id: u32,
    name: &'a str,
    origin: &'static str,
}

/// One byte range, identified the same way in every JSON envelope.
#[derive(Serialize)]
pub(crate) struct JsonRange {
    start: JsonPosition,
    end: JsonPosition,
}

/// One byte position, identified the same way in every JSON envelope.
#[derive(Serialize)]
pub(crate) struct JsonPosition {
    byte: u32,
    line: usize,
    column: usize,
}

/// An indecision's exact reason, identified the same way in every JSON envelope.
#[derive(Serialize)]
pub(crate) struct JsonReason {
    pub(crate) id: &'static str,
    pub(crate) blurb: &'static str,
}

pub(crate) fn json_file(source: &SourceFile) -> JsonFile<'_> {
    JsonFile {
        id: source.id().index(),
        name: source.name(),
        origin: origin_name(source.origin()),
    }
}

pub(crate) fn json_range(range: TextRange, start: LineColumn, end: LineColumn) -> JsonRange {
    JsonRange {
        start: json_position(range.start(), start),
        end: json_position(range.end(), end),
    }
}

pub(crate) fn json_position(byte: TextSize, location: LineColumn) -> JsonPosition {
    JsonPosition {
        byte: u32::from(byte),
        line: location.line,
        column: location.column,
    }
}

const fn origin_name(origin: &SourceOrigin) -> &'static str {
    match origin {
        SourceOrigin::File { .. } => "file",
        SourceOrigin::InMemory => "memory",
        SourceOrigin::Stdin => "stdin",
    }
}
