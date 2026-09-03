//! Process-boundary workflows over libpure and renderer-neutral outputs.

mod input;
mod write;

use std::collections::BTreeMap;
use std::fmt::Display;
use std::io::{IsTerminal, Write};

use libpure::{
    AnalysisDriver, AnalysisOutput, CanonicalEmissionOutcome, CanonicalEmissionOutput,
    CanonicalEmissionRequest, ComparisonOutcome, ComparisonOutput, ComparisonRequest, Diagnostic,
    DriverError, ExplainContent, FormatOutput, LintRequest, ModelInput, PlannedChange, Severity,
    SourceFile, SourceInput, SourceOrigin, SourceRequest, SourceStore,
};
use pure_analyzer_render::{
    CanonicalEmissionRenderInput, ColorPolicy, ComparisonRenderInput, RenderInput,
    render_canonical_emission_human, render_canonical_emission_json, render_comparison_human,
    render_comparison_json, render_human, render_json, render_sarif,
};
use thiserror::Error;

use crate::CompletionShell;
use crate::config::{ColorChoice, OutputFormat, ResolvedConfig};
use input::{comparison_sources, model_sources, query_sources};
use write::{Replacement, replace_all};

/// Successful command execution.
pub(crate) const EXIT_SUCCESS: u8 = 0;
/// Actionable diagnostics or unapplied formatting changes.
pub(crate) const EXIT_ACTIONABLE: u8 = 1;
/// Comparison could not make a sound M4a commitment.
pub(crate) const EXIT_INDECISIVE: u8 = 2;
/// Invalid invocation, inaccessible input, or unusable model.
pub(crate) const EXIT_USAGE: u8 = 3;
/// Analyzer, renderer, or output-commit invariant failure.
pub(crate) const EXIT_INTERNAL: u8 = 4;

const FMT_MIXED_INPUT_WRITE_UNAVAILABLE: &str =
    "fmt cannot combine standard input with in-place file writes; use --check, --stdout, or --diff";
const FIX_STDIN_WRITE_UNAVAILABLE: &str =
    "lint --fix cannot update standard input; use --fix --check, --fix --stdout, or --fix --diff";
const EXPLAIN_SARIF_UNSUPPORTED: &str = "explain supports only --format human or --format json";
const COMPARISON_SARIF_UNSUPPORTED: &str =
    "eq and diff support only --format human or --format json";
const CANONICAL_EMISSION_SARIF_UNSUPPORTED: &str =
    "fmt --canonical supports only --format human or --format json";
const CANONICAL_EMISSION_INPUT_COUNT: &str = "fmt --canonical requires exactly one resolved input";

/// A classified CLI boundary failure with a stable process exit code.
#[derive(Debug, Error)]
#[error("{message}")]
pub(crate) struct Failure {
    code: u8,
    message: String,
}

impl Failure {
    /// Construct an invocation or source-input failure.
    pub(crate) fn usage(error: impl Display) -> Self {
        Self {
            code: EXIT_USAGE,
            message: error.to_string(),
        }
    }

    /// Construct a model-path or model-loading failure.
    pub(crate) fn model(error: impl Display) -> Self {
        Self {
            code: EXIT_USAGE,
            message: error.to_string(),
        }
    }

    /// Construct an analyzer, renderer, or output-commit invariant failure.
    pub(crate) fn internal(error: impl Display) -> Self {
        Self {
            code: EXIT_INTERNAL,
            message: error.to_string(),
        }
    }

    /// Return this failure's unified process exit code.
    pub(crate) const fn exit_code(&self) -> u8 {
        self.code
    }
}

/// Mutually exclusive formatting output behavior selected by CLI flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct FormatMode {
    check: bool,
    stdout: bool,
    diff: bool,
}

impl FormatMode {
    /// Construct one formatting mode from clap-validated flags.
    pub(crate) const fn new(check: bool, stdout: bool, diff: bool) -> Self {
        Self {
            check,
            stdout,
            diff,
        }
    }

    const fn previews_changes(self) -> bool {
        self.check || self.diff
    }

    const fn requests_in_place_write(self) -> bool {
        !self.check && !self.stdout && !self.diff
    }
}

/// Fix behavior selected by the `lint --fix` flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct FixMode {
    enabled: bool,
    check: bool,
    stdout: bool,
    diff: bool,
}

impl FixMode {
    /// Construct one fix mode validated by clap.
    pub(crate) const fn new(enabled: bool, check: bool, stdout: bool, diff: bool) -> Self {
        Self {
            enabled,
            check,
            stdout,
            diff,
        }
    }

    const fn enabled(self) -> bool {
        self.enabled
    }

    const fn previews_changes(self) -> bool {
        self.check || self.diff
    }

    const fn writes_in_place(self) -> bool {
        self.enabled && !self.check && !self.stdout && !self.diff
    }
}

/// Execute model-free validation and render the retained analysis snapshot.
pub(crate) fn validate(files: &[String], config: &ResolvedConfig) -> Result<u8, Failure> {
    let sources = query_sources(files)?;
    let request = source_request(
        sources,
        config.jobs(),
        config.validation_policy().map_err(Failure::usage)?,
    );
    let output = AnalysisDriver.validate(&request).map_err(driver_failure)?;
    emit_analysis(&output, config)?;
    Ok(diagnostic_exit(output.diagnostics()))
}

/// Execute model-aware linting, optionally previewing or applying proven fixes.
pub(crate) fn lint(
    files: &[String],
    mode: FixMode,
    config: &ResolvedConfig,
) -> Result<u8, Failure> {
    let sources = query_sources(files)?;
    if mode.writes_in_place()
        && sources
            .iter()
            .any(|source| matches!(source, SourceInput::Stdin { .. }))
    {
        return Err(Failure::usage(FIX_STDIN_WRITE_UNAVAILABLE));
    }
    if mode.stdout && sources.len() != 1 {
        return Err(Failure::usage(
            "lint --fix --stdout requires exactly one resolved input",
        ));
    }
    let models = model_sources(config.model_paths())?;
    let request = LintRequest::new(
        source_request(
            sources,
            config.jobs(),
            config.lint_policy().map_err(Failure::usage)?,
        ),
        models,
    );
    let driver = AnalysisDriver;
    let output = driver.lint(&request).map_err(driver_failure)?;
    if !mode.enabled() {
        emit_analysis(&output, config)?;
        return Ok(diagnostic_exit(output.diagnostics()));
    }

    finish_lint_fixes(&driver, &request, &output, mode, config)
}

/// Compare two queries through the fail-closed M4a facade and render its exact outcome.
pub(crate) fn compare(left: &str, right: &str, config: &ResolvedConfig) -> Result<u8, Failure> {
    if config.output_format() == OutputFormat::Sarif {
        return Err(Failure::usage(COMPARISON_SARIF_UNSUPPORTED));
    }
    let [left, right] = comparison_sources(left, right)?;
    let models = model_sources(config.model_paths())?;
    let request = ComparisonRequest::new(left, right, models);
    let driver = AnalysisDriver;
    let output = driver.compare(&request).map_err(driver_failure)?;
    emit_comparison(&output, config)?;
    Ok(comparison_exit(output.outcome()))
}

/// Emit one proven canonical relational normal form without changing any source file.
pub(crate) fn canonical_format(files: &[String], config: &ResolvedConfig) -> Result<u8, Failure> {
    if config.output_format() == OutputFormat::Sarif {
        return Err(Failure::usage(CANONICAL_EMISSION_SARIF_UNSUPPORTED));
    }
    let sources = query_sources(files)?;
    let [source] = sources.as_slice() else {
        return Err(Failure::usage(CANONICAL_EMISSION_INPUT_COUNT));
    };
    let models = model_sources(config.model_paths())?;
    let request = CanonicalEmissionRequest::new(source.clone(), models);
    let driver = AnalysisDriver;
    let output = driver.emit_canonical(&request).map_err(driver_failure)?;
    emit_canonical_emission(&output, config)?;
    Ok(canonical_emission_exit(output.outcome()))
}

/// Execute lossless layout formatting, installing each default file input with its
/// own atomic, durable exchange.
pub(crate) fn format(
    files: &[String],
    mode: FormatMode,
    config: &ResolvedConfig,
) -> Result<u8, Failure> {
    let sources = query_sources(files)?;
    if mode.requests_in_place_write()
        && sources.len() > 1
        && sources
            .iter()
            .any(|source| matches!(source, SourceInput::Stdin { .. }))
    {
        return Err(Failure::usage(FMT_MIXED_INPUT_WRITE_UNAVAILABLE));
    }
    if mode.stdout && sources.len() != 1 {
        return Err(Failure::usage(
            "fmt --stdout requires exactly one resolved input",
        ));
    }
    let request = SourceRequest::new(sources)
        .with_jobs(config.jobs())
        .with_diagnostic_policy(config.format_policy().map_err(Failure::usage)?)
        .with_line_width(config.line_width());
    let output = AnalysisDriver.format(&request).map_err(driver_failure)?;
    let has_errors = has_actionable_diagnostics(output.diagnostics());
    if !config.quiet() && !output.diagnostics().is_empty() {
        emit_diagnostics(
            output.sources(),
            output.diagnostics(),
            config,
            Destination::Stderr,
        )?;
    }
    let formatted = finish_format(&output, mode, !output.has_recovery_diagnostics())?;
    if formatted.blocked_in_place_change
        && output.has_recovery_diagnostics()
        && output.diagnostics().is_empty()
    {
        // The write guard fires on the pre-policy recovery signal (see
        // `FormatOutput::has_recovery_diagnostics`), which a diagnostic policy
        // cannot clear by design. When that policy also filtered every
        // diagnostic that would have explained the block, surface it directly
        // so the command never exits non-zero with no output at all.
        write_stderr(&blocked_write_message(&formatted.blocked_files))?;
    }
    if has_errors
        || (formatted.changed && mode.previews_changes())
        || formatted.blocked_in_place_change
    {
        Ok(EXIT_ACTIONABLE)
    } else {
        Ok(EXIT_SUCCESS)
    }
}

fn blocked_write_message(blocked_files: &[String]) -> String {
    blocked_files
        .iter()
        .map(|file| format!("formatting blocked by suppressed recovery diagnostics in `{file}`\n"))
        .collect()
}

/// Write exact command output without mixing it with tracing or errors.
pub(crate) fn write_stdout(text: &str) -> Result<(), Failure> {
    let mut stdout = std::io::stdout().lock();
    stdout
        .write_all(text.as_bytes())
        .and_then(|()| stdout.flush())
        .map_err(|error| Failure::internal(format!("could not write standard output: {error}")))
}

/// Explain an exact registered diagnostic or reason identifier.
pub(crate) fn explain(identifier: &str, format: OutputFormat) -> Result<u8, Failure> {
    let content = libpure::explain(identifier).map_err(Failure::usage)?;
    let mut rendered = match format {
        OutputFormat::Human => render_explanation_human(content),
        OutputFormat::Json => serde_json::to_string_pretty(content).map_err(|error| {
            Failure::internal(format!("could not serialize explain content: {error}"))
        })?,
        OutputFormat::Sarif => return Err(Failure::usage(EXPLAIN_SARIF_UNSUPPORTED)),
    };
    rendered.push('\n');
    write_stdout(&rendered)?;
    Ok(EXIT_SUCCESS)
}

fn render_explanation_human(content: &ExplainContent) -> String {
    format!(
        "{} ({}, {})\n\nMeaning\n{}\n\nLimit\n{}\n\nRemedy\n{}\n\nDocumentation\n{}",
        content.identifier,
        content.kind.as_str(),
        content.classification.as_str(),
        content.meaning,
        content.limit,
        content.remedy,
        content.documentation_url,
    )
}

/// Generate one deterministic shell-completion program without reading config.
pub(crate) fn completions(
    shell: CompletionShell,
    mut command: clap::Command,
) -> Result<u8, Failure> {
    command.build();
    let text = match shell {
        CompletionShell::Bash => bash_completion(&command),
    };
    write_stdout(&text)?;
    Ok(EXIT_SUCCESS)
}

fn bash_completion(command: &clap::Command) -> String {
    let global = option_words(command);
    let visible_subcommands = command
        .get_subcommands()
        .filter(|subcommand| subcommand.get_name() != "help")
        .collect::<Vec<_>>();
    let subcommand_word_list = visible_subcommands
        .iter()
        .flat_map(|subcommand| subcommand_names(subcommand))
        .collect::<Vec<_>>();
    let mut output = String::from(
        "# bash completion for pure-analyzer\n_pure_analyzer() {\n    local current command word words\n    current=\"${COMP_WORDS[COMP_CWORD]}\"\n    command=\"\"\n    for word in \"${COMP_WORDS[@]:1}\"; do\n        case \"$word\" in\n",
    );
    output.push_str("            ");
    output.push_str(&subcommand_word_list.join("|"));
    output.push_str(") command=\"$word\"; break ;;\n");
    output.push_str("        esac\n    done\n    case \"$command\" in\n");
    for subcommand in &visible_subcommands {
        let mut words = global.clone();
        words.extend(option_words(subcommand));
        words.sort();
        words.dedup();
        output.push_str("        ");
        output.push_str(&subcommand_names(subcommand).join("|"));
        output.push_str(") words=\"");
        output.push_str(&words.join(" "));
        output.push_str("\" ;;\n");
    }
    let mut root_words = global;
    root_words.extend(subcommand_word_list);
    root_words.sort();
    root_words.dedup();
    output.push_str("        *) words=\"");
    output.push_str(&root_words.join(" "));
    output.push_str("\" ;;\n");
    output.push_str(
        "    esac\n    if [[ \"$current\" == -* ]]; then\n        COMPREPLY=( $(compgen -W \"$words\" -- \"$current\") )\n    else\n        COMPREPLY=( $(compgen -W \"$words\" -- \"$current\") $(compgen -f -- \"$current\") )\n    fi\n}\ncomplete -F _pure_analyzer pure-analyzer\n",
    );
    output
}

/// A subcommand's canonical name plus every alias it is also invocable as,
/// so shell completion offers the same names clap itself accepts.
fn subcommand_names(subcommand: &clap::Command) -> Vec<String> {
    std::iter::once(subcommand.get_name().to_owned())
        .chain(subcommand.get_all_aliases().map(str::to_owned))
        .collect()
}

fn option_words(command: &clap::Command) -> Vec<String> {
    command
        .get_arguments()
        .filter_map(|argument| argument.get_long())
        .map(|long| format!("--{long}"))
        .collect()
}

fn write_stderr(text: &str) -> Result<(), Failure> {
    let mut stderr = std::io::stderr().lock();
    stderr
        .write_all(text.as_bytes())
        .and_then(|()| stderr.flush())
        .map_err(|error| Failure::internal(format!("could not write standard error: {error}")))
}

fn source_request(
    sources: Vec<SourceInput>,
    jobs: usize,
    policy: libpure::DiagnosticPolicy,
) -> SourceRequest {
    SourceRequest::new(sources)
        .with_jobs(jobs)
        .with_diagnostic_policy(policy)
}

fn emit_analysis(output: &AnalysisOutput, config: &ResolvedConfig) -> Result<(), Failure> {
    if config.quiet() {
        return Ok(());
    }
    emit_diagnostics(
        output.sources(),
        output.diagnostics(),
        config,
        Destination::Stdout,
    )
}

fn emit_comparison(output: &ComparisonOutput, config: &ResolvedConfig) -> Result<(), Failure> {
    if config.quiet() {
        return Ok(());
    }
    let input = ComparisonRenderInput::new(output.sources(), output.outcome());
    let rendered = match config.output_format() {
        OutputFormat::Human => render_comparison_human(
            input,
            color_policy(config.color()).resolve(Destination::Stdout.is_terminal()),
        ),
        OutputFormat::Json => render_comparison_json(input),
        OutputFormat::Sarif => return Err(Failure::usage(COMPARISON_SARIF_UNSUPPORTED)),
    }
    .map_err(Failure::internal)?;
    write_stdout(&rendered)
}

fn emit_canonical_emission(
    output: &CanonicalEmissionOutput,
    config: &ResolvedConfig,
) -> Result<(), Failure> {
    if config.quiet() {
        return Ok(());
    }
    let input = CanonicalEmissionRenderInput::new(output.sources(), output.outcome());
    let rendered = match config.output_format() {
        OutputFormat::Human => render_canonical_emission_human(
            input,
            color_policy(config.color()).resolve(Destination::Stdout.is_terminal()),
        ),
        OutputFormat::Json => render_canonical_emission_json(input),
        OutputFormat::Sarif => return Err(Failure::usage(CANONICAL_EMISSION_SARIF_UNSUPPORTED)),
    }
    .map_err(Failure::internal)?;
    write_stdout(&rendered)
}

fn emit_fix_analysis(output: &AnalysisOutput, config: &ResolvedConfig) -> Result<(), Failure> {
    if config.quiet() {
        return Ok(());
    }
    emit_diagnostics(
        output.sources(),
        output.diagnostics(),
        config,
        Destination::Stderr,
    )
}

fn emit_diagnostics(
    sources: &SourceStore,
    diagnostics: &[Diagnostic],
    config: &ResolvedConfig,
    destination: Destination,
) -> Result<(), Failure> {
    let input = RenderInput::new(sources, diagnostics);
    let rendered = match config.output_format() {
        OutputFormat::Human => render_human(
            input,
            color_policy(config.color()).resolve(destination.is_terminal()),
        ),
        OutputFormat::Json => render_json(input),
        OutputFormat::Sarif => render_sarif(input),
    }
    .map_err(Failure::internal)?;
    match destination {
        Destination::Stdout => write_stdout(&rendered),
        Destination::Stderr => write_stderr(&rendered),
    }
}

#[derive(Debug, Clone, Copy)]
enum Destination {
    Stdout,
    Stderr,
}

impl Destination {
    fn is_terminal(self) -> bool {
        match self {
            Self::Stdout => std::io::stdout().is_terminal(),
            Self::Stderr => std::io::stderr().is_terminal(),
        }
    }
}

fn color_policy(choice: ColorChoice) -> ColorPolicy {
    match choice {
        ColorChoice::Auto => ColorPolicy::Auto,
        ColorChoice::Always => ColorPolicy::Always,
        ColorChoice::Never => ColorPolicy::Never,
    }
}

fn diagnostic_exit(diagnostics: &[Diagnostic]) -> u8 {
    if has_actionable_diagnostics(diagnostics) {
        EXIT_ACTIONABLE
    } else {
        EXIT_SUCCESS
    }
}

fn comparison_exit(outcome: &ComparisonOutcome) -> u8 {
    match outcome {
        ComparisonOutcome::Equivalent => EXIT_SUCCESS,
        ComparisonOutcome::NotEquivalent(_) => EXIT_ACTIONABLE,
        ComparisonOutcome::Indecisive(_) => EXIT_INDECISIVE,
    }
}

fn canonical_emission_exit(outcome: &CanonicalEmissionOutcome) -> u8 {
    match outcome {
        CanonicalEmissionOutcome::Emitted(_) => EXIT_SUCCESS,
        CanonicalEmissionOutcome::Indecisive(_) => EXIT_INDECISIVE,
    }
}

fn has_actionable_diagnostics(diagnostics: &[Diagnostic]) -> bool {
    diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity == Severity::Error)
}

fn finish_lint_fixes(
    driver: &AnalysisDriver,
    request: &LintRequest,
    output: &AnalysisOutput,
    mode: FixMode,
    config: &ResolvedConfig,
) -> Result<u8, Failure> {
    let changes = planned_fix_changes(output)?;

    if mode.diff {
        let mut rendered = String::new();
        for change in &changes {
            let source = output.sources().get(change.file).ok_or_else(|| {
                Failure::internal(format!("fix plan lost source file {}", change.file))
            })?;
            append_fix_diff(&mut rendered, source.name(), &change.before, &change.after);
        }
        if !rendered.is_empty() {
            write_stdout(&rendered)?;
        }
        emit_fix_analysis(output, config)?;
        return Ok(fix_preview_exit(output, mode, !changes.is_empty()));
    }

    if mode.stdout {
        let source = output
            .sources()
            .files()
            .nth(request.models().len())
            .ok_or_else(|| Failure::internal("lint lost its resolved input"))?;
        let preview_text = changes
            .iter()
            .find(|change| change.file == source.id())
            .map_or_else(|| source.text(), |change| change.after.as_str());
        let preview_request = lint_preview_request(request, output, &changes)?;
        let reanalyzed = driver.lint(&preview_request).map_err(driver_failure)?;
        write_stdout(preview_text)?;
        emit_fix_analysis(&reanalyzed, config)?;
        return Ok(diagnostic_exit(reanalyzed.diagnostics()));
    }

    if mode.check {
        emit_fix_analysis(output, config)?;
        return Ok(fix_preview_exit(output, mode, !changes.is_empty()));
    }

    if changes.is_empty() {
        emit_fix_analysis(output, config)?;
        return Ok(diagnostic_exit(output.diagnostics()));
    }

    replace_all(fix_replacements(output.sources(), changes)?)?;
    let reanalyzed = driver.lint(request).map_err(driver_failure)?;
    emit_fix_analysis(&reanalyzed, config)?;
    Ok(diagnostic_exit(reanalyzed.diagnostics()))
}

fn planned_fix_changes(output: &AnalysisOutput) -> Result<Vec<PlannedChange>, Failure> {
    let snapshots = output
        .sources()
        .files()
        .map(|source| (source.id(), source.text().to_owned()))
        .collect::<BTreeMap<_, _>>();
    let plan = output.plan_fixes().map_err(Failure::internal)?;
    if !plan.check(&snapshots).map_err(Failure::internal)? {
        return Ok(Vec::new());
    }
    plan.preview(&snapshots).map_err(Failure::internal)
}

fn lint_preview_request(
    request: &LintRequest,
    output: &AnalysisOutput,
    changes: &[PlannedChange],
) -> Result<LintRequest, Failure> {
    let replacements = changes
        .iter()
        .map(|change| (change.file, change.after.as_str()))
        .collect::<BTreeMap<_, _>>();
    let model_count = request.models().len();
    let expected_source_count = model_count
        .checked_add(request.sources().sources().len())
        .ok_or_else(|| Failure::internal("lint preview source count overflow"))?;
    let retained = output.sources().files().collect::<Vec<_>>();
    if retained.len() != expected_source_count {
        return Err(Failure::internal(
            "lint preview lost its original query-source boundary",
        ));
    }
    let models = request
        .models()
        .iter()
        .zip(&retained[..model_count])
        .map(|(model, source)| snapshot_model_input(model, source))
        .collect::<Vec<_>>();
    let sources = retained[model_count..]
        .iter()
        .map(|source| {
            let text = replacements
                .get(&source.id())
                .copied()
                .unwrap_or(source.text());
            snapshot_source_input(source, text)
        })
        .collect::<Vec<_>>();
    let source_request = SourceRequest::new(sources)
        .with_jobs(request.sources().jobs())
        .with_diagnostic_policy(request.sources().diagnostic_policy().clone());
    Ok(LintRequest::new(source_request, models))
}

fn snapshot_model_input(model: &ModelInput, source: &SourceFile) -> ModelInput {
    let snapshot = snapshot_source_input(source, source.text());
    match model {
        ModelInput::Pmcd { .. } => ModelInput::pmcd(snapshot),
        ModelInput::Pure { .. } => ModelInput::pure(snapshot),
    }
}

fn snapshot_source_input(source: &SourceFile, text: &str) -> SourceInput {
    match source.origin() {
        SourceOrigin::File { path } => SourceInput::file_snapshot(path.clone(), text),
        SourceOrigin::InMemory => SourceInput::in_memory(source.name(), text),
        SourceOrigin::Stdin => SourceInput::stdin(text),
    }
}

fn fix_replacements(
    sources: &SourceStore,
    changes: Vec<PlannedChange>,
) -> Result<Vec<Replacement>, Failure> {
    let mut replacements = Vec::with_capacity(changes.len());
    for change in changes {
        let source = sources.get(change.file).ok_or_else(|| {
            Failure::internal(format!("fix plan lost source file {}", change.file))
        })?;
        let SourceOrigin::File { path } = source.origin() else {
            return Err(Failure::usage(format!(
                "lint --fix cannot update {} from standard input",
                source.name()
            )));
        };
        replacements.push(Replacement {
            path: path.clone(),
            before: change.before,
            after: change.after,
        });
    }
    Ok(replacements)
}

fn fix_preview_exit(output: &AnalysisOutput, mode: FixMode, changed: bool) -> u8 {
    if has_actionable_diagnostics(output.diagnostics()) || (mode.previews_changes() && changed) {
        EXIT_ACTIONABLE
    } else {
        EXIT_SUCCESS
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct FinishedFormat {
    changed: bool,
    blocked_in_place_change: bool,
    blocked_files: Vec<String>,
}

fn finish_format(
    output: &FormatOutput,
    mode: FormatMode,
    writes_are_safe: bool,
) -> Result<FinishedFormat, Failure> {
    let mut finished = FinishedFormat::default();
    let mut replacements = Vec::new();
    let mut rendered = String::new();
    for formatted in output.formatted() {
        let source = output.sources().get(formatted.file()).ok_or_else(|| {
            Failure::internal(format!("formatter lost source file {}", formatted.file()))
        })?;
        if source.text() == formatted.text() {
            if mode.stdout || is_implicit_stdin_stdout(source, mode) {
                rendered.push_str(formatted.text());
            }
            continue;
        }
        finished.changed = true;
        if mode.diff {
            append_diff(
                &mut rendered,
                source.name(),
                source.text(),
                formatted.text(),
            );
        } else if mode.stdout || is_implicit_stdin_stdout(source, mode) {
            rendered.push_str(formatted.text());
        } else if mode.requests_in_place_write() && writes_are_safe {
            let SourceOrigin::File { path } = source.origin() else {
                return Err(Failure::internal(format!(
                    "formatter cannot persist non-file source {}",
                    source.name()
                )));
            };
            replacements.push(Replacement {
                path: path.clone(),
                before: source.text().to_owned(),
                after: formatted.text().to_owned(),
            });
        } else if mode.requests_in_place_write() {
            finished.blocked_in_place_change = true;
            finished.blocked_files.push(source.name().to_owned());
        }
    }
    if !rendered.is_empty() {
        write_stdout(&rendered)?;
    }
    if !replacements.is_empty() {
        replace_all(replacements)?;
    }
    Ok(finished)
}

fn is_implicit_stdin_stdout(source: &SourceFile, mode: FormatMode) -> bool {
    matches!(source.origin(), SourceOrigin::Stdin) && mode.requests_in_place_write()
}

fn append_diff(output: &mut String, path: &str, before: &str, after: &str) {
    output.push_str("--- ");
    output.push_str(path);
    output.push('\n');
    output.push_str("+++ ");
    output.push_str(path);
    output.push_str(" (formatted)\n");
    for line in before.lines() {
        output.push('-');
        output.push_str(line);
        output.push('\n');
    }
    for line in after.lines() {
        output.push('+');
        output.push_str(line);
        output.push('\n');
    }
}

fn append_fix_diff(output: &mut String, path: &str, before: &str, after: &str) {
    output.push_str("--- ");
    output.push_str(path);
    output.push('\n');
    output.push_str("+++ ");
    output.push_str(path);
    output.push_str(" (fixed)\n");
    for line in before.lines() {
        output.push('-');
        output.push_str(line);
        output.push('\n');
    }
    for line in after.lines() {
        output.push('+');
        output.push_str(line);
        output.push('\n');
    }
}

fn driver_failure(error: DriverError) -> Failure {
    match &error {
        DriverError::Usage { .. } | DriverError::SourceLoad { .. } => Failure::usage(error),
        DriverError::ModelSourceLoad { .. } | DriverError::ModelLoad { .. } => {
            Failure::model(error)
        }
        DriverError::Parse { .. }
        | DriverError::WorkerPool { .. }
        | DriverError::MissingSource { .. } => Failure::internal(error),
    }
}

#[cfg(test)]
fn test_nonce() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::*;
    use crate::Cli;

    #[test]
    fn comparison_exit_codes_are_stable() {
        assert_eq!(EXIT_SUCCESS, 0);
        assert_eq!(EXIT_ACTIONABLE, 1);
        assert_eq!(EXIT_INDECISIVE, 2);
        assert_eq!(EXIT_USAGE, 3);
        assert_eq!(EXIT_INTERNAL, 4);
    }

    #[test]
    fn format_modes_distinguish_read_only_previews_from_in_place_requests() {
        assert!(FormatMode::default().requests_in_place_write());
        assert!(!FormatMode::default().previews_changes());
        assert!(FormatMode::new(true, false, false).previews_changes());
        assert!(!FormatMode::new(false, true, false).previews_changes());
        assert!(FormatMode::new(false, false, true).previews_changes());
        assert!(!FormatMode::new(true, false, false).requests_in_place_write());
        assert!(!FormatMode::new(false, true, false).requests_in_place_write());
        assert!(!FormatMode::new(false, false, true).requests_in_place_write());
    }

    #[test]
    fn color_choices_map_to_the_renderer_policy() {
        assert_eq!(color_policy(ColorChoice::Auto), ColorPolicy::Auto);
        assert_eq!(color_policy(ColorChoice::Always), ColorPolicy::Always);
        assert_eq!(color_policy(ColorChoice::Never), ColorPolicy::Never);
    }

    #[test]
    fn bash_completion_matches_the_checked_in_contract() {
        let mut command = Cli::command();
        command.build();
        assert_eq!(
            bash_completion(&command),
            include_str!("../../tests/golden/completions.bash.golden")
        );
    }
}
