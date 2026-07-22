#![forbid(unsafe_code)]

//! `xtask`: developer automation entry point.
//!
//! This is a plain Rust binary (the [cargo-xtask] pattern) that shells out to
//! the underlying toolchain so that CI and local workflows share exactly one
//! source of truth. The `justfile` delegates to these subcommands.
//!
//! [cargo-xtask]: https://github.com/matklad/cargo-xtask

mod process;
mod tasks;

use anyhow::Result;
use clap::{Parser, Subcommand};

/// Developer automation task runner.
#[derive(Debug, Parser)]
#[command(name = "xtask", about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

/// The set of automation subcommands.
#[derive(Debug, Subcommand)]
enum Command {
    /// Run the fast inner-loop gate: layering → fmt-check → lint → test.
    Ci,
    /// Run cargo-machete / dependency & formatting sweep to tidy the tree.
    Sweep,
    /// Produce a test-coverage report via cargo-llvm-cov.
    Coverage {
        /// Emit an HTML report in addition to the summary.
        #[arg(long)]
        html: bool,
    },
    /// Validate `release-plz.toml` against the actual workspace (config gate).
    ReleasePlzCheck,
    /// Snapshot / verify the public API surface via cargo-public-api (nightly).
    PublicApi {
        /// Update the committed baselines instead of checking against them.
        #[arg(long)]
        bless: bool,
    },
    /// Create an isolated git worktree + branch for a new feature.
    NewFeature {
        /// Feature name; becomes branch `feature/<name>`.
        name: String,
    },
    /// Scaffold a feature spec at `specs/<name>.md`.
    Spec {
        /// Feature name; becomes `specs/<name>.md`.
        name: String,
    },
    /// Verify the workspace layering: no layer may depend outward (any kind).
    VerifyLayering,
    /// Verify every crate inherits the workspace lints (forbid-unsafe / deny-missing-docs).
    VerifyLints,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Ci => tasks::ci(),
        Command::Sweep => tasks::sweep(),
        Command::Coverage { html } => tasks::coverage(html),
        Command::ReleasePlzCheck => tasks::release_plz_check(),
        Command::PublicApi { bless } => tasks::public_api(bless),
        Command::NewFeature { name } => tasks::new_feature(&name),
        Command::Spec { name } => tasks::spec(&name),
        Command::VerifyLayering => tasks::verify_layering(),
        Command::VerifyLints => tasks::verify_lints(),
    }
}
