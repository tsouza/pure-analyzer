//! Process-boundary workflows over libpure and renderer-neutral outputs.

mod input;
mod write;

use std::collections::BTreeMap;
use std::fmt::Display;
use std::io::{IsTerminal, Write};

use libpure::{
    AnalysisDriver, AnalysisOutput, Diagnostic, DriverError, FormatOutput, LintRequest,
    PlannedChange, Severity, SourceInput, SourceOrigin, SourceRequest, SourceStore,
};
use pure_analyzer_render::{ColorPolicy, RenderInput, render_human, render_json, render_sarif};
use thiserror::Error;

use crate::CompletionShell;
use crate::config::{ColorChoice, OutputFormat, ResolvedConfig};
use input::{model_sources, query_sources};
use write::{Replacement, replace_all};

/// Successful command execution.
pub(crate) const EXIT_SUCCESS: u8 = 0;
/// Actionable diagnostics or unapplied formatting changes.
pub(crate) const EXIT_ACTIONABLE: u8 = 1;
/// Reserved for future indecisive equivalence results.
pub(crate) const EXIT_INDECISIVE: u8 = 2;
const _: u8 = EXIT_INDECISIVE;
/// Invalid invocation, inaccessible input, or unusable model.
pub(crate) const EXIT_USAGE: u8 = 3;
/// Analyzer, renderer, or output-commit invariant failure.
pub(crate) const EXIT_INTERNAL: u8 = 4;

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

/// Execute model-aware linting, optionally applying and rechecking safe fixes.
pub(crate) fn lint(files: &[String], fix: bool, config: &ResolvedConfig) -> Result<u8, Failure> {
    let sources = query_sources(files)?;
    let models = model_sources(config.model_paths())?;
    let request = || -> Result<LintRequest, Failure> {
        Ok(LintRequest::new(
            source_request(
                sources.clone(),
                config.jobs(),
                config.lint_policy().map_err(Failure::usage)?,
            ),
            models.clone(),
        ))
    };
    let driver = AnalysisDriver;
    let mut output = driver.lint(&request()?).map_err(driver_failure)?;
    if fix && apply_fixes(&output)? {
        output = driver.lint(&request()?).map_err(driver_failure)?;
    }
    emit_analysis(&output, config)?;
    Ok(diagnostic_exit(output.diagnostics()))
}

/// Execute canonical formatting in check, stdout, diff, or atomic-write mode.
pub(crate) fn format(
    files: &[String],
    mode: FormatMode,
    config: &ResolvedConfig,
) -> Result<u8, Failure> {
    let sources = query_sources(files)?;
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
    let changed = finish_format(&output, mode, !output.has_recovery_diagnostics())?;
    if has_errors || (changed && mode.previews_changes()) {
        Ok(EXIT_ACTIONABLE)
    } else {
        Ok(EXIT_SUCCESS)
    }
}

/// Write exact command output without mixing it with tracing or errors.
pub(crate) fn write_stdout(text: &str) -> Result<(), Failure> {
    let mut stdout = std::io::stdout().lock();
    stdout
        .write_all(text.as_bytes())
        .and_then(|()| stdout.flush())
        .map_err(|error| Failure::internal(format!("could not write standard output: {error}")))
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
    let subcommands = command
        .get_subcommands()
        .filter(|subcommand| subcommand.get_name() != "help")
        .map(|subcommand| subcommand.get_name())
        .collect::<Vec<_>>();
    let mut output = String::from(
        "# bash completion for pure-analyzer\n_pure_analyzer() {\n    local current command word words\n    current=\"${COMP_WORDS[COMP_CWORD]}\"\n    command=\"\"\n    for word in \"${COMP_WORDS[@]:1}\"; do\n        case \"$word\" in\n",
    );
    output.push_str("            ");
    output.push_str(&subcommands.join("|"));
    output.push_str(") command=\"$word\"; break ;;\n");
    output.push_str("        esac\n    done\n    case \"$command\" in\n");
    for subcommand in command
        .get_subcommands()
        .filter(|subcommand| subcommand.get_name() != "help")
    {
        let mut words = global.clone();
        words.extend(option_words(subcommand));
        words.sort();
        words.dedup();
        output.push_str("        ");
        output.push_str(subcommand.get_name());
        output.push_str(") words=\"");
        output.push_str(&words.join(" "));
        output.push_str("\" ;;\n");
    }
    let mut root_words = global;
    root_words.extend(subcommands.into_iter().map(str::to_owned));
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

fn has_actionable_diagnostics(diagnostics: &[Diagnostic]) -> bool {
    diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity == Severity::Error)
}

fn apply_fixes(output: &AnalysisOutput) -> Result<bool, Failure> {
    let plan = output.plan_fixes().map_err(Failure::internal)?;
    if plan.is_empty() {
        return Ok(false);
    }
    let snapshots = output
        .sources()
        .files()
        .map(|source| (source.id(), source.text().to_owned()))
        .collect::<BTreeMap<_, _>>();
    let changes = plan.preview(&snapshots).map_err(Failure::internal)?;
    let replacements = fix_replacements(output.sources(), changes)?;
    let changed = !replacements.is_empty();
    replace_all(replacements)?;
    Ok(changed)
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
                "automatic fixes cannot update {} from standard input",
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

fn finish_format(
    output: &FormatOutput,
    mode: FormatMode,
    permit_writes: bool,
) -> Result<bool, Failure> {
    let mut changed = false;
    let mut replacements = Vec::new();
    let mut rendered = String::new();
    for formatted in output.formatted() {
        let source = output.sources().get(formatted.file()).ok_or_else(|| {
            Failure::internal(format!("formatter lost source file {}", formatted.file()))
        })?;
        let stdin = matches!(source.origin(), SourceOrigin::Stdin);
        if source.text() == formatted.text() {
            if mode.stdout || (stdin && !mode.check && !mode.diff) {
                rendered.push_str(formatted.text());
            }
            continue;
        }
        changed = true;
        if mode.diff {
            append_diff(
                &mut rendered,
                source.name(),
                source.text(),
                formatted.text(),
            );
        } else if mode.stdout || (stdin && !mode.check) {
            rendered.push_str(formatted.text());
        } else if !mode.check && permit_writes {
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
        }
    }
    if !rendered.is_empty() {
        write_stdout(&rendered)?;
    }
    if permit_writes {
        replace_all(replacements)?;
    }
    Ok(changed)
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
    use libpure::{FileId, SourceInput};

    use super::*;
    use crate::Cli;

    #[test]
    fn exit_codes_keep_indecisive_reserved() {
        assert_eq!(EXIT_SUCCESS, 0);
        assert_eq!(EXIT_ACTIONABLE, 1);
        assert_eq!(EXIT_INDECISIVE, 2);
        assert_eq!(EXIT_USAGE, 3);
        assert_eq!(EXIT_INTERNAL, 4);
    }

    #[test]
    fn format_modes_identify_only_unapplied_previews() {
        assert!(!FormatMode::default().previews_changes());
        assert!(FormatMode::new(true, false, false).previews_changes());
        assert!(!FormatMode::new(false, true, false).previews_changes());
        assert!(FormatMode::new(false, false, true).previews_changes());
    }

    #[test]
    fn color_choices_map_to_the_renderer_policy() {
        assert_eq!(color_policy(ColorChoice::Auto), ColorPolicy::Auto);
        assert_eq!(color_policy(ColorChoice::Always), ColorPolicy::Always);
        assert_eq!(color_policy(ColorChoice::Never), ColorPolicy::Never);
    }

    #[test]
    fn query_fix_with_a_model_uses_the_retained_file_origin() {
        let root = std::env::temp_dir().join(format!(
            "pure-analyzer-fix-origin-{}-{}",
            std::process::id(),
            test_nonce()
        ));
        std::fs::create_dir_all(&root).expect("create fixture directory");
        let model = root.join("model.json");
        let query = root.join("query.pure");
        std::fs::write(&model, "{}").expect("write model fixture");
        std::fs::write(&query, "before").expect("write query fixture");
        let sources = SourceStore::load([SourceInput::file(&model), SourceInput::file(&query)])
            .expect("retain model and query sources");

        let replacements = fix_replacements(
            &sources,
            vec![PlannedChange {
                file: FileId::new(1),
                before: "before".to_owned(),
                after: "after".to_owned(),
            }],
        )
        .expect("map query fix after a model input");

        assert_eq!(replacements.len(), 1);
        assert_eq!(replacements[0].path, query);
        assert_eq!(replacements[0].after, "after");
        std::fs::remove_dir_all(root).expect("remove fixtures");
    }

    #[test]
    fn bash_completion_matches_the_checked_in_contract() {
        let mut command = Cli::command();
        command.build();
        assert_eq!(
            bash_completion(&command),
            include_str!("../../tests/golden/completions.bash")
        );
    }
}
