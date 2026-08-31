//! End-to-end facade tests for the libpure analysis driver.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use libpure::{
    AnalysisDriver, DiagCode, Diagnostic, DiagnosticPolicy, FileId, LineColumn, LintRequest,
    ModelInput, Severity, SourceFile, SourceInput, SourceOrigin, SourceRequest, SourceStore,
    TextSize,
};

const SEQUENTIAL_JOBS: usize = 1;
const PARALLEL_JOBS: usize = 2;
const PARITY_QUERY: &str = "(first, second)";
const INDEX_QUERY: &str = "$rows[$index]";
const FORMATTED_PARITY_QUERY: &str = "(first, second)\n";
const FORMATTED_INDEX_QUERY: &str = "$rows[$index]\n";
const RECOVERY_QUERY: &str = "[a,]";
const FORMATTED_RECOVERY_QUERY: &str = "[a,]\n";
const RECOVERY_DIAGNOSTIC_MESSAGE: &str = "expected an expression after `,`";
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
const PURE_MODEL: &str = r#"
Class model::Person
{
  name: String[0..1];
}
"#;

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

fn assert_recovery_diagnostic(diagnostic: &Diagnostic, file: FileId) {
    assert_eq!(diagnostic.code, DiagCode::MalformedSyntax);
    assert_eq!(diagnostic.message, RECOVERY_DIAGNOSTIC_MESSAGE);
    assert_eq!(diagnostic.primary.file, file);
    assert_eq!(usize::from(diagnostic.primary.span.start()), 3);
    assert_eq!(usize::from(diagnostic.primary.span.end()), 4);
}

fn source_request(jobs: usize) -> SourceRequest {
    SourceRequest::new([
        SourceInput::in_memory("tuple.pure", PARITY_QUERY),
        SourceInput::stdin(INDEX_QUERY),
    ])
    .with_jobs(jobs)
}

#[test]
fn source_request_exposes_complete_input_sequence_in_request_order() {
    let inputs = vec![
        SourceInput::in_memory("first.pure", "first()"),
        SourceInput::stdin("second()"),
    ];
    let request = SourceRequest::new(inputs.clone());

    assert_eq!(request.sources(), inputs.as_slice());
}

#[test]
fn source_request_exposes_configured_nonzero_jobs() {
    let default_request = SourceRequest::new([SourceInput::stdin("query()")]);
    let request = source_request(PARALLEL_JOBS);

    assert_eq!(default_request.jobs(), SEQUENTIAL_JOBS);
    assert_eq!(request.jobs(), PARALLEL_JOBS);
}

#[test]
fn lint_request_exposes_sources_and_models_in_supplied_order() {
    let sources =
        SourceRequest::new([SourceInput::stdin("model::Person.all()")]).with_jobs(PARALLEL_JOBS);
    let models = vec![
        ModelInput::pmcd(SourceInput::in_memory("model.json", MODEL)),
        ModelInput::pure(SourceInput::in_memory("model.pure", PURE_MODEL)),
    ];
    let request = LintRequest::new(sources.clone(), models.clone());

    assert_eq!(request.sources(), &sources);
    assert_eq!(request.models(), models.as_slice());
}

#[test]
fn source_store_reports_empty_and_nonempty_counts() {
    let empty = SourceStore::load([]).expect("load empty source store");
    let nonempty =
        SourceStore::load([SourceInput::stdin("query()")]).expect("load nonempty source store");

    assert!(empty.is_empty());
    assert_eq!(empty.len(), 0);
    assert!(!nonempty.is_empty());
    assert_eq!(nonempty.len(), 1);
}

#[test]
fn source_store_preserves_metadata_in_request_order() {
    let file_fixture = FileFixture::new("file.pure", "file()");
    let store = SourceStore::load([
        SourceInput::file(&file_fixture.path),
        SourceInput::in_memory("unicode.pure", "aé\nβ"),
        SourceInput::stdin("stdin()"),
    ])
    .expect("load retained source snapshots");

    let files = store.files().collect::<Vec<_>>();
    assert_eq!(files.len(), 3);
    assert_eq!(files[0].id(), FileId::new(0));
    assert_eq!(files[0].text(), "file()");
    assert!(matches!(
        files[0].origin(),
        SourceOrigin::File { path } if path == &file_fixture.path
    ));
    assert_eq!(files[1].id(), FileId::new(1));
    assert_eq!(files[1].name(), "unicode.pure");
    assert_eq!(files[1].origin(), &SourceOrigin::InMemory);
    assert_eq!(files[2].id(), FileId::new(2));
    assert_eq!(files[2].name(), "<stdin>");
    assert_eq!(files[2].origin(), &SourceOrigin::Stdin);
    assert!(store.get(FileId::new(3)).is_none());
}

#[test]
fn source_file_line_column_accepts_eof_and_rejects_beyond() {
    let unicode_source = "aé\nβ";
    let store = SourceStore::load([SourceInput::in_memory("unicode.pure", unicode_source)])
        .expect("load unicode source snapshot");
    let eof = u32::try_from(unicode_source.len()).expect("fixture fits TextSize");
    let source = store.get(FileId::new(0)).expect("unicode source retained");
    assert_eq!(
        source.line_column(TextSize::new(3)),
        Some(LineColumn { line: 1, column: 4 })
    );
    assert_eq!(
        source.line_column(TextSize::new(4)),
        Some(LineColumn { line: 2, column: 1 })
    );
    assert_eq!(source.line_column(TextSize::new(2)), None);
    assert_eq!(
        source.line_column(TextSize::new(eof)),
        Some(LineColumn { line: 2, column: 3 })
    );
    assert_eq!(source.line_column(TextSize::new(eof + 1)), None);
    assert!(store.get(FileId::new(3)).is_none());
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
fn parse_output_into_parts_retains_recovery_sources_and_diagnostics() {
    let driver = AnalysisDriver;
    let output = driver
        .parse(&SourceRequest::new([
            SourceInput::in_memory("valid.pure", PARITY_QUERY),
            SourceInput::in_memory("malformed.pure", RECOVERY_QUERY),
        ]))
        .expect("parse recovery-tolerant sources");
    let expected_diagnostics = output.diagnostics().to_vec();

    assert_eq!(expected_diagnostics.len(), 1);
    assert_recovery_diagnostic(&expected_diagnostics[0], FileId::new(1));

    let (sources, parsed) = output.into_parts();
    assert_eq!(
        sources.get(FileId::new(0)).map(SourceFile::text),
        Some(PARITY_QUERY)
    );
    assert_eq!(
        sources.get(FileId::new(1)).map(SourceFile::text),
        Some(RECOVERY_QUERY)
    );
    assert_eq!(parsed.len(), 2);
    assert_eq!(parsed[0].file(), FileId::new(0));
    assert_eq!(parsed[0].syntax().text(), PARITY_QUERY);
    assert!(parsed[0].diagnostics().is_empty());
    assert_eq!(parsed[1].file(), FileId::new(1));
    assert_eq!(parsed[1].syntax().text(), RECOVERY_QUERY);
    assert_eq!(parsed[1].diagnostics(), expected_diagnostics.as_slice());
}

#[test]
fn formatting_recovery_retains_meaningful_parser_diagnostics() {
    let driver = AnalysisDriver;
    let output = driver
        .format(&SourceRequest::new([
            SourceInput::in_memory("valid.pure", PARITY_QUERY),
            SourceInput::in_memory("malformed.pure", RECOVERY_QUERY),
        ]))
        .expect("format recovery-tolerant sources");
    let expected_diagnostics = output.diagnostics().to_vec();

    assert_eq!(
        output
            .formatted()
            .iter()
            .map(|source| (source.file(), source.text()))
            .collect::<Vec<_>>(),
        vec![
            (FileId::new(0), FORMATTED_PARITY_QUERY),
            (FileId::new(1), FORMATTED_RECOVERY_QUERY),
        ]
    );
    assert_eq!(expected_diagnostics.len(), 1);

    assert_recovery_diagnostic(&expected_diagnostics[0], FileId::new(1));

    let (sources, formatted, diagnostics) = output.into_parts();
    assert_eq!(
        sources.get(FileId::new(0)).map(SourceFile::text),
        Some(PARITY_QUERY)
    );
    assert_eq!(
        sources.get(FileId::new(1)).map(SourceFile::text),
        Some(RECOVERY_QUERY)
    );
    assert_eq!(
        formatted
            .iter()
            .map(|source| (source.file(), source.text()))
            .collect::<Vec<_>>(),
        vec![
            (FileId::new(0), FORMATTED_PARITY_QUERY),
            (FileId::new(1), FORMATTED_RECOVERY_QUERY),
        ]
    );
    assert_eq!(diagnostics, expected_diagnostics);
}

#[test]
fn formatting_policy_does_not_clear_the_raw_recovery_write_guard() {
    let driver = AnalysisDriver;
    let source = SourceInput::in_memory("broken.pure", "\0");
    let warned = driver
        .format(
            &SourceRequest::new([source.clone()]).with_diagnostic_policy(
                DiagnosticPolicy::new().with_severity(DiagCode::BadToken, Severity::Warning),
            ),
        )
        .expect("format recovery source with warning policy");

    assert!(warned.has_recovery_diagnostics());
    assert_eq!(warned.diagnostics().len(), 1);
    assert_eq!(warned.diagnostics()[0].code, DiagCode::BadToken);
    assert_eq!(warned.diagnostics()[0].severity, Severity::Warning);

    let ignored = driver
        .format(
            &SourceRequest::new([source])
                .with_diagnostic_policy(DiagnosticPolicy::new().ignore(DiagCode::BadToken)),
        )
        .expect("format recovery source with ignore policy");

    assert!(ignored.has_recovery_diagnostics());
    assert!(ignored.diagnostics().is_empty());
}

#[test]
fn analysis_output_into_parts_retains_sources_and_diagnostics() {
    let driver = AnalysisDriver;
    let output = driver
        .validate(&SourceRequest::new([SourceInput::in_memory(
            "tuple.pure",
            PARITY_QUERY,
        )]))
        .expect("validate source");
    let expected_diagnostics = output.diagnostics().to_vec();

    assert_eq!(
        expected_diagnostics
            .iter()
            .map(|diagnostic| diagnostic.code)
            .collect::<Vec<_>>(),
        vec![DiagCode::ParenthesizedTuple]
    );

    let (sources, diagnostics) = output.into_parts();
    assert_eq!(
        sources.get(FileId::new(0)).map(SourceFile::text),
        Some(PARITY_QUERY)
    );
    assert_eq!(diagnostics, expected_diagnostics);
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
