#![forbid(unsafe_code)]

//! `xtask`: developer automation entry point.
//!
//! This is a plain Rust binary (the [cargo-xtask] pattern) that shells out to
//! the underlying toolchain so that CI and local workflows share exactly one
//! source of truth. The `justfile` delegates to these subcommands.
//!
//! [cargo-xtask]: https://github.com/matklad/cargo-xtask

mod markdown;
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
    /// Bring up the Legend stack, test only PureCARD, and always tear down.
    TestLegend,
    /// Run the default and feature-gated mutation-test passes.
    TestMutation,
    /// Produce a test-coverage report via cargo-llvm-cov.
    Coverage {
        /// Emit an HTML report in addition to the summary.
        #[arg(long)]
        html: bool,
    },
    /// Validate `release-plz.toml` against the actual workspace (config gate).
    ReleasePlzCheck,
    /// Assert PureCARD's non-optional runtime dependencies stay allowlisted.
    CheckCoreDeplight,
    /// Assert PureCARD's documented facts match their authoritative sources.
    CheckDocFacts,
    /// Check tracked Markdown relative files and GitHub-style heading anchors.
    CheckDocLinks,
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
    /// Verify analyzer layering and the analyzer/PureCARD product boundary.
    VerifyLayering,
    /// Verify every crate inherits the workspace lints (forbid-unsafe / deny-missing-docs).
    VerifyLints,
    /// Time-box every target in PureCARD's dedicated fuzz project.
    PurecardFuzzCi {
        /// Per-target time budget in seconds.
        secs: u64,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Ci => tasks::ci(),
        Command::Sweep => tasks::sweep(),
        Command::TestLegend => tasks::test_legend(),
        Command::TestMutation => tasks::test_mutation(),
        Command::Coverage { html } => tasks::coverage(html),
        Command::ReleasePlzCheck => tasks::release_plz_check(),
        Command::CheckCoreDeplight => tasks::check_core_deplight(),
        Command::CheckDocFacts => tasks::check_doc_facts(),
        Command::CheckDocLinks => markdown::check_doc_links(),
        Command::PublicApi { bless } => tasks::public_api(bless),
        Command::NewFeature { name } => tasks::new_feature(&name),
        Command::VerifyLayering => tasks::verify_layering(),
        Command::VerifyLints => tasks::verify_lints(),
        Command::PurecardFuzzCi { secs } => tasks::purecard_fuzz_ci(secs),
    }
}
