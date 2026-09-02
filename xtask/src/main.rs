#![forbid(unsafe_code)]

//! `xtask`: developer automation entry point.
//!
//! This is a plain Rust binary (the [cargo-xtask] pattern) that shells out to
//! the underlying toolchain so that CI and local workflows share exactly one
//! source of truth. The `justfile` delegates to these subcommands.
//!
//! [cargo-xtask]: https://github.com/matklad/cargo-xtask

mod explain_docs;
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
    /// Run the real-model harness, compile its output against Legend, and always tear down.
    TestRealModel,
    /// Verify the frozen Legend parser corpus; `--refresh` also checks its live oracle.
    ParserDifferential {
        /// Refresh from the exact version-pinned Legend grammar endpoint before replaying.
        #[arg(long)]
        refresh: bool,
    },
    /// Verify frozen analyzer semantic witnesses; `--refresh` also checks decisive rows live.
    AnalysisSemanticCorpus {
        /// Verify decisive witnesses against an exactly pinned Legend engine before replaying.
        #[arg(long)]
        refresh: bool,
    },
    /// Verify frozen M4a comparison evidence; `--refresh` also checks decisive rows live.
    AnalysisComparisonCorpus {
        /// Verify decisive witnesses against an exactly pinned Legend engine before replaying.
        #[arg(long)]
        refresh: bool,
    },
    /// Run the default and feature-gated mutation-test passes, unsharded.
    TestMutation,
    /// Run one shard of the workspace-wide mutation pass (CI matrix only).
    TestMutationShard {
        /// Zero-based shard index (matches the mutation planner matrix).
        index: u32,
        /// Total number of shards (matches the mutation planner matrix).
        total: u32,
    },
    /// Run one merge-base-diff-scoped workspace mutation shard (CI only).
    TestMutationDiffShard {
        /// Zero-based shard index.
        index: u32,
        /// Total number of shards.
        total: u32,
        /// Unified diff file generated from the verified merge base.
        diff: String,
    },
    /// Run the feature-gated FFI-boundary mutation pass (fast; never sharded).
    TestMutationFfi,
    /// Run the focused M3 parser mutation pass.
    TestMutationParser,
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
    /// Generate the tracked diagnostic and reason explain reference from the shared catalog.
    GenerateExplainDocs,
    /// Assert the tracked diagnostic and reason explain reference matches the shared catalog.
    CheckExplainDocs,
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
        Command::TestRealModel => tasks::test_real_model(),
        Command::ParserDifferential { refresh } => tasks::parser_differential(refresh),
        Command::AnalysisSemanticCorpus { refresh } => tasks::analysis_semantic_corpus(refresh),
        Command::AnalysisComparisonCorpus { refresh } => tasks::analysis_comparison_corpus(refresh),
        Command::TestMutation => tasks::test_mutation(),
        Command::TestMutationShard { index, total } => tasks::test_mutation_shard(index, total),
        Command::TestMutationDiffShard { index, total, diff } => {
            tasks::test_mutation_diff_shard(index, total, &diff)
        }
        Command::TestMutationFfi => tasks::test_mutation_ffi(),
        Command::TestMutationParser => tasks::test_mutation_parser(),
        Command::Coverage { html } => tasks::coverage(html),
        Command::ReleasePlzCheck => tasks::release_plz_check(),
        Command::CheckCoreDeplight => tasks::check_core_deplight(),
        Command::CheckDocFacts => tasks::check_doc_facts(),
        Command::GenerateExplainDocs => explain_docs::generate(),
        Command::CheckExplainDocs => explain_docs::check(),
        Command::CheckDocLinks => markdown::check_doc_links(),
        Command::PublicApi { bless } => tasks::public_api(bless),
        Command::NewFeature { name } => tasks::new_feature(&name),
        Command::VerifyLayering => tasks::verify_layering(),
        Command::VerifyLints => tasks::verify_lints(),
        Command::PurecardFuzzCi { secs } => tasks::purecard_fuzz_ci(secs),
    }
}
