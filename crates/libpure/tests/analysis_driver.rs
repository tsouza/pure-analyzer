//! End-to-end facade tests for the libpure analysis driver.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use libpure::{
    AnalysisDriver, DiagCode, FileId, LintRequest, ModelInput, SourceFile, SourceInput,
    SourceRequest,
};

const SEQUENTIAL_JOBS: usize = 1;
const PARALLEL_JOBS: usize = 2;
const PARITY_QUERY: &str = "(first, second)";
const INDEX_QUERY: &str = "$rows[$index]";
const FORMATTED_PARITY_QUERY: &str = "(first, second)\n";
const FORMATTED_INDEX_QUERY: &str = "$rows[$index]\n";
const MODEL: &str = r#"{
    "_type": "data",
    "elements": [{
        "_type": "class",
        "package": "model",
        "name": "Person",
        "stereotypes": [],
        "superTypes": [],
        "properties": [{
            "name": "name",
            "genericType": {"rawType": "String", "typeArguments": []},
            "multiplicity": {"lowerBound": 0, "upperBound": 1}
        }],
        "qualifiedProperties": []
    }]
}"#;

static TEMP_FILE_COUNTER: AtomicUsize = AtomicUsize::new(0);

struct FileFixture {
    path: PathBuf,
}

impl FileFixture {
    fn new(name: &str, text: &str) -> Self {
        let counter = TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "pure-analyzer-libpure-integration-{}-{counter}-{name}",
            std::process::id()
        ));
        std::fs::write(&path, text).expect("write file fixture");
        Self { path }
    }
}

impl Drop for FileFixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn source_request(jobs: usize) -> SourceRequest {
    SourceRequest::new([
        SourceInput::in_memory("tuple.pure", PARITY_QUERY),
        SourceInput::stdin(INDEX_QUERY),
    ])
    .with_jobs(jobs)
}

fn lint_request(jobs: usize) -> LintRequest {
    LintRequest::new(
        SourceRequest::new([
            SourceInput::in_memory("first.pure", "model::Person.all()->filter(x| $x.missing)"),
            SourceInput::stdin("model::Person.all()->filter(x| $x.name)"),
        ])
        .with_jobs(jobs),
        [ModelInput::pmcd(SourceInput::in_memory(
            "model.json",
            MODEL,
        ))],
    )
}

#[test]
fn parse_validate_and_format_match_file_and_memory_snapshots() {
    let driver = AnalysisDriver;
    let query_file = FileFixture::new("query.pure", PARITY_QUERY);
    let memory_request = SourceRequest::new([SourceInput::in_memory("query.pure", PARITY_QUERY)]);
    let file_request = SourceRequest::new([SourceInput::file(&query_file.path)]);

    let memory_parse = driver
        .parse(&memory_request)
        .expect("parse in-memory source");
    let file_parse = driver
        .parse(&file_request)
        .expect("parse filesystem source");
    assert_eq!(memory_parse.parsed(), file_parse.parsed());
    assert_eq!(memory_parse.diagnostics(), file_parse.diagnostics());
    assert_eq!(memory_parse.parsed().len(), 1);
    assert!(memory_parse.parsed()[0].syntax().tokens().next().is_some());
    assert_eq!(
        file_parse
            .sources()
            .get(FileId::new(0))
            .map(SourceFile::text),
        Some(PARITY_QUERY)
    );

    let memory_validate = driver
        .validate(&memory_request)
        .expect("validate in-memory source");
    let file_validate = driver
        .validate(&file_request)
        .expect("validate filesystem source");
    assert_eq!(memory_validate.diagnostics(), file_validate.diagnostics());
    assert_eq!(
        memory_validate
            .diagnostics()
            .iter()
            .map(|diagnostic| diagnostic.code)
            .collect::<Vec<_>>(),
        vec![DiagCode::ParenthesizedTuple]
    );

    let memory_format = driver
        .format(&memory_request)
        .expect("format in-memory source");
    let file_format = driver
        .format(&file_request)
        .expect("format filesystem source");
    assert_eq!(memory_format.formatted(), file_format.formatted());
    assert_eq!(memory_format.diagnostics(), file_format.diagnostics());
    assert_eq!(memory_format.formatted()[0].text(), FORMATTED_PARITY_QUERY);
    assert_eq!(
        file_format
            .sources()
            .get(FileId::new(0))
            .map(SourceFile::text),
        Some(PARITY_QUERY)
    );
}

#[test]
fn parse_validate_and_format_are_deterministic_across_execution_modes() {
    let driver = AnalysisDriver;
    let sequential_request = source_request(SEQUENTIAL_JOBS);
    let parallel_request = source_request(PARALLEL_JOBS);

    let sequential_parse = driver.parse(&sequential_request).expect("sequential parse");
    let parallel_parse = driver.parse(&parallel_request).expect("parallel parse");
    let repeated_parse = driver.parse(&parallel_request).expect("repeated parse");
    assert_eq!(sequential_parse, parallel_parse);
    assert_eq!(parallel_parse, repeated_parse);
    assert_eq!(
        parallel_parse
            .parsed()
            .iter()
            .map(|source| source.file())
            .collect::<Vec<_>>(),
        vec![FileId::new(0), FileId::new(1)]
    );

    let sequential_validate = driver
        .validate(&sequential_request)
        .expect("sequential validation");
    let parallel_validate = driver
        .validate(&parallel_request)
        .expect("parallel validation");
    let repeated_validate = driver
        .validate(&parallel_request)
        .expect("repeated validation");
    assert_eq!(sequential_validate, parallel_validate);
    assert_eq!(parallel_validate, repeated_validate);
    assert_eq!(
        parallel_validate
            .diagnostics()
            .iter()
            .map(|diagnostic| diagnostic.code)
            .collect::<Vec<_>>(),
        vec![DiagCode::ParenthesizedTuple, DiagCode::IllegalBracketIndex]
    );

    let sequential_format = driver
        .format(&sequential_request)
        .expect("sequential formatting");
    let parallel_format = driver
        .format(&parallel_request)
        .expect("parallel formatting");
    let repeated_format = driver
        .format(&parallel_request)
        .expect("repeated formatting");
    assert_eq!(sequential_format, parallel_format);
    assert_eq!(parallel_format, repeated_format);
    assert_eq!(
        parallel_format
            .formatted()
            .iter()
            .map(|source| source.text())
            .collect::<Vec<_>>(),
        vec![FORMATTED_PARITY_QUERY, FORMATTED_INDEX_QUERY]
    );
}

#[test]
fn lint_results_are_identical_sequentially_in_parallel_and_on_repeat() {
    let driver = AnalysisDriver;
    let sequential = driver
        .lint(&lint_request(SEQUENTIAL_JOBS))
        .expect("sequential lint");
    let parallel = driver
        .lint(&lint_request(PARALLEL_JOBS))
        .expect("parallel lint");
    let repeated = driver
        .lint(&lint_request(PARALLEL_JOBS))
        .expect("repeated lint");

    assert_eq!(sequential, parallel);
    assert_eq!(parallel, repeated);
    assert_eq!(
        parallel
            .sources()
            .get(FileId::new(1))
            .expect("first source retained")
            .name(),
        "first.pure"
    );
    assert_eq!(parallel.diagnostics()[0].code, DiagCode::UnknownProperty);
    assert_eq!(parallel.diagnostics()[0].primary.file, FileId::new(1));
}

#[test]
fn lint_matches_equivalent_file_and_memory_snapshots() {
    let driver = AnalysisDriver;
    let query = "model::Person.all()->filter(x| $x.missing)";
    let model_file = FileFixture::new("model.json", MODEL);
    let query_file = FileFixture::new("query.pure", query);
    let in_memory = driver
        .lint(&LintRequest::new(
            SourceRequest::new([SourceInput::in_memory("query.pure", query)]),
            [ModelInput::pmcd(SourceInput::in_memory(
                "model.json",
                MODEL,
            ))],
        ))
        .expect("lint in-memory request");
    let from_files = driver
        .lint(&LintRequest::new(
            SourceRequest::new([SourceInput::file(&query_file.path)]),
            [ModelInput::pmcd(SourceInput::file(&model_file.path))],
        ))
        .expect("lint file request");

    assert_eq!(in_memory.diagnostics(), from_files.diagnostics());
    assert_eq!(
        from_files
            .sources()
            .get(FileId::new(1))
            .expect("query file retained")
            .text(),
        query
    );
}
