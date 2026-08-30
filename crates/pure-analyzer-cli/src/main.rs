#![forbid(unsafe_code)]
#![deny(missing_docs)]

//! Command-line entry point for `pure-analyzer`.

use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

/// Mechanical, standalone static analysis for Legend Pure.
#[derive(Debug, Parser)]
#[command(name = "pure-analyzer", version, about, long_about = None)]
struct Cli {
    /// The subcommand to run.
    #[command(subcommand)]
    command: Command,
}

/// The `pure-analyzer` subcommands.
#[derive(Debug, Subcommand)]
enum Command {
    /// Grammar + shallow well-formedness. Needs no model.
    Validate {
        /// Input files/globs; `-` reads one source from stdin.
        files: Vec<String>,
        /// Escalate shape-level warnings to errors.
        #[arg(long)]
        strict: bool,
    },
    /// Milestoning `%latest`-arity checking, unknown-property, cardinality
    /// misuse. Needs a model.
    Lint {
        /// Input files/globs; `-` reads one source from stdin.
        files: Vec<String>,
        /// PMCD JSON and/or Pure-model-file model sources; may repeat.
        #[arg(long)]
        model: Vec<String>,
        /// Apply `MachineApplicable` fixes in place.
        #[arg(long)]
        fix: bool,
    },
    /// Sound, incomplete, three-valued structural equivalence.
    Eq {
        /// The left-hand query file.
        left: String,
        /// The right-hand query file.
        right: String,
        /// PMCD JSON and/or Pure-model-file model sources; may repeat.
        #[arg(long)]
        model: Vec<String>,
    },
    /// `eq`, with diff-oriented rendering of the divergence.
    Diff {
        /// The left-hand query file.
        left: String,
        /// The right-hand query file.
        right: String,
        /// PMCD JSON and/or Pure-model-file model sources; may repeat.
        #[arg(long)]
        model: Vec<String>,
    },
    /// Canonical formatting.
    Fmt {
        /// Input files/globs; `-` reads one source from stdin.
        files: Vec<String>,
        /// Check formatting without writing; exit non-zero if any file would
        /// change.
        #[arg(long, conflicts_with_all = ["stdout", "diff"])]
        check: bool,
        /// Print formatted content to standard output instead of writing files.
        #[arg(long, conflicts_with_all = ["check", "diff"])]
        stdout: bool,
        /// Print a compact before/after diff instead of writing files.
        #[arg(long, conflicts_with_all = ["check", "stdout"])]
        diff: bool,
    },
    /// Print the `docs/reason-codes/<code>.md` page for a `PUR<nnnn>` code.
    Explain {
        /// The diagnostic code to explain, e.g. `PUR2001`.
        code: String,
    },
}

fn main() -> anyhow::Result<()> {
    init_tracing();
    let cli = Cli::parse();
    tracing::debug!(command = ?cli.command, "dispatching subcommand");

    match cli.command {
        Command::Validate { .. } => not_yet_implemented("validate"),
        Command::Lint { .. } => not_yet_implemented("lint"),
        Command::Eq { .. } => not_yet_implemented("eq"),
        Command::Diff { .. } => not_yet_implemented("diff"),
        Command::Fmt {
            files,
            check,
            stdout,
            diff,
        } => format_files(&files, check, stdout, diff),
        Command::Explain { code } => not_yet_implemented(&format!("explain {code}")),
    }
}

fn format_files(files: &[String], check: bool, stdout: bool, diff: bool) -> anyhow::Result<()> {
    if files.is_empty() {
        anyhow::bail!("fmt requires at least one file or - for standard input");
    }
    let mut changed = false;
    for (index, path) in files.iter().enumerate() {
        let source = if path == "-" {
            std::io::read_to_string(std::io::stdin())?
        } else {
            std::fs::read_to_string(path)?
        };
        let formatted = pure_analyzer_analysis::format_query(
            &source,
            pure_analyzer_diagnostics::FileId::new(index as u32),
        )?;
        let text = formatted.text();
        changed |= text != source;
        if stdout || path == "-" {
            print!("{text}");
        } else if diff && text != source {
            print_diff(path, &source, text);
        } else if !check && text != source {
            std::fs::write(path, text)?;
        }
    }
    if check && changed {
        anyhow::bail!("formatting changes required");
    }
    Ok(())
}

fn print_diff(path: &str, before: &str, after: &str) {
    println!("--- {path}");
    println!("+++ {path} (formatted)");
    for line in before.lines() {
        println!("-{line}");
    }
    for line in after.lines() {
        println!("+{line}");
    }
}

/// Report an unavailable `subcommand`.
///
/// # Errors
///
/// Always returns an error.
fn not_yet_implemented(subcommand: &str) -> anyhow::Result<()> {
    anyhow::bail!("`{subcommand}` is unavailable in this build")
}

/// Initialize the `tracing` subscriber, respecting `RUST_LOG`.
fn init_tracing() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,pure_analyzer_cli=info"));

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(true)
        .init();
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
            Command::Validate { files, strict } => {
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
            Command::Lint { model, .. } => assert_eq!(model, vec!["a.json", "b.pure"]),
            other => panic!("expected Lint, got {other:?}"),
        }
    }

    #[test]
    fn eq_requires_exactly_two_positional_files() {
        let err = Cli::try_parse_from(["pure-analyzer", "eq", "only-one.pure"])
            .expect_err("missing RIGHT should fail");
        assert_eq!(err.kind(), clap::error::ErrorKind::MissingRequiredArgument);
    }
}
