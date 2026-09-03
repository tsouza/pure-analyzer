#![forbid(unsafe_code)]
#![deny(missing_docs)]

//! Command-line entry point for `pure-analyzer`.

mod config;
mod workflow;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
use config::{ConfigFlags, ConfigOverrides, ConfigResolver};
use tracing_subscriber::EnvFilter;
use workflow::{EXIT_SUCCESS, Failure};

/// Mechanical, standalone static analysis for Legend Pure.
#[derive(Debug, Parser)]
#[command(
    name = "pure-analyzer",
    version,
    about,
    long_about = None,
    after_long_help = "Configuration precedence, from lowest to highest: built-in defaults; the user config file; the nearest repository config (or --config); PURE_ANALYZER_* environment variables; command-line flags. --no-config disables only file layers. Use --print-config to inspect the complete versioned result."
)]
struct Cli {
    /// Configuration discovery and diagnostic policy.
    #[command(flatten)]
    config: ConfigFlags,
    /// The subcommand to run.
    #[command(subcommand)]
    command: Option<Command>,
}

/// The `pure-analyzer` subcommands.
#[derive(Debug, Subcommand)]
enum Command {
    /// Grammar + shallow well-formedness. Needs no model.
    Validate {
        /// Input files/globs; `-` reads one source from stdin.
        files: Vec<String>,
        /// Escalate shape-level warnings to errors.
        #[arg(long, conflicts_with = "no_strict")]
        strict: bool,
        /// Override configured strict validation.
        #[arg(long)]
        no_strict: bool,
    },
    /// Milestoning `%latest`-arity checking, unknown-property, cardinality
    /// misuse. Needs a model.
    Lint {
        /// Input files/globs; `-` reads one source from stdin.
        files: Vec<String>,
        /// PMCD JSON and/or Pure-model-file model sources; may repeat.
        #[arg(long)]
        model: Vec<String>,
        /// Apply machine-applicable fixes in place, one atomic file exchange at a time, where
        /// atomic path exchange is available.
        #[arg(long)]
        fix: bool,
        /// Check whether `--fix` would change any input without writing.
        #[arg(long, requires = "fix", conflicts_with_all = ["stdout", "diff"])]
        check: bool,
        /// Print one `--fix` preview to standard output without writing.
        #[arg(long, requires = "fix", conflicts_with_all = ["check", "diff"])]
        stdout: bool,
        /// Print a compact `--fix` diff without writing.
        #[arg(long, requires = "fix", conflicts_with_all = ["check", "stdout"])]
        diff: bool,
    },
    /// Lossless layout formatting with atomic, per-file in-place updates where atomic path
    /// exchange is available.
    #[command(
        after_long_help = "`fmt --canonical` emits a proven relational normal form to standard output without writing input. Exit status: 0 emitted; 2 indecisive."
    )]
    Fmt {
        /// Input files/globs; `-` reads one source from stdin.
        files: Vec<String>,
        /// Emit only a proven relational normal form. This never writes files and does not
        /// preserve source layout or comments.
        #[arg(
            long,
            conflicts_with_all = ["check", "stdout", "diff", "line_width"]
        )]
        canonical: bool,
        /// PMCD JSON and/or Pure-model-file model sources for `--canonical`; may repeat.
        #[arg(long, requires = "canonical")]
        model: Vec<String>,
        /// Check formatting without modifying files; exit non-zero if any file
        /// would change.
        #[arg(long, conflicts_with_all = ["stdout", "diff"])]
        check: bool,
        /// Print formatted content to standard output.
        #[arg(long, conflicts_with_all = ["check", "diff"])]
        stdout: bool,
        /// Print a compact before/after diff.
        #[arg(long, conflicts_with_all = ["check", "stdout"])]
        diff: bool,
        /// Preferred layout line width.
        #[arg(long)]
        line_width: Option<usize>,
    },
    /// Compare two relational queries for proven M4a equivalence.
    #[command(
        after_long_help = "Exit status: 0 equivalent; 1 structurally not equivalent; 2 indecisive."
    )]
    Eq {
        /// First query input; a file, glob resolving to one file, or `-` for standard input.
        left: String,
        /// Second query input; a file, glob resolving to one file, or `-` for standard input.
        right: String,
        /// PMCD JSON and/or Pure-model-file model sources; may repeat.
        #[arg(long)]
        model: Vec<String>,
    },
    /// Compare two relational queries and report an M4a structural difference when proven.
    #[command(
        after_long_help = "Exit status: 0 equivalent; 1 structurally not equivalent; 2 indecisive."
    )]
    Diff {
        /// First query input; a file, glob resolving to one file, or `-` for standard input.
        left: String,
        /// Second query input; a file, glob resolving to one file, or `-` for standard input.
        right: String,
        /// PMCD JSON and/or Pure-model-file model sources; may repeat.
        #[arg(long)]
        model: Vec<String>,
    },
    /// Explain one exact registered diagnostic or reason identifier.
    Explain {
        /// Exact registered diagnostic (`PUR<nnnn>`) or reason identifier.
        identifier: String,
    },
    /// Generate deterministic shell completion code.
    Completions {
        /// Shell whose completion code should be emitted.
        #[arg(value_enum)]
        shell: CompletionShell,
    },
}

/// Shells supported by the dependency-free v0.1 completion generator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum CompletionShell {
    /// Bourne Again Shell completion function.
    Bash,
}

fn main() -> ExitCode {
    init_tracing();
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => {
            let code = if error.use_stderr() {
                workflow::EXIT_USAGE
            } else {
                EXIT_SUCCESS
            };
            let _ = error.print();
            return ExitCode::from(code);
        }
    };

    match run(cli) {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            tracing::debug!(exit_code = error.exit_code(), error = %error, "command failed");
            eprintln!("error: {error}");
            ExitCode::from(error.exit_code())
        }
    }
}

fn run(cli: Cli) -> Result<u8, Failure> {
    tracing::debug!(command = ?cli.command, "dispatching subcommand");
    if !cli.config.print_requested()
        && let Some(Command::Completions { shell }) = &cli.command
    {
        return workflow::completions(*shell, Cli::command());
    }

    let resolved = ConfigResolver::from_process()
        .map_err(Failure::usage)?
        .resolve(&cli.config, command_overrides(&cli.command, &cli.config))
        .map_err(Failure::usage)?;
    tracing::debug!(
        jobs = resolved.jobs(),
        output_format = ?resolved.output_format(),
        color = ?resolved.color(),
        quiet = resolved.quiet(),
        validate_strict = resolved.validate_strict(),
        line_width = resolved.line_width(),
        model_count = resolved.model_paths().len(),
        "resolved configuration"
    );
    if cli.config.print_requested() {
        let text = resolved.to_toml().map_err(Failure::usage)?;
        workflow::write_stdout(&text)?;
        return Ok(EXIT_SUCCESS);
    }
    let command = cli
        .command
        .ok_or_else(|| Failure::usage("a subcommand or --print-config is required"))?;

    match command {
        Command::Validate { files, .. } => workflow::validate(&files, &resolved),
        Command::Lint {
            files,
            fix,
            check,
            stdout,
            diff,
            ..
        } => workflow::lint(
            &files,
            workflow::FixMode::new(fix, check, stdout, diff),
            &resolved,
        ),
        Command::Fmt {
            files,
            canonical: true,
            ..
        } => workflow::canonical_format(&files, &resolved),
        Command::Fmt {
            files,
            check,
            stdout,
            diff,
            canonical: false,
            ..
        } => workflow::format(
            &files,
            workflow::FormatMode::new(check, stdout, diff),
            &resolved,
        ),
        Command::Eq {
            left,
            right,
            model: _,
        }
        | Command::Diff {
            left,
            right,
            model: _,
        } => workflow::compare(&left, &right, &resolved),
        Command::Explain { identifier } => workflow::explain(&identifier, resolved.output_format()),
        Command::Completions { shell } => workflow::completions(shell, Cli::command()),
    }
}

fn command_overrides(command: &Option<Command>, flags: &ConfigFlags) -> ConfigOverrides {
    let strict = match command {
        Some(Command::Validate { strict: true, .. }) => Some(true),
        Some(Command::Validate {
            no_strict: true, ..
        }) => Some(false),
        _ => None,
    };
    let line_width = match command {
        Some(Command::Fmt { line_width, .. }) => *line_width,
        _ => None,
    };
    let models = match command {
        Some(
            Command::Lint { model, .. }
            | Command::Fmt {
                canonical: true,
                model,
                ..
            }
            | Command::Eq { model, .. }
            | Command::Diff { model, .. },
        ) => model.iter().map(PathBuf::from).collect(),
        _ => Vec::new(),
    };
    flags.overrides(strict, line_width, models)
}

/// Initialize the `tracing` subscriber, respecting `RUST_LOG`.
fn init_tracing() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,pure_analyzer_cli=info"));

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(true)
        .with_writer(std::io::stderr)
        .try_init()
        .ok();
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn validate_parses_files_and_strict_flag() {
        let cli =
            Cli::try_parse_from(["pure-analyzer", "validate", "--strict", "a.pure", "b.pure"])
                .expect("parses");
        match cli.command {
            Some(Command::Validate { files, strict, .. }) => {
                assert_eq!(files, vec!["a.pure", "b.pure"]);
                assert!(strict);
            }
            other => panic!("expected Validate, got {other:?}"),
        }
    }

    #[test]
    fn lint_parses_repeated_model_flags() {
        let cli = Cli::try_parse_from([
            "pure-analyzer",
            "lint",
            "q.pure",
            "--model",
            "a.json",
            "--model",
            "b.pure",
        ])
        .expect("parses");
        match cli.command {
            Some(Command::Lint { model, .. }) => assert_eq!(model, vec!["a.json", "b.pure"]),
            other => panic!("expected Lint, got {other:?}"),
        }
    }

    #[test]
    fn canonical_format_parses_models_and_rejects_layout_modes() {
        let cli = Cli::try_parse_from([
            "pure-analyzer",
            "fmt",
            "query.pure",
            "--canonical",
            "--model",
            "model.json",
            "--model",
            "domain.pure",
        ])
        .expect("parses canonical formatter mode");
        match cli.command {
            Some(Command::Fmt {
                canonical, model, ..
            }) => {
                assert!(canonical);
                assert_eq!(model, vec!["model.json", "domain.pure"]);
            }
            other => panic!("expected canonical Fmt, got {other:?}"),
        }

        for arguments in [
            vec![
                "pure-analyzer",
                "fmt",
                "query.pure",
                "--canonical",
                "--check",
            ],
            vec![
                "pure-analyzer",
                "fmt",
                "query.pure",
                "--canonical",
                "--stdout",
            ],
            vec![
                "pure-analyzer",
                "fmt",
                "query.pure",
                "--canonical",
                "--diff",
            ],
            vec![
                "pure-analyzer",
                "fmt",
                "query.pure",
                "--canonical",
                "--line-width",
                "80",
            ],
            vec![
                "pure-analyzer",
                "fmt",
                "query.pure",
                "--model",
                "model.json",
            ],
        ] {
            assert!(
                Cli::try_parse_from(arguments).is_err(),
                "canonical formatter accepted an incompatible invocation"
            );
        }
    }

    #[test]
    fn comparison_commands_parse_two_inputs_and_repeated_model_flags() {
        for command in ["eq", "diff"] {
            let cli = Cli::try_parse_from([
                "pure-analyzer",
                command,
                "left.pure",
                "right.pure",
                "--model",
                "model.json",
                "--model",
                "domain.pure",
            ])
            .expect("parses comparison command");
            match cli.command {
                Some(Command::Eq { left, right, model })
                | Some(Command::Diff { left, right, model }) => {
                    assert_eq!(left, "left.pure");
                    assert_eq!(right, "right.pure");
                    assert_eq!(model, vec!["model.json", "domain.pure"]);
                }
                other => panic!("expected comparison command, got {other:?}"),
            }
        }
    }

    #[test]
    fn lint_fix_modes_require_fix_and_are_mutually_exclusive() {
        let cli = Cli::try_parse_from(["pure-analyzer", "lint", "query.pure", "--fix", "--diff"])
            .expect("parses fix diff mode");
        match cli.command {
            Some(Command::Lint {
                fix,
                check,
                stdout,
                diff,
                ..
            }) => {
                assert!(fix);
                assert!(!check);
                assert!(!stdout);
                assert!(diff);
            }
            other => panic!("expected Lint, got {other:?}"),
        }

        assert!(Cli::try_parse_from(["pure-analyzer", "lint", "query.pure", "--check"]).is_err());
        assert!(
            Cli::try_parse_from([
                "pure-analyzer",
                "lint",
                "query.pure",
                "--fix",
                "--check",
                "--diff",
            ])
            .is_err()
        );
    }

    #[test]
    fn supported_commands_are_exact() {
        let mut command = Cli::command();
        command.build();
        let commands = command
            .get_subcommands()
            .filter(|subcommand| subcommand.get_name() != "help")
            .map(clap::Command::get_name)
            .collect::<Vec<_>>();
        assert_eq!(
            commands,
            [
                "validate",
                "lint",
                "fmt",
                "eq",
                "diff",
                "explain",
                "completions"
            ]
        );
    }

    #[test]
    fn config_flags_are_global_and_boolean_overrides_conflict() {
        let cli = Cli::try_parse_from([
            "pure-analyzer",
            "validate",
            "source.pure",
            "--jobs",
            "3",
            "--deny",
            "PUR2*",
        ])
        .expect("parse global config flags after the command");
        let _overrides = command_overrides(&cli.command, &cli.config);
        assert!(
            Cli::try_parse_from([
                "pure-analyzer",
                "validate",
                "--strict",
                "--no-strict",
                "source.pure",
            ])
            .is_err()
        );
    }

    #[test]
    fn print_config_does_not_require_a_subcommand() {
        let cli = Cli::try_parse_from(["pure-analyzer", "--print-config", "--no-config"])
            .expect("parse standalone print-config invocation");
        assert!(cli.command.is_none());
        assert!(cli.config.print_requested());
    }

    #[test]
    fn long_help_documents_configuration_precedence() {
        let help = Cli::command().render_long_help().to_string();
        assert!(help.contains("Configuration precedence, from lowest to highest"));
        assert!(help.contains("PURE_ANALYZER_* environment variables"));
        assert!(help.contains("--print-config"));
    }
}
