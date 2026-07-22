//! Streaming loader for the gold-query corpus (`corpus/gold_queries.jsonl`).
//!
//! One JSON object per line (`docs/spec/testing.md` §13.1). Records are parsed lazily, one
//! line at a time, so the ~4.7 MB file is never loaded whole.

use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

use crate::error::CorpusError;

/// One line of `corpus/gold_queries.jsonl`.
///
/// Shared scaffolding across the replay targets; each reads a different subset of
/// fields (e.g. the BPE-split target does not read `source_id`), so `dead_code`
/// fires per-target even though every field is live somewhere in the suite.
#[derive(Debug, Clone, serde::Deserialize)]
#[allow(dead_code)]
pub struct GoldRecord {
    /// Spider database id, e.g. `"car_1"`.
    pub db_id: String,
    /// Provenance id of the source query, e.g. `"train_spider:6715"`.
    pub source_id: String,
    /// Emitted idiom: `"A"` (relational) or `"C"` (class-navigation).
    pub arm: String,
    /// SQL-level construct tags behind the query, e.g. `["agg", "group_by"]`.
    pub constructs: Vec<String>,
    /// The execution-verified gold Pure lambda text — the §8.1 replay input.
    pub pure_text: String,
}

/// Stream gold records from `path`, one parsed record per line, lazily.
///
/// Blank lines (e.g. a trailing newline) are skipped. Each non-blank line is
/// parsed on demand; a parse failure yields [`CorpusError::Json`] carrying the
/// 1-based line number.
///
/// # Errors
/// Returns [`CorpusError::Io`] if `path` cannot be opened. Per-line I/O and
/// parse failures surface as `Err` items of the returned iterator.
pub fn load_gold(
    path: &Path,
) -> Result<impl Iterator<Item = Result<GoldRecord, CorpusError>> + use<>, CorpusError> {
    let reader = BufReader::new(File::open(path)?);
    let records = reader.lines().enumerate().filter_map(|(index, line)| {
        let line = match line {
            Ok(text) => text,
            Err(source) => return Some(Err(CorpusError::Io(source))),
        };
        if line.trim().is_empty() {
            return None;
        }
        let parsed =
            serde_json::from_str::<GoldRecord>(&line).map_err(|source| CorpusError::Json {
                line: index + 1,
                source,
            });
        Some(parsed)
    });
    Ok(records)
}

#[cfg(test)]
mod tests {
    use super::{GoldRecord, load_gold};
    use crate::error::CorpusError;
    use std::io::Write;

    fn write_temp(name: &str, contents: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(name);
        let mut file = std::fs::File::create(&path).expect("create temp corpus");
        file.write_all(contents.as_bytes())
            .expect("write temp corpus");
        path
    }

    #[test]
    fn parses_a_valid_line() {
        let json = r#"{"db_id":"car_1","source_id":"train_spider:1","arm":"C","constructs":["agg"],"pure_text":"|X.all()"}"#;
        let record: GoldRecord = serde_json::from_str(json).expect("valid record");
        assert_eq!(record.db_id, "car_1");
        assert_eq!(record.arm, "C");
        assert_eq!(record.constructs, ["agg"]);
        assert_eq!(record.pure_text, "|X.all()");
    }

    #[test]
    fn streams_records_and_skips_blank_lines() {
        let line = r#"{"db_id":"d","source_id":"s","arm":"A","constructs":[],"pure_text":"|Y"}"#;
        let path = write_temp("purecard_gold_ok.jsonl", &format!("{line}\n\n{line}\n"));
        // The blank middle line is skipped, so the two data lines yield exactly
        // two records and every item parses `Ok` — the blank contributes nothing.
        let items: Vec<Result<GoldRecord, CorpusError>> = load_gold(&path).expect("open").collect();
        assert_eq!(items.len(), 2, "{items:?}");
        assert!(items.iter().all(Result::is_ok), "{items:?}");
    }

    #[test]
    fn a_blank_only_file_yields_no_records() {
        let path = write_temp("purecard_gold_blank.jsonl", "\n\n  \n\t\n");
        let count = load_gold(&path).expect("open").count();
        assert_eq!(count, 0);
    }

    #[test]
    fn malformed_line_reports_its_line_number() {
        let good = r#"{"db_id":"d","source_id":"s","arm":"A","constructs":[],"pure_text":"|Y"}"#;
        let path = write_temp("purecard_gold_bad.jsonl", &format!("{good}\n{{not json\n"));
        let errors: Vec<CorpusError> = load_gold(&path)
            .expect("open")
            .filter_map(Result::err)
            .collect();
        assert!(
            matches!(errors.as_slice(), [CorpusError::Json { line: 2, .. }]),
            "{errors:?}"
        );
    }

    #[test]
    fn a_blank_line_before_a_malformed_line_reports_the_physical_line() {
        // The blank first line is skipped but still counted, so the malformed
        // third line must be reported as physical line 3 — pins the
        // enumerate-before-filter ordering against an enumerate-after-filter
        // regression that would misreport it as line 2.
        let good = r#"{"db_id":"d","source_id":"s","arm":"A","constructs":[],"pure_text":"|Y"}"#;
        let path = write_temp(
            "purecard_gold_blank_then_bad.jsonl",
            &format!("\n{good}\n{{not json\n"),
        );
        let errors: Vec<CorpusError> = load_gold(&path)
            .expect("open")
            .filter_map(Result::err)
            .collect();
        assert!(
            matches!(errors.as_slice(), [CorpusError::Json { line: 3, .. }]),
            "{errors:?}"
        );
    }

    #[test]
    fn an_invalid_utf8_line_surfaces_as_an_io_error() {
        // `BufRead::lines` yields `Err` for a line that is not valid UTF-8, which
        // must map to `CorpusError::Io` (and render via its `Display`).
        let path = std::env::temp_dir().join("purecard_gold_bad_utf8.jsonl");
        std::fs::write(&path, [0xff, 0xfe, b'\n']).expect("write temp corpus");
        let errors: Vec<CorpusError> = load_gold(&path)
            .expect("open")
            .filter_map(Result::err)
            .collect();
        assert!(
            matches!(errors.as_slice(), [CorpusError::Io(_)]),
            "{errors:?}"
        );
        assert!(!errors[0].to_string().is_empty());
    }
}
