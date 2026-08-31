//! Renderer-independent orchestration over retained source snapshots.

use pure_analyzer_analysis::{
    AnalysisEngine, AnalysisInput, AnalysisPass, FindingPolicy, MilestoningArityLintPass,
    NavigationLintPass, ValidatePass, format_query, format_query_with_width,
};
use std::collections::{BTreeMap, BTreeSet};

use pure_analyzer_diagnostics::{
    ALL_DIAG_CODES, DiagCode, Diagnostic, FileId, FixPlan, FixPlanError, PlannedFile, Severity,
};
use pure_analyzer_model::{
    ModelDocument, ModelError, ModelGraph, PmcdDocument, PureDocument, load_model_documents,
};
use pure_analyzer_parser::parse_query;
use pure_analyzer_syntax::{BuildError, GreenNode};
use rayon::prelude::*;
use thiserror::Error;

use crate::{SourceFile, SourceInput, SourceStore, SourceStoreError};

const DEFAULT_JOBS: usize = 1;

/// Renderer-neutral selection and severity policy for analyzer findings.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DiagnosticPolicy {
    selected: Option<BTreeSet<DiagCode>>,
    ignored: BTreeSet<DiagCode>,
    severity_overrides: BTreeMap<DiagCode, Severity>,
    warnings_as_errors: bool,
}

impl DiagnosticPolicy {
    /// Construct a policy that retains every warning and error unchanged.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Restrict findings to the supplied registered code.
    ///
    /// Calling this method more than once grows the selected set. A policy
    /// with no selected codes retains every registered code.
    #[must_use]
    pub fn select(mut self, code: DiagCode) -> Self {
        self.selected.get_or_insert_with(BTreeSet::new).insert(code);
        self
    }

    /// Suppress findings with the supplied registered code.
    #[must_use]
    pub fn ignore(mut self, code: DiagCode) -> Self {
        self.ignored.insert(code);
        self
    }

    /// Override the presentation severity for the supplied registered code.
    #[must_use]
    pub fn with_severity(mut self, code: DiagCode, severity: Severity) -> Self {
        self.severity_overrides.insert(code, severity);
        self
    }

    /// Promote default warnings before applying exact-code overrides.
    #[must_use]
    pub const fn with_warnings_as_errors(mut self, enabled: bool) -> Self {
        self.warnings_as_errors = enabled;
        self
    }

    fn finding_policy(&self) -> FindingPolicy {
        let mut policy = FindingPolicy::new().with_warnings_as_errors(self.warnings_as_errors);
        if let Some(selected) = &self.selected {
            for &code in ALL_DIAG_CODES {
                if !selected.contains(&code) {
                    policy = policy.suppress(code);
                }
            }
        }
        for &code in &self.ignored {
            policy = policy.suppress(code);
        }
        for (&code, &severity) in &self.severity_overrides {
            policy = policy.with_severity(code, severity);
        }
        policy
    }
}

/// Source inputs and execution policy shared by parse, validate, and format.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceRequest {
    sources: Vec<SourceInput>,
    jobs: usize,
    policy: DiagnosticPolicy,
    line_width: Option<usize>,
}

impl SourceRequest {
    /// Construct a request that executes deterministically on one worker.
    #[must_use]
    pub fn new(sources: impl IntoIterator<Item = SourceInput>) -> Self {
        Self {
            sources: sources.into_iter().collect(),
            jobs: DEFAULT_JOBS,
            policy: DiagnosticPolicy::new(),
            line_width: None,
        }
    }

    /// Set the maximum number of independent source files to analyze at once.
    ///
    /// A value of zero is rejected as a [`RequestError`] when the driver runs.
    #[must_use]
    pub const fn with_jobs(mut self, jobs: usize) -> Self {
        self.jobs = jobs;
        self
    }

    /// Return the input descriptions in stable request order.
    #[must_use]
    pub fn sources(&self) -> &[SourceInput] {
        &self.sources
    }

    /// Return the requested maximum parallelism.
    #[must_use]
    pub const fn jobs(&self) -> usize {
        self.jobs
    }

    /// Set the finding selection and severity policy used by analysis and formatting methods.
    #[must_use]
    pub fn with_diagnostic_policy(mut self, policy: DiagnosticPolicy) -> Self {
        self.policy = policy;
        self
    }

    /// Return the finding selection and severity policy.
    #[must_use]
    pub const fn diagnostic_policy(&self) -> &DiagnosticPolicy {
        &self.policy
    }

    /// Set the preferred maximum line width used when formatting.
    #[must_use]
    pub const fn with_line_width(mut self, line_width: usize) -> Self {
        self.line_width = Some(line_width);
        self
    }

    /// Return the preferred formatting line width, when one was requested.
    #[must_use]
    pub const fn line_width(&self) -> Option<usize> {
        self.line_width
    }

    fn validate(&self) -> Result<(), RequestError> {
        if self.sources.is_empty() {
            return Err(RequestError::NoSources);
        }
        if self.jobs == 0 {
            return Err(RequestError::ZeroJobs);
        }
        if self.line_width == Some(0) {
            return Err(RequestError::ZeroLineWidth);
        }
        Ok(())
    }
}

/// One model source supplied to [`LintRequest`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelInput {
    /// A PMCD JSON model document.
    Pmcd {
        /// The source snapshot or path containing PMCD JSON.
        source: SourceInput,
    },
    /// A Pure Domain model document.
    Pure {
        /// The source snapshot or path containing Pure Domain source.
        source: SourceInput,
    },
}

impl ModelInput {
    /// Construct a PMCD JSON model input.
    #[must_use]
    pub const fn pmcd(source: SourceInput) -> Self {
        Self::Pmcd { source }
    }

    /// Construct a Pure Domain model input.
    #[must_use]
    pub const fn pure(source: SourceInput) -> Self {
        Self::Pure { source }
    }

    fn source(&self) -> &SourceInput {
        match self {
            Self::Pmcd { source } | Self::Pure { source } => source,
        }
    }
}

/// The source and model inputs required for model-aware linting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LintRequest {
    sources: SourceRequest,
    models: Vec<ModelInput>,
}

impl LintRequest {
    /// Construct a lint request with optional model inputs.
    #[must_use]
    pub fn new(sources: SourceRequest, models: impl IntoIterator<Item = ModelInput>) -> Self {
        Self {
            sources,
            models: models.into_iter().collect(),
        }
    }

    /// Return the query-source request.
    #[must_use]
    pub const fn sources(&self) -> &SourceRequest {
        &self.sources
    }

    /// Return model inputs in their deterministic loading order.
    #[must_use]
    pub fn models(&self) -> &[ModelInput] {
        &self.models
    }
}

/// One lossless syntax tree and recovery findings produced for a source file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedSource {
    file: FileId,
    syntax: GreenNode,
    diagnostics: Vec<Diagnostic>,
}

impl ParsedSource {
    /// Return the request-local identity of the parsed source.
    #[must_use]
    pub const fn file(&self) -> FileId {
        self.file
    }

    /// Return the immutable lossless syntax tree for this exact snapshot.
    #[must_use]
    pub const fn syntax(&self) -> &GreenNode {
        &self.syntax
    }

    /// Return lexer and parser recovery diagnostics for this source.
    #[must_use]
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }
}

/// Parse results retaining source snapshots, lossless trees, and findings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseOutput {
    sources: SourceStore,
    parsed: Vec<ParsedSource>,
    diagnostics: Vec<Diagnostic>,
}

impl ParseOutput {
    /// Return the source snapshots referenced by parsed trees and diagnostics.
    #[must_use]
    pub const fn sources(&self) -> &SourceStore {
        &self.sources
    }

    /// Return lossless parse results in stable source-request order.
    #[must_use]
    pub fn parsed(&self) -> &[ParsedSource] {
        &self.parsed
    }

    /// Return all parser recovery diagnostics in stable source-request order.
    #[must_use]
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    /// Consume this output into its owned source store and parsed files.
    #[must_use]
    pub fn into_parts(self) -> (SourceStore, Vec<ParsedSource>) {
        (self.sources, self.parsed)
    }
}

/// A validation or lint result over one immutable source store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnalysisOutput {
    sources: SourceStore,
    diagnostics: Vec<Diagnostic>,
}

impl AnalysisOutput {
    /// Return every retained source snapshot referenced by this output.
    #[must_use]
    pub const fn sources(&self) -> &SourceStore {
        &self.sources
    }

    /// Return deterministic diagnostics in source-request order.
    #[must_use]
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    /// Build an all-or-nothing plan from this output's exact source snapshots.
    ///
    /// # Errors
    ///
    /// Returns [`FixPlanError`] when selected automatic fixes are invalid,
    /// conflicting, stale, or unsupported by their applicability proof.
    pub fn plan_fixes(&self) -> Result<FixPlan, FixPlanError> {
        FixPlan::build(
            self.sources
                .files()
                .map(|source| PlannedFile::new(source.id(), source.text())),
            self.diagnostics.clone(),
        )
    }

    /// Consume this output into the source store and diagnostics it owns.
    #[must_use]
    pub fn into_parts(self) -> (SourceStore, Vec<Diagnostic>) {
        (self.sources, self.diagnostics)
    }
}

/// One formatted buffer produced without writing any path or standard stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormattedSource {
    file: FileId,
    text: String,
}

impl FormattedSource {
    /// Return the request-local file identity this formatted buffer replaces.
    #[must_use]
    pub const fn file(&self) -> FileId {
        self.file
    }

    /// Return the complete canonical formatting result.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }
}

/// A formatting result retaining both original snapshots and new buffers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormatOutput {
    sources: SourceStore,
    formatted: Vec<FormattedSource>,
    diagnostics: Vec<Diagnostic>,
    has_recovery_diagnostics: bool,
}

impl FormatOutput {
    /// Return original snapshots retained for diff, check, or atomic writes.
    #[must_use]
    pub const fn sources(&self) -> &SourceStore {
        &self.sources
    }

    /// Return formatted buffers in stable input order.
    #[must_use]
    pub fn formatted(&self) -> &[FormattedSource] {
        &self.formatted
    }

    /// Return policy-filtered parser diagnostics retained while formatting the
    /// same snapshots.
    #[must_use]
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    /// Return whether parsing produced recovery diagnostics before policy filtering.
    ///
    /// Front ends must preserve this signal when deciding whether formatting
    /// results may replace source files: a presentation policy can suppress or
    /// downgrade a diagnostic, but it cannot make recovery output safe to write.
    #[must_use]
    pub const fn has_recovery_diagnostics(&self) -> bool {
        self.has_recovery_diagnostics
    }

    /// Consume this output into original sources, formatted buffers, and findings.
    #[must_use]
    pub fn into_parts(self) -> (SourceStore, Vec<FormattedSource>, Vec<Diagnostic>) {
        (self.sources, self.formatted, self.diagnostics)
    }
}

/// The stable facade that drives parser, analysis, formatter, and fix planning.
#[derive(Debug, Clone, Copy, Default)]
pub struct AnalysisDriver;

impl AnalysisDriver {
    /// Parse sources and return parser recovery diagnostics without model loading.
    ///
    /// # Errors
    ///
    /// Returns a typed [`DriverError`] for invalid requests, source loading, or
    /// the parser's lossless-tree construction failure.
    pub fn parse(&self, request: &SourceRequest) -> Result<ParseOutput, DriverError> {
        let sources = load_source_request(request)?;
        let files = sources.files().map(SourceFile::id).collect::<Vec<_>>();
        let parsed = run_sources(&sources, &files, request.jobs(), parse_source)?;
        let diagnostics = parsed
            .iter()
            .flat_map(|result| result.diagnostics.iter().cloned())
            .collect();
        Ok(ParseOutput {
            sources,
            parsed,
            diagnostics,
        })
    }

    /// Validate model-free source syntax and targeted grammar constraints.
    ///
    /// # Errors
    ///
    /// Returns a typed [`DriverError`] for invalid requests, source loading, or
    /// the parser's lossless-tree construction failure.
    pub fn validate(&self, request: &SourceRequest) -> Result<AnalysisOutput, DriverError> {
        let sources = load_source_request(request)?;
        let files = sources.files().map(SourceFile::id).collect::<Vec<_>>();
        let diagnostics = flatten(run_sources(&sources, &files, request.jobs(), |source| {
            analyze_source(
                source,
                None,
                AnalysisKind::Validate,
                request.diagnostic_policy(),
            )
        })?);
        Ok(AnalysisOutput {
            sources,
            diagnostics,
        })
    }

    /// Lint sources using one immutable model graph loaded once per request.
    ///
    /// Model-free validation still runs when no model inputs were supplied.
    ///
    /// # Errors
    ///
    /// Returns a typed [`DriverError`] that distinguishes request, source,
    /// model-loading, and internal parser or worker-pool failures.
    pub fn lint(&self, request: &LintRequest) -> Result<AnalysisOutput, DriverError> {
        let (sources, files, model) = load_lint_request(request)?;
        let diagnostics = flatten(run_sources(
            &sources,
            &files,
            request.sources.jobs(),
            |source| {
                analyze_source(
                    source,
                    model.as_ref(),
                    AnalysisKind::Lint,
                    request.sources.diagnostic_policy(),
                )
            },
        )?);
        let finding_policy = request.sources.diagnostic_policy().finding_policy();
        let mut all_diagnostics = model.as_ref().map_or_else(Vec::new, |graph| {
            graph
                .diagnostics()
                .iter()
                .cloned()
                .filter_map(|diagnostic| finding_policy.apply(diagnostic))
                .collect()
        });
        all_diagnostics.extend(diagnostics);
        Ok(AnalysisOutput {
            sources,
            diagnostics: all_diagnostics,
        })
    }

    /// Format sources into buffers without rereading, printing, or writing them.
    ///
    /// # Errors
    ///
    /// Returns a typed [`DriverError`] for invalid requests, source loading, or
    /// the parser's lossless-tree construction failure.
    pub fn format(&self, request: &SourceRequest) -> Result<FormatOutput, DriverError> {
        let sources = load_source_request(request)?;
        let files = sources.files().map(SourceFile::id).collect::<Vec<_>>();
        let results = run_sources(&sources, &files, request.jobs(), |source| {
            format_source(source, request.line_width())
        })?;
        let has_recovery_diagnostics = results.iter().any(|result| !result.diagnostics.is_empty());
        let finding_policy = request.diagnostic_policy().finding_policy();
        let diagnostics = results
            .iter()
            .flat_map(|result| result.diagnostics.iter().cloned())
            .filter_map(|diagnostic| finding_policy.apply(diagnostic))
            .collect();
        let formatted = results.into_iter().map(FormatResult::into_source).collect();
        Ok(FormatOutput {
            sources,
            formatted,
            diagnostics,
            has_recovery_diagnostics,
        })
    }
}

/// A request error that a CLI or LSP client can present as a usage failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum RequestError {
    /// No query source was supplied.
    #[error("at least one source input is required")]
    NoSources,
    /// The requested worker count was zero.
    #[error("worker count must be at least one")]
    ZeroJobs,
    /// The requested formatter width was zero.
    #[error("formatter line width must be at least one")]
    ZeroLineWidth,
}

/// A typed failure from the analysis facade.
#[derive(Debug, Error)]
pub enum DriverError {
    /// The request shape is not actionable by a frontend.
    #[error("invalid analysis request: {source}")]
    Usage {
        /// The typed usage cause.
        #[source]
        source: RequestError,
    },
    /// A query source could not be retained before analysis.
    #[error("could not load query source: {source}")]
    SourceLoad {
        /// The source-retention failure.
        #[source]
        source: SourceStoreError,
    },
    /// A model source could not be retained before model loading.
    #[error("could not load model source: {source}")]
    ModelSourceLoad {
        /// The source-retention failure.
        #[source]
        source: SourceStoreError,
    },
    /// A retained model snapshot could not be normalized into a model graph.
    #[error("could not load model: {source}")]
    ModelLoad {
        /// The typed model-loading failure.
        #[source]
        source: ModelError,
    },
    /// The lossless parser could not represent a retained source snapshot.
    #[error("internal parser failure for file {file}: {source}")]
    Parse {
        /// The request-local source identity.
        file: FileId,
        /// The typed syntax-builder failure.
        #[source]
        source: BuildError,
    },
    /// A requested parallel worker pool could not be constructed.
    #[error("internal worker-pool failure: {source}")]
    WorkerPool {
        /// The worker-pool construction failure reported by Rayon.
        #[source]
        source: rayon::ThreadPoolBuildError,
    },
    /// An internal source-store invariant was not preserved.
    #[error("internal source-store invariant lost file {file}")]
    MissingSource {
        /// The file identity that was expected in the source store.
        file: FileId,
    },
}

#[derive(Debug, Clone, Copy)]
enum AnalysisKind {
    Validate,
    Lint,
}

#[derive(Debug)]
struct FormatResult {
    source: FormattedSource,
    diagnostics: Vec<Diagnostic>,
}

impl FormatResult {
    fn into_source(self) -> FormattedSource {
        self.source
    }
}

fn load_source_request(request: &SourceRequest) -> Result<SourceStore, DriverError> {
    request.validate().map_err(DriverError::usage)?;
    SourceStore::load(request.sources.iter().cloned()).map_err(DriverError::source_load)
}

fn load_lint_request(
    request: &LintRequest,
) -> Result<(SourceStore, Vec<FileId>, Option<ModelGraph>), DriverError> {
    request.sources.validate().map_err(DriverError::usage)?;
    let model_sources = SourceStore::load(request.models.iter().map(ModelInput::source).cloned())
        .map_err(DriverError::model_source_load)?;
    let first_query_id = u32::try_from(model_sources.len())
        .map_err(|_| DriverError::model_source_load(SourceStoreError::TooManySources))?;
    let query_sources =
        SourceStore::load_from(first_query_id, request.sources.sources.iter().cloned())
            .map_err(DriverError::source_load)?;
    let files = query_sources
        .files()
        .map(SourceFile::id)
        .collect::<Vec<_>>();
    let sources = model_sources.append(query_sources);
    let model = load_model(&sources, &request.models)?;
    Ok((sources, files, model))
}

fn load_model(
    sources: &SourceStore,
    inputs: &[ModelInput],
) -> Result<Option<ModelGraph>, DriverError> {
    if inputs.is_empty() {
        return Ok(None);
    }
    let mut documents = Vec::with_capacity(inputs.len());
    for (index, input) in inputs.iter().enumerate() {
        let file = source_at(sources, index)?;
        let document = match input {
            ModelInput::Pmcd { .. } => {
                ModelDocument::Pmcd(PmcdDocument::new(file.name(), file.text()))
            }
            ModelInput::Pure { .. } => {
                ModelDocument::Pure(PureDocument::new(file.name(), file.text()))
            }
        };
        documents.push(document);
    }
    load_model_documents(&documents)
        .map(Some)
        .map_err(DriverError::model_load)
}

fn source_at(sources: &SourceStore, index: usize) -> Result<&SourceFile, DriverError> {
    let file = u32::try_from(index)
        .map(FileId::new)
        .map_err(|_| DriverError::model_source_load(SourceStoreError::TooManySources))?;
    sources.get(file).ok_or(DriverError::MissingSource { file })
}

fn run_sources<T, F>(
    sources: &SourceStore,
    files: &[FileId],
    jobs: usize,
    run: F,
) -> Result<Vec<T>, DriverError>
where
    T: Send,
    F: Fn(&SourceFile) -> Result<T, DriverError> + Send + Sync,
{
    let files = files
        .iter()
        .map(|file| {
            sources
                .get(*file)
                .ok_or(DriverError::MissingSource { file: *file })
        })
        .collect::<Result<Vec<_>, _>>()?;
    if jobs == DEFAULT_JOBS {
        return files.iter().map(|file| run(file)).collect();
    }
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(jobs)
        .build()
        .map_err(DriverError::worker_pool)?;
    pool.install(|| files.par_iter().map(|file| run(file)).collect())
}

fn parse_source(source: &SourceFile) -> Result<ParsedSource, DriverError> {
    parse_query(source.text(), source.id())
        .map(|parsed| ParsedSource {
            file: source.id(),
            syntax: parsed.green,
            diagnostics: parsed.diagnostics,
        })
        .map_err(|error| DriverError::parse(source.id(), error))
}

fn analyze_source(
    source: &SourceFile,
    model: Option<&ModelGraph>,
    kind: AnalysisKind,
    policy: &DiagnosticPolicy,
) -> Result<Vec<Diagnostic>, DriverError> {
    let parsed = parse_query(source.text(), source.id())
        .map_err(|error| DriverError::parse(source.id(), error))?;
    let passes = passes(kind);
    let engine = AnalysisEngine::new(passes, policy.finding_policy());
    Ok(engine
        .analyze(AnalysisInput::new(
            source.id(),
            source.text(),
            &parsed.green,
            &parsed.diagnostics,
            model,
        ))
        .into_diagnostics())
}

fn passes(kind: AnalysisKind) -> Vec<Box<dyn AnalysisPass>> {
    match kind {
        AnalysisKind::Validate => vec![Box::new(ValidatePass)],
        AnalysisKind::Lint => vec![
            Box::new(ValidatePass),
            Box::new(NavigationLintPass),
            Box::new(MilestoningArityLintPass),
        ],
    }
}

fn format_source(
    source: &SourceFile,
    line_width: Option<usize>,
) -> Result<FormatResult, DriverError> {
    let formatted = line_width.map_or_else(
        || format_query(source.text(), source.id()),
        |line_width| format_query_with_width(source.text(), source.id(), line_width),
    );
    formatted
        .map(|formatted| {
            let (text, diagnostics) = formatted.into_parts();
            FormatResult {
                source: FormattedSource {
                    file: source.id(),
                    text,
                },
                diagnostics,
            }
        })
        .map_err(|error| DriverError::parse(source.id(), error))
}

fn flatten(diagnostics: Vec<Vec<Diagnostic>>) -> Vec<Diagnostic> {
    diagnostics.into_iter().flatten().collect()
}

impl DriverError {
    fn usage(source: RequestError) -> Self {
        Self::Usage { source }
    }

    fn source_load(source: SourceStoreError) -> Self {
        Self::SourceLoad { source }
    }

    fn model_source_load(source: SourceStoreError) -> Self {
        Self::ModelSourceLoad { source }
    }

    fn model_load(source: ModelError) -> Self {
        Self::ModelLoad { source }
    }

    fn parse(file: FileId, source: BuildError) -> Self {
        Self::Parse { file, source }
    }

    fn worker_pool(source: rayon::ThreadPoolBuildError) -> Self {
        Self::WorkerPool { source }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use pure_analyzer_diagnostics::DiagCode;

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
                "pure-analyzer-libpure-driver-{}-{counter}-{name}",
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

    fn query_request(jobs: usize) -> SourceRequest {
        SourceRequest::new([
            SourceInput::in_memory("first.pure", "model::Person.all()->filter(x| $x.missing)"),
            SourceInput::stdin("model::Person.all()->filter(x| $x.name)"),
        ])
        .with_jobs(jobs)
    }

    fn lint_request(jobs: usize) -> LintRequest {
        LintRequest::new(
            query_request(jobs),
            [ModelInput::pmcd(SourceInput::in_memory(
                "model.json",
                MODEL,
            ))],
        )
    }

    #[test]
    fn run_sources_with_default_jobs_executes_on_the_calling_thread() {
        let sources = SourceStore::load([SourceInput::in_memory("query.pure", "query()")])
            .expect("load source snapshot");
        let files = sources.files().map(SourceFile::id).collect::<Vec<_>>();
        let calling_thread = std::thread::current().id();

        let executing_threads = run_sources(&sources, &files, DEFAULT_JOBS, |_| {
            Ok(std::thread::current().id())
        })
        .expect("run source on the default execution path");

        assert_eq!(executing_threads, vec![calling_thread]);
    }

    #[test]
    fn lint_results_are_identical_sequentially_in_parallel_and_on_repeat() {
        let driver = AnalysisDriver;
        let sequential = driver
            .lint(&lint_request(DEFAULT_JOBS))
            .expect("sequential lint");
        let parallel = driver
            .lint(&lint_request(PARALLEL_JOBS))
            .expect("parallel lint");
        let repeated = driver
            .lint(&lint_request(PARALLEL_JOBS))
            .expect("repeated lint");

        assert_eq!(sequential, parallel);
        assert_eq!(parallel, repeated);
        assert_eq!(parallel.sources().len(), 3);
        assert_eq!(
            parallel
                .sources()
                .get(FileId::new(0))
                .expect("model source retained")
                .name(),
            "model.json"
        );
        assert_eq!(
            parallel.diagnostics()[0].code,
            DiagCode::UnknownProperty,
            "the lint must retain actionable model-aware findings"
        );
        assert_eq!(parallel.diagnostics()[0].primary.file, FileId::new(1));
    }

    #[test]
    fn file_and_memory_requests_produce_identical_lint_findings() {
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
            .expect("lint in-memory sources");
        let from_files = driver
            .lint(&LintRequest::new(
                SourceRequest::new([SourceInput::file(&query_file.path)]),
                [ModelInput::pmcd(SourceInput::file(&model_file.path))],
            ))
            .expect("lint filesystem sources");

        assert_eq!(in_memory.diagnostics(), from_files.diagnostics());
        assert_eq!(
            in_memory
                .sources()
                .get(FileId::new(1))
                .map(SourceFile::text),
            from_files
                .sources()
                .get(FileId::new(1))
                .map(SourceFile::text)
        );
    }

    #[test]
    fn parse_validate_and_format_retain_the_same_single_snapshot() {
        let driver = AnalysisDriver;
        let request = SourceRequest::new([SourceInput::stdin("(value, other)")]);

        let parsed = driver.parse(&request).expect("parse source");
        let validated = driver.validate(&request).expect("validate source");
        let formatted = driver.format(&request).expect("format source");

        assert_eq!(parsed.parsed().len(), 1);
        assert_eq!(parsed.parsed()[0].file(), FileId::new(0));
        assert!(parsed.parsed()[0].syntax().tokens().next().is_some());
        assert_eq!(
            parsed
                .sources()
                .get(FileId::new(0))
                .expect("stdin source retained")
                .text(),
            "(value, other)"
        );
        assert_eq!(validated.sources(), parsed.sources());
        assert_eq!(formatted.sources(), parsed.sources());
        assert_eq!(formatted.formatted()[0].text(), "(value, other)\n");
        assert!(
            validated
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.code == DiagCode::ParenthesizedTuple)
        );
    }

    #[test]
    fn format_request_applies_line_width_and_rejects_zero() {
        let driver = AnalysisDriver;
        let source = SourceInput::in_memory(
            "query.pure",
            "function(firstArgument,secondArgument,thirdArgument)",
        );
        let request = SourceRequest::new([source.clone()]).with_line_width(30);

        let formatted = driver.format(&request).expect("format narrow source");
        assert_eq!(request.line_width(), Some(30));
        assert_eq!(
            formatted.formatted()[0].text(),
            "function(firstArgument,\n        secondArgument,\n        thirdArgument)\n"
        );

        let error = driver
            .format(&SourceRequest::new([source]).with_line_width(0))
            .expect_err("reject zero line width");
        assert!(matches!(
            error,
            DriverError::Usage {
                source: RequestError::ZeroLineWidth
            }
        ));
    }

    #[test]
    fn lint_without_a_model_keeps_model_free_validation() {
        let driver = AnalysisDriver;
        let request = LintRequest::new(
            SourceRequest::new([SourceInput::in_memory("query.pure", "(value, other)")]),
            [],
        );

        let output = driver.lint(&request).expect("lint source without model");

        assert!(
            output
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.code == DiagCode::ParenthesizedTuple)
        );
        assert!(output.plan_fixes().expect("empty fix plan").is_empty());
    }

    #[test]
    fn diagnostic_policy_filters_and_reclassifies_without_changing_identity() {
        let driver = AnalysisDriver;
        let source =
            SourceInput::in_memory("query.pure", "model::Person.all()->filter(x| $x.missing)");
        let base = driver
            .lint(&LintRequest::new(
                SourceRequest::new([source.clone()]),
                [ModelInput::pmcd(SourceInput::in_memory(
                    "model.json",
                    MODEL,
                ))],
            ))
            .expect("lint with default policy");
        let warning = driver
            .lint(&LintRequest::new(
                SourceRequest::new([source]).with_diagnostic_policy(
                    DiagnosticPolicy::new()
                        .with_severity(DiagCode::UnknownProperty, Severity::Warning),
                ),
                [ModelInput::pmcd(SourceInput::in_memory(
                    "model.json",
                    MODEL,
                ))],
            ))
            .expect("lint with severity policy");

        let base_finding = base
            .diagnostics()
            .iter()
            .find(|diagnostic| diagnostic.code == DiagCode::UnknownProperty)
            .expect("default unknown-property finding");
        let warning_finding = warning
            .diagnostics()
            .iter()
            .find(|diagnostic| diagnostic.code == DiagCode::UnknownProperty)
            .expect("reclassified unknown-property finding");
        assert_eq!(base_finding.code, warning_finding.code);
        assert_eq!(base_finding.message, warning_finding.message);
        assert_eq!(base_finding.primary, warning_finding.primary);
        assert_eq!(warning_finding.severity, Severity::Warning);

        let ignored = driver
            .lint(&LintRequest::new(
                SourceRequest::new([SourceInput::in_memory(
                    "query.pure",
                    "model::Person.all()->filter(x| $x.missing)",
                )])
                .with_diagnostic_policy(DiagnosticPolicy::new().ignore(DiagCode::UnknownProperty)),
                [ModelInput::pmcd(SourceInput::in_memory(
                    "model.json",
                    MODEL,
                ))],
            ))
            .expect("lint with ignored code");
        assert!(
            ignored
                .diagnostics()
                .iter()
                .all(|diagnostic| diagnostic.code != DiagCode::UnknownProperty)
        );
    }

    #[test]
    fn diagnostic_policy_also_applies_to_model_loader_findings() {
        let request = |policy| {
            LintRequest::new(
                SourceRequest::new([SourceInput::in_memory("query.pure", "model::Person.all()")])
                    .with_diagnostic_policy(policy),
                [
                    ModelInput::pmcd(SourceInput::in_memory("first.json", MODEL)),
                    ModelInput::pmcd(SourceInput::in_memory("second.json", MODEL)),
                ],
            )
        };
        let driver = AnalysisDriver;
        let default = driver
            .lint(&request(DiagnosticPolicy::new()))
            .expect("lint duplicate model inputs");
        let ignored = driver
            .lint(&request(
                DiagnosticPolicy::new().ignore(DiagCode::ModelMergeConflict),
            ))
            .expect("lint duplicate models with ignored merge finding");

        assert!(
            default
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.code == DiagCode::ModelMergeConflict)
        );
        assert!(
            ignored
                .diagnostics()
                .iter()
                .all(|diagnostic| diagnostic.code != DiagCode::ModelMergeConflict)
        );
    }

    #[test]
    fn diagnostic_policy_is_deterministic_across_worker_counts() {
        let policy = DiagnosticPolicy::new()
            .select(DiagCode::UnknownProperty)
            .with_severity(DiagCode::UnknownProperty, Severity::Warning)
            .with_warnings_as_errors(true);
        let request = |jobs| {
            LintRequest::new(
                query_request(jobs).with_diagnostic_policy(policy.clone()),
                [ModelInput::pmcd(SourceInput::in_memory(
                    "model.json",
                    MODEL,
                ))],
            )
        };
        let driver = AnalysisDriver;
        let sequential = driver
            .lint(&request(DEFAULT_JOBS))
            .expect("sequential policy run");
        let parallel = driver
            .lint(&request(PARALLEL_JOBS))
            .expect("parallel policy run");

        assert_eq!(sequential, parallel);
        assert!(sequential.diagnostics().iter().all(|diagnostic| {
            diagnostic.code == DiagCode::UnknownProperty && diagnostic.severity == Severity::Warning
        }));
    }

    #[test]
    fn pure_model_inputs_share_the_same_driver_path_as_pmcd_models() {
        let driver = AnalysisDriver;
        let output = driver
            .lint(&LintRequest::new(
                SourceRequest::new([SourceInput::in_memory(
                    "query.pure",
                    "model::Person.all()->filter(x| $x.missing)",
                )]),
                [ModelInput::pure(SourceInput::in_memory(
                    "model.pure",
                    PURE_MODEL,
                ))],
            ))
            .expect("lint against Pure model");

        assert!(
            output
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.code == DiagCode::UnknownProperty)
        );
        assert_eq!(
            output
                .sources()
                .get(FileId::new(0))
                .expect("Pure model source retained")
                .name(),
            "model.pure"
        );
    }

    #[test]
    fn request_and_model_failures_keep_their_actionable_categories() {
        let driver = AnalysisDriver;
        let empty = SourceRequest::new([]);
        let malformed_model = LintRequest::new(
            SourceRequest::new([SourceInput::stdin("query()")]),
            [ModelInput::pmcd(SourceInput::in_memory("bad.json", "{"))],
        );

        assert!(matches!(
            driver.parse(&empty),
            Err(DriverError::Usage {
                source: RequestError::NoSources
            })
        ));
        assert!(matches!(
            driver.lint(&malformed_model),
            Err(DriverError::ModelLoad { .. })
        ));
    }

    #[test]
    fn worker_pool_failures_preserve_the_rayon_error_source() {
        let source = rayon::ThreadPoolBuilder::new()
            .spawn_handler(|_| Err(std::io::Error::other("fixture worker failure")))
            .build()
            .expect_err("fixture spawn handler must reject worker creation");
        let error = DriverError::worker_pool(source);

        assert!(
            std::error::Error::source(&error)
                .and_then(|source| source.downcast_ref::<rayon::ThreadPoolBuildError>())
                .is_some(),
            "the public driver error must retain Rayon's typed construction failure"
        );
    }
}
