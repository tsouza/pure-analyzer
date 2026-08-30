//! End-to-end facade tests for the libpure analysis driver.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use libpure::{
    AnalysisDriver, DiagCode, FileId, LintRequest, ModelInput, SourceInput, SourceRequest,
};

const PARALLEL_JOBS: usize = 2;
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

fn request(jobs: usize) -> LintRequest {
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
fn facade_produces_identical_sequential_parallel_and_repeated_results() {
    let driver = AnalysisDriver;
    let sequential = driver.lint(&request(1)).expect("sequential lint");
    let parallel = driver.lint(&request(PARALLEL_JOBS)).expect("parallel lint");
    let repeated = driver.lint(&request(PARALLEL_JOBS)).expect("repeated lint");

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
fn facade_lints_equivalent_file_and_in_memory_snapshots() {
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
