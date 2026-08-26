//! Implementations of each `xtask` subcommand.
//!
//! Each task shells out to the underlying tool via [`crate::process`] and
//! propagates exit codes, so `xtask` stays a thin, auditable orchestrator.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::process::{run, run_cargo_steps, run_stdout};

/// Reject empty names and path-escaping input (`/`, `\`, `..`) before it's
/// interpolated into a filesystem or worktree path.
fn validate_name(name: &str, usage: &str) -> Result<()> {
    if name.is_empty() {
        anyhow::bail!("usage: xtask {usage} <name>");
    }
    if name.contains(['/', '\\']) || name.contains("..") {
        anyhow::bail!("name must not contain path separators or '..'");
    }
    Ok(())
}

/// Full local CI pipeline, fail-fast: lint contract, then layering gate, then
/// format check, then lint, then test.
///
/// Mirrors the ordering used in the CI workflow so a green `xtask ci` locally
/// is a strong predictor of a green pipeline. [`verify_lints`] and
/// [`verify_layering`] run first because they are fast, offline manifest
/// parses and catch a missing lint or an architecture-breaking dependency edge
/// before the heavier compile-and-test steps.
pub fn ci() -> Result<()> {
    verify_lints()?;
    verify_layering()?;
    check_core_deplight()?;
    verify_purecard_fuzz_workspace()?;
    check_doc_facts()?;
    crate::markdown::check_doc_links()?;
    run_cargo_steps(&[
        &["fmt", "--all", "--check"],
        // PureCARD's always-on classifier binary is cfg(not(feature =
        // "legend")), so an all-features-only pass would compile it out and
        // leave that configuration without Clippy coverage.
        &[
            "clippy",
            "--workspace",
            "--all-targets",
            "--",
            "-D",
            "warnings",
        ],
        // clippy with --all-features is compile-checking, not execution — safe
        // and correct to check every feature combination actually builds.
        &[
            "clippy",
            "--workspace",
            "--all-targets",
            "--all-features",
            "--",
            "-D",
            "warnings",
        ],
        // No --all-features here, unlike clippy above: this *executes* tests,
        // and pure-analyzer-purecard's optional legend/qwen-oracle/fused-extract
        // features gate heavy, network-/env-dependent tests that must stay out
        // of the hermetic per-PR gate (each has its own on-demand `just`
        // target instead) — matching the pattern purecard's own CI already
        // established pre-migration. nextest also never runs doctests
        // (unlike `cargo test`), so the separate --doc --all-features step
        // below is the only place that coverage exists; that command is safe
        // with --all-features since doctests carry no heavy optional deps.
        &["nextest", "run", "--workspace"],
        &["test", "--workspace", "--doc", "--all-features"],
    ])
}

/// Run the structural / hygiene sweep: ast-grep guardrail rules over the tree.
///
/// The rules (banned constructs, architecture invariants) live under
/// `ast-grep-rules/` and are wired via `sgconfig.yml`. `ast-grep` is required
/// on PATH; a missing binary surfaces a clear error from
/// [`crate::process::run`].
pub fn sweep() -> Result<()> {
    // `sg scan` reads sgconfig.yml at the repo root and applies every rule.
    run("ast-grep", &["scan"])
}

/// Root of the migrated PureCARD workspace member.
const PURECARD_ROOT: &str = "crates/pure-analyzer-purecard";
/// Cargo package name of the migrated PureCARD workspace member.
const PURECARD_PACKAGE: &str = "pure-analyzer-purecard";
/// PureCARD's manifest, relative to the workspace root.
const PURECARD_MANIFEST: &str = "crates/pure-analyzer-purecard/Cargo.toml";
/// Docker Compose file for the pinned Legend engine stack.
const PURECARD_LEGEND_COMPOSE: &str =
    "crates/pure-analyzer-purecard/corpus/legend-stack/docker-compose.yml";
/// PureCARD's workspace-excluded cargo-fuzz project.
const PURECARD_FUZZ_DIR: &str = "crates/pure-analyzer-purecard/fuzz";
/// PureCARD fuzz manifest, explicitly isolated from the ancestor workspace.
const PURECARD_FUZZ_MANIFEST: &str = "crates/pure-analyzer-purecard/fuzz/Cargo.toml";
/// Every target in PureCARD's dedicated fuzz project.
const PURECARD_FUZZ_TARGETS: &[&str] = &["accept_token", "allowed_mask", "schema_from_json"];
/// Directory containing the source file for every registered PureCARD fuzz target.
const PURECARD_FUZZ_TARGET_DIR: &str = "crates/pure-analyzer-purecard/fuzz/fuzz_targets";
/// PureCARD's feature-gated FFI source, tested in a separate mutation pass.
const PURECARD_FFI_SOURCE: &str = "crates/pure-analyzer-purecard/src/ffi.rs";
/// Parent directory required by cargo-mutants before it creates its reports.
const MUTATION_OUTPUT_ROOT: &str = "target";

/// Resolve a path owned by the nested PureCARD crate.
fn purecard_path(relative: impl AsRef<Path>) -> PathBuf {
    Path::new(PURECARD_ROOT).join(relative)
}

/// Run the opt-in PureCARD Legend lane with guaranteed stack teardown.
///
/// The checked-in stack is brought up before package-scoped tests run. Teardown
/// is attempted after a failed startup as well as after tests, and the primary
/// startup or test error is retained if cleanup also fails.
///
/// # Errors
///
/// Returns the startup or test failure, or a teardown failure after successful
/// tests. If the primary operation and cleanup both fail, cleanup is attached
/// as context to the primary error.
pub fn test_legend() -> Result<()> {
    let started = run(
        "docker",
        &["compose", "-f", PURECARD_LEGEND_COMPOSE, "up", "-d"],
    );
    if let Err(start_err) = started {
        let torn_down = run(
            "docker",
            &["compose", "-f", PURECARD_LEGEND_COMPOSE, "down"],
        );
        return match torn_down {
            Ok(()) => Err(start_err),
            Err(teardown_err) => Err(start_err.context(format!(
                "PureCARD Legend stack startup failed and cleanup failed; containers may remain: \
                 {teardown_err:#}"
            ))),
        };
    }
    let tested = run(
        "cargo",
        &[
            "nextest",
            "run",
            "-p",
            PURECARD_PACKAGE,
            "--features",
            "legend",
        ],
    );
    let torn_down = run(
        "docker",
        &["compose", "-f", PURECARD_LEGEND_COMPOSE, "down"],
    );
    match (tested, torn_down) {
        (Err(test_err), Err(teardown_err)) => Err(test_err.context(format!(
            "PureCARD Legend tests failed and stack teardown failed; containers may remain: \
             {teardown_err:#}"
        ))),
        (Err(test_err), Ok(())) => Err(test_err),
        (Ok(()), teardown) => teardown,
    }
}

/// Run both mutation-test passes with portable output-directory preparation.
///
/// The default workspace pass excludes PureCARD's feature-gated FFI source;
/// the second pass enables `python-test` and targets that source explicitly so
/// neither surface can pass vacuously. Both run in place for parity with CI's
/// disposable checkout and the existing local workflow.
///
/// # Errors
///
/// Returns an error when the output parent cannot be created or either
/// cargo-mutants pass fails.
pub fn test_mutation() -> Result<()> {
    std::fs::create_dir_all(MUTATION_OUTPUT_ROOT)
        .context("creating mutation report output parent")?;
    run(
        "cargo",
        &[
            "mutants",
            "--workspace",
            "--exclude",
            PURECARD_FFI_SOURCE,
            "--in-place",
            "--output",
            "target/mutants-default",
        ],
    )?;
    run(
        "cargo",
        &[
            "mutants",
            "--package",
            PURECARD_PACKAGE,
            "--features",
            "python-test",
            "--file",
            PURECARD_FFI_SOURCE,
            "--in-place",
            "--output",
            "target/mutants-ffi",
            "--",
            "--lib",
        ],
    )
}

/// Time-box every target in PureCARD's dedicated cargo-fuzz project.
///
/// The explicit `--fuzz-dir` prevents cargo-fuzz from selecting the umbrella
/// analyzer's unrelated top-level fuzz project.
///
/// # Errors
///
/// Returns the first target failure, including a crash or missing nightly/tool.
pub fn purecard_fuzz_ci(secs: u64) -> Result<()> {
    let budget = format!("-max_total_time={secs}");
    for target in PURECARD_FUZZ_TARGETS {
        run(
            "cargo",
            &[
                "+nightly",
                "fuzz",
                "run",
                "--fuzz-dir",
                PURECARD_FUZZ_DIR,
                target,
                "--",
                &budget,
            ],
        )?;
    }
    Ok(())
}

/// PureCARD's permitted non-optional, normal runtime dependencies.
const CORE_DEP_ALLOWLIST: &[&str] = &["thiserror", "serde", "serde_json"];

/// Names of the non-optional normal dependencies in one metadata package.
fn non_optional_runtime_dependencies(package: &serde_json::Value) -> Result<BTreeSet<String>> {
    let dependencies = package["dependencies"]
        .as_array()
        .context("PureCARD cargo metadata has no dependencies array")?;
    let mut names = BTreeSet::new();
    for dependency in dependencies {
        let is_normal = dependency["kind"].is_null();
        let is_optional = dependency["optional"]
            .as_bool()
            .context("PureCARD cargo metadata dependency has no optional flag")?;
        if is_normal && !is_optional {
            let name = dependency["name"]
                .as_str()
                .context("PureCARD cargo metadata dependency has no name")?;
            names.insert(name.to_string());
        }
    }
    Ok(names)
}

/// Runtime dependency names outside PureCARD's protected allowlist.
fn disallowed_core_deps(dependencies: &BTreeSet<String>) -> Vec<String> {
    dependencies
        .iter()
        .filter(|dependency| !CORE_DEP_ALLOWLIST.contains(&dependency.as_str()))
        .cloned()
        .collect()
}

/// Assert that PureCARD stays dep-light in its default shipped configuration.
///
/// Reads Cargo metadata for the nested package and rejects every non-optional
/// normal dependency outside [`CORE_DEP_ALLOWLIST`]. Optional Python/tokenizer
/// boundaries and dev/build-only oracle dependencies are intentionally outside
/// this default runtime surface. Unlike standalone PureCARD's former gate, this
/// performs no `cargo package --list` check: the migrated crate is unpublished.
///
/// # Errors
///
/// Returns an error when the package cannot be located at its migrated manifest
/// or its non-optional normal dependency set contains an unallowlisted crate.
pub fn check_core_deplight() -> Result<()> {
    let json = run_stdout("cargo", &["metadata", "--no-deps", "--format-version", "1"])?;
    let metadata: serde_json::Value =
        serde_json::from_str(&json).context("parsing `cargo metadata` output")?;
    let packages = metadata["packages"]
        .as_array()
        .context("`cargo metadata` has no packages array")?;
    let package = packages
        .iter()
        .find(|package| package["name"].as_str() == Some(PURECARD_PACKAGE))
        .with_context(|| {
            format!("workspace has no `{PURECARD_PACKAGE}` package at {PURECARD_MANIFEST}")
        })?;
    let manifest = package["manifest_path"]
        .as_str()
        .context("PureCARD cargo metadata has no manifest_path")?;
    if !Path::new(manifest).ends_with(PURECARD_MANIFEST) {
        anyhow::bail!("`{PURECARD_PACKAGE}` resolved to {manifest}, expected {PURECARD_MANIFEST}");
    }

    let dependencies = non_optional_runtime_dependencies(package)?;
    let disallowed = disallowed_core_deps(&dependencies);
    if !disallowed.is_empty() {
        anyhow::bail!(
            "`{PURECARD_PACKAGE}` non-optional runtime dependencies may contain only {{ {} }}, \
             but found: {}. Move oracle dependencies to dev-dependencies or make an explicit \
             boundary optional.",
            CORE_DEP_ALLOWLIST.join(", "),
            disallowed.join(", ")
        );
    }
    Ok(())
}

/// Verify Cargo resolves the nested fuzz project as its own workspace.
///
/// A root `workspace.exclude` entry is insufficient for a fuzz crate nested
/// below a workspace member; without the fuzz manifest's empty `[workspace]`,
/// every `cargo fuzz` command fails before compilation.
fn verify_purecard_fuzz_workspace() -> Result<()> {
    let json = run_stdout(
        "cargo",
        &[
            "metadata",
            "--manifest-path",
            PURECARD_FUZZ_MANIFEST,
            "--no-deps",
            "--format-version",
            "1",
        ],
    )?;
    let metadata: serde_json::Value =
        serde_json::from_str(&json).context("parsing PureCARD fuzz cargo metadata")?;
    let workspace_root = metadata["workspace_root"]
        .as_str()
        .context("PureCARD fuzz cargo metadata has no workspace_root")?;
    if !Path::new(workspace_root).ends_with(PURECARD_FUZZ_DIR) {
        anyhow::bail!(
            "{PURECARD_FUZZ_MANIFEST} must define its own `[workspace]`; Cargo resolved it under \
             {workspace_root} instead of {PURECARD_FUZZ_DIR}"
        );
    }

    let registered: BTreeSet<String> = PURECARD_FUZZ_TARGETS
        .iter()
        .map(|target| (*target).to_string())
        .collect();
    let manifest_targets = fuzz_target_names_from_metadata(&metadata)?;
    let source_targets = fuzz_target_names_on_disk(Path::new(PURECARD_FUZZ_TARGET_DIR))?;
    let drift = fuzz_target_registry_problems(&registered, &manifest_targets, &source_targets);
    if !drift.is_empty() {
        anyhow::bail!(
            "PureCARD fuzz target registry drift (xtask, Cargo manifest, and fuzz_targets/*.rs \
             must agree): {}",
            drift.join("; ")
        );
    }
    Ok(())
}

/// Binary target names declared by the nested PureCARD fuzz manifest.
fn fuzz_target_names_from_metadata(metadata: &serde_json::Value) -> Result<BTreeSet<String>> {
    let packages = metadata["packages"]
        .as_array()
        .context("PureCARD fuzz cargo metadata has no packages array")?;
    let package = packages
        .iter()
        .find(|package| {
            package["manifest_path"]
                .as_str()
                .is_some_and(|path| Path::new(path).ends_with(PURECARD_FUZZ_MANIFEST))
        })
        .with_context(|| format!("cargo metadata has no package at {PURECARD_FUZZ_MANIFEST}"))?;
    let targets = package["targets"]
        .as_array()
        .context("PureCARD fuzz cargo metadata has no targets array")?;

    Ok(targets
        .iter()
        .filter(|target| {
            target["kind"]
                .as_array()
                .is_some_and(|kinds| kinds.iter().any(|kind| kind.as_str() == Some("bin")))
        })
        .filter_map(|target| target["name"].as_str().map(str::to_string))
        .collect())
}

/// Whether a repository directory walk includes nested directories.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WalkDepth {
    /// Inspect files directly below the root only.
    Shallow,
    /// Inspect files below the root and every nested directory.
    Recursive,
}

/// Every file discovered below `root` at the requested traversal depth.
fn files_under(root: &Path, depth: WalkDepth) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        for entry in std::fs::read_dir(&directory)
            .with_context(|| format!("reading {}", directory.display()))?
        {
            let entry =
                entry.with_context(|| format!("reading entry in {}", directory.display()))?;
            let path = entry.path();
            if entry
                .file_type()
                .with_context(|| format!("reading file type for {}", path.display()))?
                .is_dir()
            {
                if depth == WalkDepth::Recursive {
                    stack.push(path);
                }
            } else {
                files.push(path);
            }
        }
    }
    files.sort();
    Ok(files)
}

/// Rust source stems present in the dedicated PureCARD fuzz target directory.
fn fuzz_target_names_on_disk(directory: &Path) -> Result<BTreeSet<String>> {
    let mut names = BTreeSet::new();
    for path in files_under(directory, WalkDepth::Shallow)? {
        if path.extension().is_some_and(|extension| extension == "rs") {
            let stem = path
                .file_stem()
                .and_then(|value| value.to_str())
                .with_context(|| format!("non-UTF-8 fuzz target path: {}", path.display()))?;
            names.insert(stem.to_string());
        }
    }
    Ok(names)
}

/// Deterministic diagnostics for any disagreement among fuzz target registries.
fn fuzz_target_registry_problems(
    registered: &BTreeSet<String>,
    manifest: &BTreeSet<String>,
    sources: &BTreeSet<String>,
) -> Vec<String> {
    let mut problems = Vec::new();
    if manifest != registered {
        problems.push(format!(
            "Cargo targets [{}], xtask targets [{}]",
            manifest.iter().cloned().collect::<Vec<_>>().join(", "),
            registered.iter().cloned().collect::<Vec<_>>().join(", ")
        ));
    }
    if sources != registered {
        problems.push(format!(
            "source targets [{}], xtask targets [{}]",
            sources.iter().cloned().collect::<Vec<_>>().join(", "),
            registered.iter().cloned().collect::<Vec<_>>().join(", ")
        ));
    }
    problems
}

/// The minimum acceptable line-coverage percentage. Enforced as a hard floor so
/// coverage can only ratchet upward. Tighten with human sign-off; never loosen.
const COVERAGE_FLOOR_PCT: &str = "70";

/// Path regex excluded from coverage measurement: `xtask` is the build/CI
/// orchestrator, not shipped product code, so it is not held to the product
/// coverage floor. This scopes what is measured; it does not lower the floor.
const COVERAGE_IGNORE_REGEX: &str = "xtask/";

/// Produce a coverage report using `cargo-llvm-cov` and enforce a floor.
///
/// `--fail-under-lines` makes the command exit non-zero if line coverage drops
/// below [`COVERAGE_FLOOR_PCT`], so `xtask coverage` doubles as a CI gate. With
/// `html`, also emits a browsable HTML report under `target/llvm-cov`.
pub fn coverage(html: bool) -> Result<()> {
    let mut args = vec![
        "llvm-cov",
        "--workspace",
        "--ignore-filename-regex",
        COVERAGE_IGNORE_REGEX,
        "--fail-under-lines",
        COVERAGE_FLOOR_PCT,
    ];
    if html {
        args.push("--html");
    } else {
        args.push("--summary-only");
    }
    run("cargo", &args)
}

/// Path to the release-plz configuration, relative to the workspace root.
const RELEASE_PLZ_CONFIG: &str = "release-plz.toml";

/// Validate `release-plz.toml` against the real workspace membership.
///
/// release-plz only runs on push to `main` (post-merge), so a config whose
/// `[[package]]` override names a crate that isn't a workspace member — the
/// exact drift that reddened the trunk once already — cannot be caught before
/// merge. release-plz rejects such an override at runtime ("overrides are not
/// present in the workspace"); this reproduces that check offline so it fails a
/// PR instead of the trunk.
///
/// Running release-plz's own CLI as the gate is unfit here: `update` needs a
/// branch upstream and git history that a PR's detached-HEAD checkout lacks, so
/// it fails for reasons unrelated to config. Comparing the config's overrides
/// against `cargo metadata` is deterministic, needs no network or git state,
/// and targets precisely the class of bug that broke the trunk.
pub fn release_plz_check() -> Result<()> {
    let src = std::fs::read_to_string(RELEASE_PLZ_CONFIG)
        .with_context(|| format!("reading {RELEASE_PLZ_CONFIG}"))?;
    let overrides = release_plz_override_names(&src);
    let members = workspace_member_names()?;

    let missing = missing_overrides(&overrides, &members);
    if !missing.is_empty() {
        anyhow::bail!(
            "{RELEASE_PLZ_CONFIG} has [[package]] overrides not present in the workspace: {}. \
             Remove them or fix the name — an override for a non-member crate reddens every \
             push to main.",
            missing.join(", ")
        );
    }
    Ok(())
}

/// Extract the `name` of every `[[package]]` table in a release-plz config.
///
/// A hand scan rather than a TOML dependency: the only key we need is the
/// override name, and the array-of-tables shape release-plz uses is trivial to
/// walk line by line.
fn release_plz_override_names(toml_src: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut in_package = false;
    for line in toml_src.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_package = trimmed == "[[package]]";
            continue;
        }
        if !in_package {
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("name")
            && let Some(value) = rest.trim_start().strip_prefix('=')
        {
            // Take only the first quoted token, so a trailing inline comment
            // (`name = "domain" # note`) doesn't leak into the parsed name.
            if let Some(name) = value
                .trim()
                .strip_prefix('"')
                .and_then(|v| v.split('"').next())
            {
                names.push(name.to_string());
            }
        }
    }
    names
}

/// Names of every workspace-member package, via `cargo metadata`. With
/// `--no-deps` the reported packages are exactly the workspace members (the set
/// release-plz resolves overrides against), so excluded crates like `lints` are
/// correctly absent.
fn workspace_member_names() -> Result<Vec<String>> {
    let json = run_stdout("cargo", &["metadata", "--no-deps", "--format-version", "1"])?;
    let meta: serde_json::Value =
        serde_json::from_str(&json).context("parsing `cargo metadata` output")?;
    let packages = meta["packages"]
        .as_array()
        .context("`cargo metadata` has no packages array")?;
    Ok(packages
        .iter()
        .filter_map(|p| p["name"].as_str().map(str::to_string))
        .collect())
}

/// Override names absent from the workspace-member set.
fn missing_overrides(overrides: &[String], members: &[String]) -> Vec<String> {
    overrides
        .iter()
        .filter(|name| !members.iter().any(|member| member == *name))
        .cloned()
        .collect()
}

/// Public library crates whose API surface is snapshotted. `pure-analyzer-cli`
/// is a binary (no library API) and `xtask` is dev tooling; both are
/// intentionally excluded.
const PUBLIC_API_CRATES: &[&str] = &[
    "pure-analyzer-diagnostics",
    "pure-analyzer-lexer",
    "pure-analyzer-syntax",
    "pure-analyzer-parser",
    "pure-analyzer-model",
    "pure-analyzer-resolve",
    "pure-analyzer-analysis",
    "libpure",
];

/// Directory holding the committed public-API baseline snapshots.
const PUBLIC_API_DIR: &str = "public-api";

/// Snapshot each public crate's API with `cargo public-api` (which needs a
/// nightly toolchain for rustdoc JSON) and, unless `bless` is set, fail if it
/// drifts from the committed baseline under [`PUBLIC_API_DIR`].
///
/// # Errors
///
/// Returns an error if a snapshot cannot be produced or written, or (when not
/// blessing) if the regenerated surface differs from the committed baseline.
pub fn public_api(bless: bool) -> Result<()> {
    if bless {
        std::fs::create_dir_all(PUBLIC_API_DIR)
            .with_context(|| format!("creating {PUBLIC_API_DIR}/"))?;
    }

    let mut drift = Vec::new();
    for krate in PUBLIC_API_CRATES {
        let surface = run_stdout("cargo", &["+nightly", "public-api", "-p", krate])?;
        let path = format!("{PUBLIC_API_DIR}/{krate}.txt");

        if bless {
            std::fs::write(&path, surface).with_context(|| format!("writing {path}"))?;
        } else {
            // Check-only: compare against the committed baseline in memory, never
            // touching the working tree.
            let baseline = std::fs::read_to_string(&path).with_context(|| {
                format!("reading baseline {path} (run `just public-api-bless`)")
            })?;
            if baseline != surface {
                drift.push(krate.to_string());
            }
        }
    }

    if !drift.is_empty() {
        anyhow::bail!(
            "public API drifted for: {}. Review and run `just public-api-bless` if intended.",
            drift.join(", ")
        );
    }
    Ok(())
}

/// Create an isolated git worktree + branch `feature/<name>` for a change.
///
/// One worktree per branch keeps parallel work from stepping on each other.
///
/// # Errors
///
/// Returns an error if `name` is empty or the underlying `git` commands fail.
pub fn new_feature(name: &str) -> Result<()> {
    validate_name(name, "new-feature")?;
    let branch = format!("feature/{name}");
    let repo_dir = std::env::current_dir().context("reading current directory")?;
    let repo_name = repo_dir
        .file_name()
        .context("current directory has no name")?
        .to_string_lossy();
    let worktree = format!("../{repo_name}-{name}");

    // Best-effort: an offline `fetch` shouldn't block creating the worktree.
    let _ = run("git", &["fetch", "--quiet", "origin"]);
    run("git", &["worktree", "add", "-b", &branch, &worktree])?;

    println!("Created worktree at {worktree} on branch {branch}");
    println!("  cd \"{worktree}\" && just ci");
    Ok(())
}

// ---------------------------------------------------------------------------
// PureCARD doc-fact assertions (L3): every discrete fact a copied doc cites is
// checked against its one authoritative source inside the nested crate.
// ---------------------------------------------------------------------------

/// PureCARD README path relative to [`PURECARD_ROOT`].
const DOC_README: &str = "README.md";
/// PureCARD documentation directory relative to [`PURECARD_ROOT`].
const DOC_DIR: &str = "docs";
/// Gold-soundness source relative to [`PURECARD_ROOT`].
const SOUNDNESS_REPLAY_SRC: &str = "tests/soundness_replay.rs";
/// Gold-count source relative to [`PURECARD_ROOT`].
const SELFCHECK_CORPUS_SRC: &str = "tests/selfcheck_corpus.rs";
/// L2 in-scope split source relative to [`PURECARD_ROOT`].
const L2_SOUNDNESS_SRC: &str = "tests/l2_soundness.rs";
/// Independent L2 total source relative to [`PURECARD_ROOT`].
const L2_PROPERTIES_SRC: &str = "tests/l2_properties.rs";
/// Gold corpus relative to [`PURECARD_ROOT`].
const GOLD_CORPUS: &str = "corpus/gold_queries.jsonl";
/// PureCARD architecture document relative to [`PURECARD_ROOT`].
const ARCHITECTURE_DOC: &str = "docs/spec/architecture.md";
/// Heading preceding the fenced source-module tree in the architecture doc.
const MODULE_TREE_HEADING: &str = "### 3.2 Crate layout";
/// The crate root is represented as the tree root, not as a leaf module.
const CRATE_ROOT_STEM: &str = "lib";
/// Root-level differential labeler that owns the Legend engine version pin.
const LABELER_SRC: &str = "scripts/label-differential.mjs";
/// JavaScript constant in [`LABELER_SRC`] holding the engine pin.
const ENGINE_PIN_CONST: &str = "PINNED_ENGINE_VERSION";
/// PureCARD grammar spec relative to [`PURECARD_ROOT`].
const GRAMMAR_DOC: &str = "docs/spec/grammar.md";
/// PureCARD overview relative to [`PURECARD_ROOT`].
const OVERVIEW_DOC: &str = "docs/spec/overview.md";
/// PureCARD decoder-testing guide relative to [`PURECARD_ROOT`].
const DECODER_TESTING_DOC: &str = "docs/methodology/decoder-testing.md";
/// Enum literals whose combined corpus occurrence count is documented.
const SORT_DIRECTION_LITERALS: [&str; 2] = ["SortDirection.ASC", "SortDirection.DESC"];
/// Pipeline step whose per-record gold count is documented.
const MAP_STEP: &str = "->map(";

/// Assert every discrete PureCARD documentation fact matches its source.
///
/// The scan is intentionally rooted at [`PURECARD_ROOT`], so similarly named
/// analyzer docs, tests, and modules cannot contaminate PureCARD's figures.
/// All violations are collected before returning to make one run actionable.
///
/// # Errors
///
/// Returns an error if an authoritative source cannot be read, the documented
/// module tree cannot be found, or one or more cited facts have drifted.
pub fn check_doc_facts() -> Result<()> {
    let mut errors = Vec::new();
    let gold = check_gold_count_facts(&mut errors)?;
    check_in_scope_facts(&mut errors)?;
    check_module_tree_fact(&mut errors)?;
    check_doc_enumerations(&collect_docs()?, gold, &mut errors);
    check_grammar_and_usage_facts(&mut errors)?;

    if !errors.is_empty() {
        anyhow::bail!(
            "PureCARD doc-fact drift; every cited fact must match its single source:\n{}",
            errors
                .iter()
                .map(|error| format!("  - {error}"))
                .collect::<Vec<_>>()
                .join("\n")
        );
    }
    Ok(())
}

/// Check the gold partition constants and physical corpus count.
fn check_gold_count_facts(errors: &mut Vec<String>) -> Result<usize> {
    let soundness_replay = purecard_path(SOUNDNESS_REPLAY_SRC);
    let selfcheck_corpus = purecard_path(SELFCHECK_CORPUS_SRC);
    let gold_corpus = purecard_path(GOLD_CORPUS);
    let arm_a = read_usize_const(&soundness_replay, "ARM_A")?;
    let arm_c = read_usize_const(&soundness_replay, "ARM_C")?;
    let gold = read_usize_const(&selfcheck_corpus, "EXPECTED_GOLD_RECORDS")?;
    if arm_a + arm_c != gold {
        errors.push(format!(
            "gold-count consts disagree: {} ARM_A+ARM_C = {} but {} \
             EXPECTED_GOLD_RECORDS = {gold}",
            soundness_replay.display(),
            arm_a + arm_c,
            selfcheck_corpus.display()
        ));
    }
    let corpus = count_corpus_records(&gold_corpus)?;
    if corpus != gold {
        errors.push(format!(
            "{} holds {corpus} records but EXPECTED_GOLD_RECORDS = {gold}",
            gold_corpus.display()
        ));
    }
    Ok(gold)
}

/// Check the independently maintained L2 in-scope totals.
fn check_in_scope_facts(errors: &mut Vec<String>) -> Result<()> {
    let l2_soundness = purecard_path(L2_SOUNDNESS_SRC);
    let l2_properties = purecard_path(L2_PROPERTIES_SRC);
    let in_a = read_usize_const(&l2_soundness, "IN_SCOPE_ARM_A")?;
    let in_c = read_usize_const(&l2_soundness, "IN_SCOPE_ARM_C")?;
    let in_total = read_usize_const(&l2_soundness, "IN_SCOPE_TOTAL")?;
    if in_a + in_c != in_total {
        errors.push(format!(
            "in-scope consts disagree in {}: {in_a} + {in_c} != {in_total}",
            l2_soundness.display()
        ));
    }
    let in_total_props = read_usize_const(&l2_properties, "IN_SCOPE_TOTAL")?;
    if in_total_props != in_total {
        errors.push(format!(
            "IN_SCOPE_TOTAL drifted: {} = {in_total}, {} = {in_total_props}",
            l2_soundness.display(),
            l2_properties.display()
        ));
    }
    Ok(())
}

/// Check the documented module tree against PureCARD's nested source tree.
fn check_module_tree_fact(errors: &mut Vec<String>) -> Result<()> {
    let architecture_doc = purecard_path(ARCHITECTURE_DOC);
    let architecture = std::fs::read_to_string(&architecture_doc)
        .with_context(|| format!("reading {}", architecture_doc.display()))?;
    let documented_modules = module_names_in_tree(&architecture).with_context(|| {
        format!(
            "locating the module tree in {} §3.2",
            architecture_doc.display()
        )
    })?;
    let source_modules = src_module_names()?;
    let ghosts: Vec<String> = documented_modules
        .difference(&source_modules)
        .cloned()
        .collect();
    let missing: Vec<String> = source_modules
        .difference(&documented_modules)
        .cloned()
        .collect();
    if !ghosts.is_empty() {
        errors.push(format!(
            "{} §3.2 lists modules absent from src/: {}",
            architecture_doc.display(),
            ghosts.join(", ")
        ));
    }
    if !missing.is_empty() {
        errors.push(format!(
            "{} §3.2 omits src/ modules: {}",
            architecture_doc.display(),
            missing.join(", ")
        ));
    }
    Ok(())
}

/// Check dependency-set prose and gold-total ratios across copied PureCARD docs.
fn check_doc_enumerations(docs: &[(String, String)], gold: usize, errors: &mut Vec<String>) {
    let allow: BTreeSet<String> = CORE_DEP_ALLOWLIST
        .iter()
        .map(|dependency| (*dependency).to_string())
        .collect();
    for (path, text) in docs {
        for set in allowlist_sets(text) {
            let documented: BTreeSet<String> = set.iter().cloned().collect();
            if documented != allow {
                errors.push(format!(
                    "{path} states a core-dep allowlist {{ {} }} that contradicts \
                     CORE_DEP_ALLOWLIST {{ {} }}",
                    set.join(", "),
                    CORE_DEP_ALLOWLIST.join(", ")
                ));
            }
        }
    }
    for (path, text) in docs {
        for cited in gold_ratio_citations(text) {
            if cited != gold {
                errors.push(format!(
                    "{path} cites a gold ratio {cited}/…; the gold total is {gold}"
                ));
            }
        }
    }
}

/// Check the engine pin, SortDirection count, and `map` record count.
fn check_grammar_and_usage_facts(errors: &mut Vec<String>) -> Result<()> {
    let gold_corpus = purecard_path(GOLD_CORPUS);
    let grammar_doc_path = purecard_path(GRAMMAR_DOC);
    let engine_pin = read_js_str_const(Path::new(LABELER_SRC), ENGINE_PIN_CONST)?;
    let grammar_doc = std::fs::read_to_string(&grammar_doc_path)
        .with_context(|| format!("reading {}", grammar_doc_path.display()))?;
    if !grammar_doc.contains(&engine_pin) {
        errors.push(format!(
            "{} does not cite Legend engine version {engine_pin} pinned by \
             {LABELER_SRC} {ENGINE_PIN_CONST}",
            grammar_doc_path.display()
        ));
    }
    let sort_direction = count_corpus_occurrences(&gold_corpus, &SORT_DIRECTION_LITERALS)?;
    if !grammar_doc.contains(&format!("({sort_direction} occurrences)")) {
        errors.push(format!(
            "{} does not cite the SortDirection occurrence count ({sort_direction}) from {}",
            grammar_doc_path.display(),
            gold_corpus.display()
        ));
    }
    let map_gold = count_corpus_records_with(&gold_corpus, MAP_STEP)?;
    let map_citation = format!("`map` ({map_gold} gold");
    for relative in [OVERVIEW_DOC, DECODER_TESTING_DOC] {
        let path = purecard_path(relative);
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        if !text.contains(&map_citation) {
            errors.push(format!(
                "{} does not cite `map` ({map_gold} gold) from {}",
                path.display(),
                gold_corpus.display()
            ));
        }
    }
    Ok(())
}

/// Read an integer `const <name>: usize = <literal>;` from `path`.
fn read_usize_const(path: &Path, name: &str) -> Result<usize> {
    let content =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    parse_usize_const(&content, name)
        .with_context(|| format!("no integer `const {name}: usize` in {}", path.display()))
}

/// Parse an integer usize constant, tolerating `_` digit separators.
fn parse_usize_const(content: &str, name: &str) -> Option<usize> {
    for line in content.lines() {
        let Some(rest) = line.trim_start().strip_prefix("const ") else {
            continue;
        };
        let Some(after_name) = rest.trim_start().strip_prefix(name) else {
            continue;
        };
        let after = after_name.trim_start();
        if !after.starts_with(':') {
            continue;
        }
        let Some(eq) = after.find('=') else {
            continue;
        };
        let digits: String = after[eq + 1..]
            .trim_start()
            .chars()
            .take_while(|character| character.is_ascii_digit() || *character == '_')
            .filter(|character| *character != '_')
            .collect();
        if let Ok(value) = digits.parse::<usize>() {
            return Some(value);
        }
    }
    None
}

/// Read a double-quoted JavaScript string constant from `path`.
fn read_js_str_const(path: &Path, name: &str) -> Result<String> {
    let content =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    parse_js_str_const(&content, name)
        .with_context(|| format!("no string `const {name}` in {}", path.display()))
}

/// Parse `const <name> = "<value>"`, rejecting prefix-name matches.
fn parse_js_str_const(content: &str, name: &str) -> Option<String> {
    for line in content.lines() {
        let Some(rest) = line.trim_start().strip_prefix("const ") else {
            continue;
        };
        let Some(after_name) = rest.trim_start().strip_prefix(name) else {
            continue;
        };
        let after = after_name.trim_start();
        if !after.starts_with('=') {
            continue;
        }
        let value = after[1..].trim_start();
        let Some(inner) = value.strip_prefix('"') else {
            continue;
        };
        if let Some(end) = inner.find('"') {
            return Some(inner[..end].to_owned());
        }
    }
    None
}

/// Count non-empty records in a JSONL corpus.
fn count_corpus_records(path: &Path) -> Result<usize> {
    let content =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    Ok(content
        .lines()
        .filter(|line| !line.trim().is_empty())
        .count())
}

/// Count every raw occurrence of any `needle` across a corpus.
fn count_corpus_occurrences(path: &Path, needles: &[&str]) -> Result<usize> {
    let content =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    Ok(needles
        .iter()
        .map(|needle| content.matches(needle).count())
        .sum())
}

/// Count corpus records containing `needle` at least once.
fn count_corpus_records_with(path: &Path, needle: &str) -> Result<usize> {
    let content =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    Ok(content.lines().filter(|line| line.contains(needle)).count())
}

/// Module paths named in the fenced tree under [`MODULE_TREE_HEADING`].
fn module_names_in_tree(architecture: &str) -> Result<BTreeSet<String>> {
    let heading = architecture
        .find(MODULE_TREE_HEADING)
        .context("module-tree heading not found")?;
    let after_heading = &architecture[heading..];
    let fence_open = after_heading
        .find("```")
        .context("no code fence after the heading")?;
    let body = &after_heading[fence_open + 3..];
    let fence_close = body.find("```").context("unterminated code fence")?;
    Ok(module_paths_in_tree_body(&body[..fence_close]))
}

/// Normalize one indented source tree to paths relative to `src/`.
///
/// Directory entries establish the parent for more-indented `.rs` entries;
/// this keeps `grammar/mod.rs` and `schema/mod.rs` distinct instead of
/// collapsing both to the bare stem `mod`.
fn module_paths_in_tree_body(body: &str) -> BTreeSet<String> {
    let mut directories: BTreeMap<usize, String> = BTreeMap::new();
    let mut modules = BTreeSet::new();

    for line in body.lines() {
        let indent = line.len() - line.trim_start_matches(' ').len();
        let Some(entry) = line.split_whitespace().next() else {
            continue;
        };

        if let Some(directory) = entry.strip_suffix('/') {
            directories.retain(|existing_indent, _| *existing_indent < indent);
            if indent == 0 {
                directories.clear();
                continue;
            }
            let parent = directories
                .range(..indent)
                .next_back()
                .map(|(_, path)| path.as_str());
            let path = parent.map_or_else(
                || directory.to_string(),
                |parent| format!("{parent}/{directory}"),
            );
            directories.insert(indent, path);
            continue;
        }

        let Some(file) = entry.strip_suffix(".rs") else {
            continue;
        };
        let parent = directories
            .range(..indent)
            .next_back()
            .map(|(_, path)| path.as_str());
        let path = parent.map_or_else(|| file.to_string(), |parent| format!("{parent}/{file}"));
        if path != CRATE_ROOT_STEM {
            modules.insert(path);
        }
    }

    modules
}

/// Module paths under PureCARD's nested `src/`, excluding root `lib.rs`.
fn src_module_names() -> Result<BTreeSet<String>> {
    let mut names = BTreeSet::new();
    let source_root = purecard_path("src");
    for path in files_under(&source_root, WalkDepth::Recursive)? {
        if let Some(module) = normalized_rs_module_path(&source_root, &path)? {
            names.insert(module);
        }
    }
    Ok(names)
}

/// Normalize one Rust source path to an extensionless path relative to `src/`.
fn normalized_rs_module_path(source_root: &Path, path: &Path) -> Result<Option<String>> {
    if !path.extension().is_some_and(|extension| extension == "rs") {
        return Ok(None);
    }
    let relative = path
        .strip_prefix(source_root)
        .with_context(|| format!("{} is outside {}", path.display(), source_root.display()))?;
    if relative == Path::new("lib.rs") {
        return Ok(None);
    }
    let without_extension = relative.with_extension("");
    let normalized = without_extension
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/");
    Ok(Some(normalized))
}

/// PureCARD's README and every Markdown file below its copied `docs/` tree.
fn collect_docs() -> Result<Vec<(String, String)>> {
    let mut docs = Vec::new();
    let readme_path = purecard_path(DOC_README);
    let readme = std::fs::read_to_string(&readme_path)
        .with_context(|| format!("reading {}", readme_path.display()))?;
    docs.push((readme_path.to_string_lossy().into_owned(), readme));

    for path in files_under(&purecard_path(DOC_DIR), WalkDepth::Recursive)? {
        if path.extension().is_some_and(|extension| extension == "md") {
            let text = std::fs::read_to_string(&path)
                .with_context(|| format!("reading {}", path.display()))?;
            docs.push((path.to_string_lossy().into_owned(), text));
        }
    }
    Ok(docs)
}

/// English stopwords excluded from candidate `{ dependency, … }` sets.
const BRACE_STOPWORDS: &[&str] = &["and", "the", "for", "not", "but", "with", "plus"];
/// Maximum length of a brace body treated as a dependency enumeration.
const MAX_ALLOWLIST_BRACE_LEN: usize = 80;
/// Minimum token length in a candidate dependency enumeration.
const MIN_ALLOWLIST_TOKEN_LEN: usize = 3;
/// Leading authoritative dependencies needed to identify an allowlist claim.
const ALLOWLIST_TRIGGER_DEPENDENCIES: usize = 2;

/// Dependency-set enumerations that claim the widened PureCARD allowlist.
fn allowlist_sets(text: &str) -> Vec<Vec<String>> {
    let mut sets = Vec::new();
    let mut from = 0;
    while let Some(relative) = text[from..].find('{') {
        let open = from + relative;
        let Some(relative_close) = text[open + 1..].find('}') else {
            from = open + 1;
            continue;
        };
        let inner = &text[open + 1..open + 1 + relative_close];
        if inner.len() >= MAX_ALLOWLIST_BRACE_LEN {
            from = open + 1;
            continue;
        }
        from = open + 1 + relative_close + 1;
        let tokens: Vec<String> = inner
            .split(|character: char| !(character.is_ascii_alphanumeric() || character == '_'))
            .filter(|token| {
                token.len() >= MIN_ALLOWLIST_TOKEN_LEN
                    && token.chars().all(|character| {
                        character.is_ascii_lowercase()
                            || character == '_'
                            || character.is_ascii_digit()
                    })
            })
            .filter(|token| !BRACE_STOPWORDS.contains(token))
            .map(str::to_string)
            .collect();
        let is_allowlist_claim = CORE_DEP_ALLOWLIST
            .iter()
            .take(ALLOWLIST_TRIGGER_DEPENDENCIES)
            .all(|dependency| tokens.iter().any(|token| token == dependency));
        if is_allowlist_claim {
            sets.push(tokens);
        }
    }
    sets
}

/// Numbers from every unspaced `N/N` ratio on a line mentioning gold.
fn gold_ratio_citations(text: &str) -> Vec<usize> {
    let mut citations = Vec::new();
    for line in text.lines() {
        if !line.to_ascii_lowercase().contains("gold") {
            continue;
        }
        let characters: Vec<char> = line.chars().collect();
        let is_run = |character: char| character.is_ascii_digit() || character == ',';
        for slash in 0..characters.len() {
            if characters[slash] != '/' {
                continue;
            }
            let mut left = slash;
            while left > 0 && is_run(characters[left - 1]) {
                left -= 1;
            }
            let mut right = slash + 1;
            while right < characters.len() && is_run(characters[right]) {
                right += 1;
            }
            if left == slash || right == slash + 1 {
                continue;
            }
            let left_token: String = characters[left..slash].iter().collect();
            let right_token: String = characters[slash + 1..right].iter().collect();
            if let (Some(left_value), Some(right_value)) =
                (parse_grouped(&left_token), parse_grouped(&right_token))
            {
                citations.push(left_value);
                citations.push(right_value);
            }
        }
    }
    citations
}

/// Parse a possibly comma-grouped integer.
fn parse_grouped(token: &str) -> Option<usize> {
    let digits: String = token.chars().filter(char::is_ascii_digit).collect();
    if digits.is_empty() {
        return None;
    }
    digits.parse().ok()
}

/// The analysis-engine crate DAG (constitution §1, ADR-0003):
/// for each enforced workspace crate, the set of internal crates it may
/// depend on, in any dependency kind. The engine direction is parser → model →
/// resolve: the resolver may depend on model types, never the reverse. An
/// explicit allow-set captures that order together with the diagnostics leaf,
/// facade, and front-end boundaries more clearly than a rank comparison.
/// `pure-analyzer-diagnostics` is a leaf every parser-and-above crate may depend
/// on; the lexer and syntax layers may not (they sit below the
/// diagnostics-consuming boundary).
const ALLOWED_INTERNAL_DEPS: &[(&str, &[&str])] = &[
    ("pure-analyzer-diagnostics", &[]),
    ("pure-analyzer-lexer", &[]),
    ("pure-analyzer-syntax", &["pure-analyzer-lexer"]),
    (
        "pure-analyzer-parser",
        &[
            "pure-analyzer-lexer",
            "pure-analyzer-syntax",
            "pure-analyzer-diagnostics",
        ],
    ),
    (
        "pure-analyzer-model",
        &[
            "pure-analyzer-lexer",
            "pure-analyzer-syntax",
            "pure-analyzer-parser",
            "pure-analyzer-diagnostics",
        ],
    ),
    (
        "pure-analyzer-resolve",
        &[
            "pure-analyzer-lexer",
            "pure-analyzer-syntax",
            "pure-analyzer-parser",
            "pure-analyzer-model",
            "pure-analyzer-diagnostics",
        ],
    ),
    (
        "pure-analyzer-analysis",
        &[
            "pure-analyzer-lexer",
            "pure-analyzer-syntax",
            "pure-analyzer-parser",
            "pure-analyzer-model",
            "pure-analyzer-resolve",
            "pure-analyzer-diagnostics",
        ],
    ),
    (
        "libpure",
        &[
            "pure-analyzer-lexer",
            "pure-analyzer-syntax",
            "pure-analyzer-parser",
            "pure-analyzer-model",
            "pure-analyzer-resolve",
            "pure-analyzer-analysis",
            "pure-analyzer-diagnostics",
        ],
    ),
    (
        "pure-analyzer-cli",
        &["libpure", "pure-analyzer-diagnostics"],
    ),
];

/// The internal crates `name` may depend on, or `None` if `name` is not part
/// of the enforced DAG (e.g. `xtask`, a third-party crate).
fn allowed_internal_deps(name: &str) -> Option<&'static [&'static str]> {
    ALLOWED_INTERNAL_DEPS
        .iter()
        .find(|(crate_name, _)| *crate_name == name)
        .map(|(_, allowed)| *allowed)
}

/// Repository-level product boundary assigned to a Cargo package.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WorkspaceMemberClass {
    /// A crate in the ADR-0003 analysis-engine DAG.
    Analyzer,
    /// The independent PureCARD decoder product.
    Purecard,
    /// Repository automation permitted to orchestrate either product.
    Orchestration,
}

/// Workspace package allowed to orchestrate both independent products.
const ORCHESTRATION_PACKAGE: &str = "xtask";

/// Fail-closed ownership for workspace-excluded Cargo packages.
const EXCLUDED_PACKAGE_BOUNDARIES: &[(&str, &str, WorkspaceMemberClass)] = &[
    ("fuzz", "fuzz", WorkspaceMemberClass::Analyzer),
    (
        "crates/pure-analyzer-purecard/fuzz",
        "purecard-fuzz",
        WorkspaceMemberClass::Purecard,
    ),
    (
        "crates/pure-analyzer-purecard/lints",
        "lints",
        WorkspaceMemberClass::Purecard,
    ),
];

/// The product boundary for a known repository Cargo package.
fn workspace_member_class(name: &str) -> Option<WorkspaceMemberClass> {
    if allowed_internal_deps(name).is_some() {
        Some(WorkspaceMemberClass::Analyzer)
    } else if name == PURECARD_PACKAGE {
        Some(WorkspaceMemberClass::Purecard)
    } else if name == ORCHESTRATION_PACKAGE {
        Some(WorkspaceMemberClass::Orchestration)
    } else {
        EXCLUDED_PACKAGE_BOUNDARIES
            .iter()
            .find(|(_, package_name, _)| *package_name == name)
            .map(|(_, _, class)| *class)
    }
}

/// Workspace members not assigned to a repository product boundary.
fn unclassified_workspace_members(packages: &[serde_json::Value]) -> Vec<String> {
    let mut unclassified: Vec<String> = packages
        .iter()
        .filter_map(|package| package["name"].as_str())
        .filter(|name| workspace_member_class(name).is_none())
        .map(str::to_string)
        .collect();
    unclassified.sort();
    unclassified.dedup();
    unclassified
}

/// Render a dependency edge with every Cargo shape relevant to the boundary.
fn dependency_edge(source: &str, dependency: &serde_json::Value) -> Option<String> {
    let target = dependency["name"].as_str()?;
    let mut attributes = vec![dependency["kind"].as_str().unwrap_or("normal").to_string()];
    if dependency["optional"].as_bool().unwrap_or(false) {
        attributes.push("optional".to_string());
    }
    if let Some(rename) = dependency["rename"].as_str() {
        attributes.push(format!("renamed as {rename}"));
    }
    Some(format!(
        "{source} --({})--> {target}",
        attributes.join(", ")
    ))
}

/// Analyzer-to-PureCARD and PureCARD-to-analyzer dependency edges.
fn cross_product_violations(packages: &[serde_json::Value]) -> Vec<String> {
    let workspace_members: BTreeSet<&str> = packages
        .iter()
        .filter_map(|package| package["name"].as_str())
        .collect();
    let mut violations = Vec::new();
    for package in packages {
        let Some(source) = package["name"].as_str() else {
            continue;
        };
        let Some(source_class) = workspace_member_class(source) else {
            continue;
        };
        let Some(dependencies) = package["dependencies"].as_array() else {
            continue;
        };
        for dependency in dependencies {
            let Some(target) = dependency["name"].as_str() else {
                continue;
            };
            if !workspace_members.contains(target) {
                continue;
            }
            let Some(target_class) = workspace_member_class(target) else {
                continue;
            };
            let crosses_product_boundary = matches!(
                (source_class, target_class),
                (
                    WorkspaceMemberClass::Analyzer,
                    WorkspaceMemberClass::Purecard
                ) | (
                    WorkspaceMemberClass::Purecard,
                    WorkspaceMemberClass::Analyzer
                )
            );
            if crosses_product_boundary && let Some(edge) = dependency_edge(source, dependency) {
                violations.push(edge);
            }
        }
    }
    violations.sort();
    violations.dedup();
    violations
}

/// Collect every layering violation in the parsed `cargo metadata` packages:
/// an enforced crate depending — in any dependency kind — on another enforced
/// crate that is not in its [`ALLOWED_INTERNAL_DEPS`] entry.
///
/// Pure over the JSON so the rule can be unit-tested without shelling out.
fn layering_violations(packages: &[serde_json::Value]) -> Vec<String> {
    let mut violations = Vec::new();
    for package in packages {
        let Some(name) = package["name"].as_str() else {
            continue;
        };
        let Some(allowed) = allowed_internal_deps(name) else {
            continue;
        };
        let Some(dependencies) = package["dependencies"].as_array() else {
            continue;
        };
        for dependency in dependencies {
            let Some(dependency_name) = dependency["name"].as_str() else {
                continue;
            };
            // Only an edge onto another *enforced* crate is in scope; a dep
            // on xtask or a third-party crate isn't part of this DAG.
            if allowed_internal_deps(dependency_name).is_none() {
                continue;
            }
            if !allowed.contains(&dependency_name) {
                // `kind` is null for a normal dep, "dev"/"build" otherwise.
                let kind = dependency["kind"].as_str().unwrap_or("normal");
                violations.push(format!("{name} --({kind})--> {dependency_name}"));
            }
        }
    }
    violations.sort();
    violations.dedup();
    violations
}

/// Deterministic diagnostic for every workspace dependency-topology failure.
fn layering_diagnostic(
    analyzer_violations: &[String],
    product_violations: &[String],
    unclassified_members: &[String],
) -> String {
    let mut sections = Vec::new();
    if !analyzer_violations.is_empty() {
        sections.push(format!(
            "analysis-engine DAG violations (constitution §1, ADR-0003): {}",
            analyzer_violations.join(", ")
        ));
    }
    if !product_violations.is_empty() {
        sections.push(format!(
            "cross-product dependency violations (ADR-0004/ADR-0009): {}",
            product_violations.join(", ")
        ));
    }
    if !unclassified_members.is_empty() {
        sections.push(format!(
            "unclassified workspace members (ADR-0004/ADR-0009): {}",
            unclassified_members.join(", ")
        ));
    }
    sections.join("; ")
}

/// Load workspace-excluded Cargo packages so product boundaries cover tooling.
fn excluded_manifest_packages(workspace_root: &Path) -> Result<Vec<serde_json::Value>> {
    let root_manifest_path = workspace_root.join("Cargo.toml");
    let root_source = std::fs::read_to_string(&root_manifest_path)
        .with_context(|| format!("reading {}", root_manifest_path.display()))?;
    let root: toml::Value = toml::from_str(&root_source)
        .with_context(|| format!("parsing {}", root_manifest_path.display()))?;
    let exclusions = root
        .get("workspace")
        .and_then(|workspace| workspace.get("exclude"))
        .and_then(toml::Value::as_array)
        .context("root Cargo.toml workspace.exclude must be an array")?;

    let classified_paths = classified_excluded_paths(exclusions)?;

    let mut packages = Vec::with_capacity(classified_paths.len());
    for (relative, expected_name) in classified_paths {
        let manifest_path = workspace_root.join(relative).join("Cargo.toml");
        let source = std::fs::read_to_string(&manifest_path)
            .with_context(|| format!("reading excluded manifest {}", manifest_path.display()))?;
        let package = manifest_package_value(&source, &manifest_path.display().to_string())?;
        let actual_name = package["name"]
            .as_str()
            .context("parsed excluded package has no name")?;
        validate_excluded_package_name(relative, expected_name, actual_name)?;
        packages.push(package);
    }
    Ok(packages)
}

/// Match `workspace.exclude` exactly against the classified package paths.
fn classified_excluded_paths(exclusions: &[toml::Value]) -> Result<Vec<(&str, &'static str)>> {
    let mut relative_paths: Vec<&str> = exclusions
        .iter()
        .map(|value| {
            value
                .as_str()
                .context("workspace.exclude entries must be strings")
        })
        .collect::<Result<_>>()?;
    relative_paths.sort_unstable();

    let mut classified_paths = Vec::with_capacity(relative_paths.len());
    for relative in relative_paths {
        if relative.contains(['*', '?', '[', ']']) {
            anyhow::bail!(
                "workspace.exclude pattern `{relative}` cannot be classified fail-closed; \
                 list each excluded Cargo package explicitly"
            );
        }
        let expected_name = EXCLUDED_PACKAGE_BOUNDARIES
            .iter()
            .find(|(path, _, _)| *path == relative)
            .map(|(_, package_name, _)| *package_name)
            .with_context(|| {
                format!(
                    "workspace.exclude path `{relative}` has no product-boundary classification"
                )
            })?;
        classified_paths.push((relative, expected_name));
    }

    let present_paths: BTreeSet<&str> = classified_paths
        .iter()
        .map(|(relative, _)| *relative)
        .collect();
    let missing_paths: Vec<&str> = EXCLUDED_PACKAGE_BOUNDARIES
        .iter()
        .map(|(relative, _, _)| *relative)
        .filter(|relative| !present_paths.contains(relative))
        .collect();
    if !missing_paths.is_empty() {
        anyhow::bail!(
            "root Cargo.toml workspace.exclude is missing expected product-boundary path(s): {}",
            missing_paths
                .iter()
                .map(|relative| format!("`{relative}`"))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    Ok(classified_paths)
}

/// Ensure a classified exclusion still declares the package name we inspect.
fn validate_excluded_package_name(
    relative: &str,
    expected_name: &str,
    actual_name: &str,
) -> Result<()> {
    if actual_name != expected_name {
        anyhow::bail!(
            "workspace.exclude path `{relative}` is classified as package `{expected_name}` \
             but its manifest declares `{actual_name}`"
        );
    }
    Ok(())
}

/// Convert a standalone Cargo manifest into the metadata subset used by gates.
fn manifest_package_value(source: &str, label: &str) -> Result<serde_json::Value> {
    let document: toml::Value =
        toml::from_str(source).with_context(|| format!("parsing excluded manifest {label}"))?;
    let package_name = document
        .get("package")
        .and_then(|package| package.get("name"))
        .and_then(toml::Value::as_str)
        .with_context(|| format!("excluded manifest {label} has no package.name"))?;
    let dependencies = manifest_dependencies(&document, label)?;

    let mut package = serde_json::Map::new();
    package.insert("name".to_string(), serde_json::Value::from(package_name));
    package.insert(
        "dependencies".to_string(),
        serde_json::Value::Array(dependencies),
    );
    Ok(serde_json::Value::Object(package))
}

/// Collect normal/dev/build dependencies, including target-specific tables.
fn manifest_dependencies(document: &toml::Value, label: &str) -> Result<Vec<serde_json::Value>> {
    let mut dependencies = Vec::new();
    append_manifest_dependency_kinds(document, label, &mut dependencies)?;

    if let Some(targets) = document.get("target") {
        let targets = targets
            .as_table()
            .with_context(|| format!("excluded manifest {label} target must be a table"))?;
        for (selector, target) in targets {
            let target_label = format!("{label} target.{selector}");
            append_manifest_dependency_kinds(target, &target_label, &mut dependencies)?;
        }
    }
    Ok(dependencies)
}

/// Append Cargo's three dependency-table kinds from one manifest table.
fn append_manifest_dependency_kinds(
    document: &toml::Value,
    label: &str,
    dependencies: &mut Vec<serde_json::Value>,
) -> Result<()> {
    for (table_name, kind) in [
        ("dependencies", None),
        ("dev-dependencies", Some("dev")),
        ("build-dependencies", Some("build")),
    ] {
        let Some(table) = document.get(table_name) else {
            continue;
        };
        let table = table
            .as_table()
            .with_context(|| format!("excluded manifest {label} {table_name} must be a table"))?;
        for (alias, specification) in table {
            let (package_name, optional) = match specification {
                toml::Value::String(_) => (alias.as_str(), false),
                toml::Value::Table(specification) => {
                    if let Some(workspace) = specification.get("workspace") {
                        let inherited = workspace.as_bool().with_context(|| {
                            format!(
                                "excluded manifest {label} dependency {alias}.workspace \
                                 must be a boolean"
                            )
                        })?;
                        if inherited {
                            anyhow::bail!(
                                "excluded manifest {label} dependency {alias} inherits a \
                                 workspace dependency whose renamed package cannot be resolved \
                                 fail-closed; declare its package/version/path explicitly"
                            );
                        }
                    }
                    let package_name = match specification.get("package") {
                        Some(value) => value.as_str().with_context(|| {
                            format!(
                                "excluded manifest {label} dependency {alias}.package \
                                 must be a string"
                            )
                        })?,
                        None => alias,
                    };
                    let optional = match specification.get("optional") {
                        Some(value) => value.as_bool().with_context(|| {
                            format!(
                                "excluded manifest {label} dependency {alias}.optional \
                                 must be a boolean"
                            )
                        })?,
                        None => false,
                    };
                    (package_name, optional)
                }
                _ => {
                    anyhow::bail!(
                        "excluded manifest {label} dependency {alias} must use a string or table"
                    );
                }
            };
            dependencies.push(metadata_dependency_value(
                package_name,
                kind,
                optional,
                (package_name != alias).then_some(alias.as_str()),
            ));
        }
    }
    Ok(())
}

/// Build the cargo-metadata dependency fields consumed by topology checks.
fn metadata_dependency_value(
    name: &str,
    kind: Option<&str>,
    optional: bool,
    rename: Option<&str>,
) -> serde_json::Value {
    let mut dependency = serde_json::Map::new();
    dependency.insert("name".to_string(), serde_json::Value::from(name));
    dependency.insert(
        "kind".to_string(),
        kind.map_or(serde_json::Value::Null, serde_json::Value::from),
    );
    dependency.insert("optional".to_string(), serde_json::Value::from(optional));
    dependency.insert(
        "rename".to_string(),
        rename.map_or(serde_json::Value::Null, serde_json::Value::from),
    );
    serde_json::Value::Object(dependency)
}

/// Fail if a Cargo package is unclassified, violates the analyzer DAG, or
/// crosses the analyzer/PureCARD product boundary in any dependency kind.
///
/// The layering rule (constitution §1, ADR-0003) requires the analysis-engine
/// dependency graph to follow the documented DAG exactly. `cargo-deny` bans a
/// crate globally, not "crate X may not depend on crate Y", and the
/// `no-front-end-deps-in-core` ast-grep rule only keeps renderer/protocol
/// crates out of the core — neither catches a sideways or reversed edge among
/// the core crates themselves (e.g. `pure-analyzer-model` depending on
/// `pure-analyzer-resolve`). This closes the gap by reading the workspace
/// dependency graph from `cargo metadata` and rejecting any edge absent from
/// [`ALLOWED_INTERNAL_DEPS`] — including a dev-dependency, the exact edge a
/// crate-global ban misses. It is deterministic, offline, and reproduces
/// faithfully in a PR's detached-`HEAD` checkout.
///
/// ADR-0004 and ADR-0009 additionally keep PureCARD independent from the
/// analyzer DAG. The boundary rejects normal, dev, build, optional, and renamed
/// edges in either direction across workspace members and excluded fuzz/lint
/// packages, while `xtask` may orchestrate both products.
///
/// # Errors
///
/// Returns an error naming each offending edge (including optionality and
/// renames at the product boundary) or unclassified workspace member.
pub fn verify_layering() -> Result<()> {
    let json = run_stdout("cargo", &["metadata", "--no-deps", "--format-version", "1"])?;
    let meta: serde_json::Value =
        serde_json::from_str(&json).context("parsing `cargo metadata` output")?;
    let mut packages = meta["packages"]
        .as_array()
        .context("`cargo metadata` has no packages array")?
        .clone();
    let workspace_root = meta["workspace_root"]
        .as_str()
        .context("`cargo metadata` has no workspace_root")?;
    packages.extend(excluded_manifest_packages(Path::new(workspace_root))?);

    let analyzer_violations = layering_violations(&packages);
    let product_violations = cross_product_violations(&packages);
    let unclassified_members = unclassified_workspace_members(&packages);
    if !analyzer_violations.is_empty()
        || !product_violations.is_empty()
        || !unclassified_members.is_empty()
    {
        anyhow::bail!(
            "forbidden workspace dependency topology: {}",
            layering_diagnostic(
                &analyzer_violations,
                &product_violations,
                &unclassified_members
            )
        );
    }
    Ok(())
}

/// The workspace-lint keys every member must inherit, with the exact level the
/// constitution mandates (§1.2 `unsafe_code`, §1.3 `missing_docs`). A gate that
/// checked only presence, not level, would let a member silently downgrade one.
const REQUIRED_WORKSPACE_LINTS: &[(&str, &str)] =
    &[("unsafe_code", "forbid"), ("missing_docs", "deny")];

/// Verify the workspace-wide lint contract (constitution §1.2/§1.3): the root
/// manifest declares `[workspace.lints.rust]` with each mandated lint at its
/// required level, and every workspace member inherits it via `[lints]
/// workspace = true`. This kills the class of bug where a crate silently omits
/// `#![forbid(unsafe_code)]` / `#![deny(missing_docs)]`: a new member that
/// forgets to wire the shared lints fails the gate instead of shipping unguarded.
///
/// # Errors
///
/// Returns an error if the root table is missing a lint (or sets a weaker
/// level), or if any member manifest does not inherit the workspace lints.
pub fn verify_lints() -> Result<()> {
    let root = std::fs::read_to_string(ROOT_MANIFEST)
        .with_context(|| format!("reading {ROOT_MANIFEST}"))?;
    let mut problems = root_lint_problems(&root);

    for (name, manifest_path) in workspace_member_manifests()? {
        let src = std::fs::read_to_string(&manifest_path)
            .with_context(|| format!("reading {manifest_path}"))?;
        if !inherits_workspace_lints(&src) {
            problems.push(format!(
                "member `{name}` ({manifest_path}) does not inherit workspace lints \
                 (add `[lints]\\nworkspace = true`)"
            ));
        }
    }

    if !problems.is_empty() {
        anyhow::bail!(
            "workspace lint contract violated (constitution §1.2/§1.3): {}",
            problems.join("; ")
        );
    }
    Ok(())
}

/// Root manifest, source of the `[workspace.lints.rust]` table.
const ROOT_MANIFEST: &str = "Cargo.toml";

/// Problems with the root `[workspace.lints.rust]` table: a mandated lint that
/// is absent or set below its required level.
fn root_lint_problems(toml_src: &str) -> Vec<String> {
    REQUIRED_WORKSPACE_LINTS
        .iter()
        .filter_map(|(lint, level)| {
            match table_string_value("workspace.lints.rust", lint, toml_src) {
                Some(actual) if actual == *level => None,
                Some(actual) => Some(format!(
                    "root [workspace.lints.rust] sets `{lint} = \"{actual}\"`, must be \"{level}\""
                )),
                None => Some(format!(
                    "root [workspace.lints.rust] is missing `{lint} = \"{level}\"`"
                )),
            }
        })
        .collect()
}

/// Whether a member manifest inherits the workspace lints, i.e. declares a
/// `[lints]` table with `workspace = true`.
fn inherits_workspace_lints(toml_src: &str) -> bool {
    table_bool_value("lints", "workspace", toml_src) == Some(true)
}

/// Read a bare-string value (`key = "value"`) from a named table, ignoring a
/// trailing inline comment. A hand scan rather than a TOML dependency, matching
/// [`release_plz_override_names`]; the keys we need are simple scalars.
fn table_string_value(table: &str, key: &str, toml_src: &str) -> Option<String> {
    table_value_token(table, key, toml_src)?
        .trim()
        .strip_prefix('"')
        .and_then(|v| v.split('"').next())
        .map(str::to_string)
}

/// Read a boolean value (`key = true`) from a named table.
fn table_bool_value(table: &str, key: &str, toml_src: &str) -> Option<bool> {
    match table_value_token(table, key, toml_src)?.trim() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

/// Raw right-hand-side token for `key` within `[table]`, with any trailing
/// inline comment stripped. Returns `None` if the table or key is absent.
fn table_value_token(table: &str, key: &str, toml_src: &str) -> Option<String> {
    let header = format!("[{table}]");
    let mut in_table = false;
    for line in toml_src.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_table = trimmed == header;
            continue;
        }
        if !in_table {
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix(key)
            && let Some(value) = rest.trim_start().strip_prefix('=')
        {
            let token = value.split('#').next().unwrap_or(value).trim();
            return Some(token.to_string());
        }
    }
    None
}

/// `(name, manifest_path)` for every workspace-member package, via `cargo
/// metadata`. With `--no-deps` the reported packages are exactly the members,
/// so excluded crates like `fuzz`/`lints` are correctly absent.
fn workspace_member_manifests() -> Result<Vec<(String, String)>> {
    let json = run_stdout("cargo", &["metadata", "--no-deps", "--format-version", "1"])?;
    let meta: serde_json::Value =
        serde_json::from_str(&json).context("parsing `cargo metadata` output")?;
    let packages = meta["packages"]
        .as_array()
        .context("`cargo metadata` has no packages array")?;
    Ok(packages
        .iter()
        .filter_map(|p| {
            let name = p["name"].as_str()?.to_string();
            let manifest = p["manifest_path"].as_str()?.to_string();
            Some((name, manifest))
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn purecard_paths_are_rooted_at_the_migrated_crate() {
        assert_eq!(
            purecard_path("tests/soundness_replay.rs"),
            PathBuf::from("crates/pure-analyzer-purecard/tests/soundness_replay.rs")
        );
        assert_eq!(
            PURECARD_MANIFEST,
            "crates/pure-analyzer-purecard/Cargo.toml"
        );
        assert_eq!(PURECARD_FUZZ_DIR, "crates/pure-analyzer-purecard/fuzz");
        assert_eq!(
            PURECARD_FUZZ_MANIFEST,
            "crates/pure-analyzer-purecard/fuzz/Cargo.toml"
        );
    }

    fn metadata_dependency(name: &str, kind: Option<&str>, optional: bool) -> serde_json::Value {
        let mut dependency = serde_json::Map::new();
        dependency.insert("name".to_string(), serde_json::Value::from(name));
        dependency.insert(
            "kind".to_string(),
            kind.map_or(serde_json::Value::Null, serde_json::Value::from),
        );
        dependency.insert("optional".to_string(), serde_json::Value::from(optional));
        serde_json::Value::Object(dependency)
    }

    fn metadata_package(dependencies: Vec<serde_json::Value>) -> serde_json::Value {
        let mut package = serde_json::Map::new();
        package.insert(
            "dependencies".to_string(),
            serde_json::Value::Array(dependencies),
        );
        serde_json::Value::Object(package)
    }

    #[test]
    fn core_dependency_classification_keeps_only_non_optional_normal_edges() {
        let package = metadata_package(vec![
            metadata_dependency("thiserror", None, false),
            metadata_dependency("serde", None, false),
            metadata_dependency("pyo3", None, true),
            metadata_dependency("ureq", Some("dev"), false),
            metadata_dependency("build-helper", Some("build"), false),
        ]);
        let dependencies = non_optional_runtime_dependencies(&package).expect("metadata parses");
        assert_eq!(
            dependencies,
            ["serde".to_string(), "thiserror".to_string()]
                .into_iter()
                .collect()
        );
    }

    #[test]
    fn core_dependency_allowlist_flags_only_unapproved_runtime_edges() {
        let dependencies = [
            "thiserror".to_string(),
            "serde".to_string(),
            "serde_json".to_string(),
            "tokio".to_string(),
        ]
        .into_iter()
        .collect();
        assert_eq!(disallowed_core_deps(&dependencies), ["tokio"]);

        let allowed = CORE_DEP_ALLOWLIST
            .iter()
            .map(|dependency| (*dependency).to_string())
            .collect();
        assert!(disallowed_core_deps(&allowed).is_empty());
    }

    #[test]
    fn parse_js_str_const_reads_a_quoted_value_and_rejects_prefix_names() {
        assert_eq!(
            parse_js_str_const(
                "const PINNED_ENGINE_VERSION = \"4.113.0\";",
                "PINNED_ENGINE_VERSION"
            ),
            Some("4.113.0".to_owned())
        );
        assert_eq!(
            parse_js_str_const(
                "const PINNED_ENGINE_VERSION_X = \"9\";",
                "PINNED_ENGINE_VERSION"
            ),
            None
        );
        assert_eq!(parse_js_str_const("const N = 8000;", "N"), None);
    }

    #[test]
    fn parse_usize_const_reads_a_literal_and_tolerates_separators() {
        assert_eq!(
            parse_usize_const("const ARM_A: usize = 4639;", "ARM_A"),
            Some(4639)
        );
        assert_eq!(
            parse_usize_const("const N: usize = 1_024;", "N"),
            Some(1024)
        );
    }

    #[test]
    fn parse_usize_const_ignores_a_prefix_name_and_a_derived_value() {
        assert_eq!(
            parse_usize_const("const ARM_ABC: usize = 7;", "ARM_A"),
            None
        );
        assert_eq!(
            parse_usize_const(
                "const EXPECTED_GOLD_RECORDS: usize = ARM_A + ARM_C;",
                "EXPECTED_GOLD_RECORDS"
            ),
            None
        );
    }

    #[test]
    fn module_names_in_tree_reads_only_the_fenced_tree_after_the_heading() {
        let architecture = "\
intro\n\n### 3.2 Crate layout\n\n```\npurecard/\n  vocab.rs   the vocab\n  session.rs the session\n```\n\nProse mentioning a ghost engine.rs must be ignored.\n";
        let actual = module_names_in_tree(architecture).expect("tree parses");
        let expected: BTreeSet<String> = ["vocab".to_string(), "session".to_string()]
            .into_iter()
            .collect();
        assert_eq!(actual, expected);
    }

    #[test]
    fn module_paths_keep_nested_mod_files_distinct() {
        let architecture = "\
### 3.2 Crate layout\n\n```\npurecard/\n  grammar/\n    mod.rs grammar root\n  schema/\n    mod.rs schema root\n  session.rs session\n```\n";
        let actual = module_names_in_tree(architecture).expect("tree parses");
        let expected: BTreeSet<String> = ["grammar/mod", "schema/mod", "session"]
            .into_iter()
            .map(str::to_string)
            .collect();
        assert_eq!(actual, expected);

        let root = Path::new("src");
        assert_eq!(
            normalized_rs_module_path(root, Path::new("src/grammar/mod.rs"))
                .expect("path normalizes")
                .as_deref(),
            Some("grammar/mod")
        );
        assert_eq!(
            normalized_rs_module_path(root, Path::new("src/schema/mod.rs"))
                .expect("path normalizes")
                .as_deref(),
            Some("schema/mod")
        );
        assert_eq!(
            normalized_rs_module_path(root, Path::new("src/lib.rs")).expect("path normalizes"),
            None
        );
    }

    #[test]
    fn allowlist_sets_flags_the_widened_form_and_exempts_the_historical_one() {
        assert!(allowlist_sets("M1 widened it to `{ thiserror }`.").is_empty());
        assert_eq!(
            allowlist_sets("the widened `{ thiserror, serde, serde_json }` set")[0],
            ["thiserror", "serde", "serde_json"]
        );
        let long = format!("{{ thiserror {} serde }}", "x".repeat(100));
        assert!(allowlist_sets(&long).is_empty());
    }

    #[test]
    fn allowlist_detection_derives_its_trigger_from_the_authoritative_set() {
        let trigger = CORE_DEP_ALLOWLIST[..ALLOWLIST_TRIGGER_DEPENDENCIES].join(", ");
        let text = format!("the widened `{{ {trigger}, tokio, io }}` set");
        let sets = allowlist_sets(&text);
        assert_eq!(sets.len(), 1);
        assert!(sets[0].iter().any(|dependency| dependency == "tokio"));
        assert!(!sets[0].iter().any(|dependency| dependency == "io"));
    }

    #[test]
    fn purecard_fuzz_target_registry_detects_manifest_and_source_drift() {
        let registered: BTreeSet<String> = ["accept_token", "allowed_mask"]
            .into_iter()
            .map(str::to_string)
            .collect();
        let manifest: BTreeSet<String> = ["accept_token", "new_target"]
            .into_iter()
            .map(str::to_string)
            .collect();
        let sources: BTreeSet<String> = ["accept_token"].into_iter().map(str::to_string).collect();
        let problems = fuzz_target_registry_problems(&registered, &manifest, &sources);
        assert_eq!(problems.len(), 2);
        assert!(problems[0].contains("new_target"));
        assert!(problems[1].contains("allowed_mask"));
    }

    #[test]
    fn allowlist_sets_keeps_scanning_past_a_brace_that_cannot_form_a_set() {
        let filler = "prose ".repeat(20);
        let text = format!(
            "a stray {{ {filler}then `{{ thiserror, serde, tokio }}` widens it, trailing {{"
        );
        assert_eq!(
            allowlist_sets(&text),
            vec![vec![
                "thiserror".to_string(),
                "serde".to_string(),
                "tokio".to_string(),
            ]]
        );
    }

    #[test]
    fn gold_ratio_citations_reads_only_ratios_on_gold_lines() {
        assert_eq!(
            gold_ratio_citations("the is_accepting change keeps gold at 5034/5034."),
            [5034, 5034]
        );
        assert!(gold_ratio_citations("see src/grammar for the gold path").is_empty());
        assert!(gold_ratio_citations("the ratio 12/12 on a plain line").is_empty());
    }

    #[test]
    fn gold_ratio_citations_survives_multibyte_section_refs() {
        assert!(
            gold_ratio_citations(
                "not in the gold corpus; oracle'd by the seed corpus (gap report §5/G2)."
            )
            .is_empty()
        );
        assert_eq!(
            gold_ratio_citations("§8 gold soundness stays 5034/5034 (see §5.8)"),
            [5034, 5034]
        );
    }

    #[test]
    fn gold_ratio_citations_handles_comments_whitespace_and_quotes() {
        assert_eq!(
            gold_ratio_citations("assert_eq!(n, 5034); // gold stays 5034/5034"),
            [5034, 5034]
        );
        assert_eq!(
            gold_ratio_citations("# gold soundness note: 5,034/5,034 replayed"),
            [5034, 5034]
        );
        assert_eq!(
            gold_ratio_citations(r#"the gold ratio is "5034/5034" today"#),
            [5034, 5034]
        );
        assert!(gold_ratio_citations("gold stays 5034 / 5034").is_empty());
        assert!(gold_ratio_citations("gold 5034/ 5034").is_empty());
        assert!(gold_ratio_citations("gold 5034 /5034").is_empty());
    }

    #[test]
    fn parse_grouped_strips_separators() {
        assert_eq!(parse_grouped("5,034"), Some(5034));
        assert_eq!(parse_grouped("395"), Some(395));
        assert_eq!(parse_grouped("nope"), None);
    }

    #[test]
    fn validate_name_accepts_plain_names() {
        assert!(validate_name("widget", "spec").is_ok());
    }

    #[test]
    fn validate_name_rejects_empty() {
        assert!(validate_name("", "spec").is_err());
    }

    #[test]
    fn validate_name_rejects_path_escapes() {
        assert!(validate_name("../../etc/passwd", "spec").is_err());
        assert!(validate_name("foo/bar", "spec").is_err());
        assert!(validate_name("foo\\bar", "spec").is_err());
        assert!(validate_name("..", "spec").is_err());
    }

    #[test]
    fn release_plz_override_names_extracts_package_names() {
        let src = "\
[workspace]
changelog_update = true

[changelog]
header = \"x\"

[[package]]
name = \"domain\"
publish = false

[[package]]
name=\"xtask\"
release = false
";
        assert_eq!(release_plz_override_names(src), ["domain", "xtask"]);
    }

    #[test]
    fn release_plz_override_names_ignores_non_package_name_keys() {
        // A `name = ` under a non-`[[package]]` table must not be collected.
        let src = "[workspace]\nname = \"not-a-package\"\n";
        assert!(release_plz_override_names(src).is_empty());
    }

    #[test]
    fn release_plz_override_names_strips_trailing_inline_comment() {
        // A trailing comment must not leak into the parsed name, or a valid
        // config would falsely trip the drift gate.
        let src = "[[package]]\nname = \"domain\" # keep in sync\n";
        assert_eq!(release_plz_override_names(src), ["domain"]);
    }

    #[test]
    fn missing_overrides_flags_non_members() {
        let overrides = ["domain".to_string(), "lints".to_string()];
        let members = ["domain".to_string(), "xtask".to_string()];
        assert_eq!(missing_overrides(&overrides, &members), ["lints"]);
    }

    #[test]
    fn missing_overrides_empty_when_all_present() {
        let overrides = ["domain".to_string(), "xtask".to_string()];
        let members = ["domain".to_string(), "xtask".to_string()];
        assert!(missing_overrides(&overrides, &members).is_empty());
    }

    #[test]
    fn inherits_workspace_lints_detects_the_table() {
        assert!(inherits_workspace_lints("[lints]\nworkspace = true\n"));
        assert!(inherits_workspace_lints(
            "[package]\nname = \"x\"\n\n[lints]\nworkspace = true\n\n[dependencies]\n"
        ));
    }

    #[test]
    fn inherits_workspace_lints_rejects_absent_or_false() {
        assert!(!inherits_workspace_lints("[package]\nname = \"x\"\n"));
        assert!(!inherits_workspace_lints("[lints]\nworkspace = false\n"));
        // A `workspace = true` outside a `[lints]` table must not count.
        assert!(!inherits_workspace_lints(
            "[dependencies]\nworkspace = true\n"
        ));
    }

    #[test]
    fn root_lint_problems_empty_when_contract_met() {
        let src = "\
[workspace.lints.rust]
unsafe_code = \"forbid\"
missing_docs = \"deny\"
";
        assert!(root_lint_problems(src).is_empty());
    }

    #[test]
    fn root_lint_problems_flags_missing_lint() {
        let src = "[workspace.lints.rust]\nunsafe_code = \"forbid\"\n";
        let problems = root_lint_problems(src);
        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains("missing_docs"));
    }

    #[test]
    fn root_lint_problems_flags_weaker_level() {
        // A downgraded level must trip the gate, not just an absent key.
        let src = "\
[workspace.lints.rust]
unsafe_code = \"forbid\"
missing_docs = \"warn\"
";
        let problems = root_lint_problems(src);
        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains("missing_docs"));
        assert!(problems[0].contains("must be \"deny\""));
    }

    #[test]
    fn table_string_value_strips_trailing_comment() {
        let src = "[lints]\nlevel = \"deny\" # keep strict\n";
        assert_eq!(
            table_string_value("lints", "level", src).as_deref(),
            Some("deny")
        );
    }

    // Build metadata `Value`s by hand rather than via `serde_json::json!`: the
    // macro expands to an internal `.unwrap()`, which the `disallowed_methods`
    // clippy lint forbids everywhere, tests included.
    fn dependency(
        name: &str,
        kind: Option<&str>,
        optional: bool,
        rename: Option<&str>,
    ) -> serde_json::Value {
        let mut dependency = serde_json::Map::new();
        dependency.insert("name".to_string(), serde_json::Value::from(name));
        dependency.insert(
            "kind".to_string(),
            kind.map_or(serde_json::Value::Null, serde_json::Value::from),
        );
        dependency.insert("optional".to_string(), serde_json::Value::from(optional));
        dependency.insert(
            "rename".to_string(),
            rename.map_or(serde_json::Value::Null, serde_json::Value::from),
        );
        serde_json::Value::Object(dependency)
    }

    fn package_with_dependencies(
        name: &str,
        dependencies: Vec<serde_json::Value>,
    ) -> serde_json::Value {
        let mut package = serde_json::Map::new();
        package.insert("name".to_string(), serde_json::Value::from(name));
        package.insert(
            "dependencies".to_string(),
            serde_json::Value::Array(dependencies),
        );
        serde_json::Value::Object(package)
    }

    fn package(name: &str, deps: &[(&str, Option<&str>)]) -> serde_json::Value {
        package_with_dependencies(
            name,
            deps.iter()
                .map(|(dependency_name, kind)| dependency(dependency_name, *kind, false, None))
                .collect(),
        )
    }

    #[test]
    fn layering_violations_allows_the_documented_dag_edges() {
        // The real workspace shape: every edge follows the DAG.
        let packages = [
            package("pure-analyzer-diagnostics", &[]),
            package("pure-analyzer-lexer", &[]),
            package("pure-analyzer-syntax", &[("pure-analyzer-lexer", None)]),
            package(
                "pure-analyzer-parser",
                &[
                    ("pure-analyzer-lexer", None),
                    ("pure-analyzer-syntax", None),
                    ("pure-analyzer-diagnostics", None),
                ],
            ),
            package(
                "libpure",
                &[
                    ("pure-analyzer-syntax", None),
                    ("pure-analyzer-parser", None),
                    ("pure-analyzer-diagnostics", None),
                ],
            ),
            // A non-DAG crate depending on a DAG crate is fine and ignored.
            package("xtask", &[("pure-analyzer-diagnostics", None)]),
        ];
        assert!(layering_violations(&packages).is_empty());
    }

    #[test]
    fn resolver_may_depend_on_model_but_model_may_not_depend_on_resolver() {
        let allowed = [package(
            "pure-analyzer-resolve",
            &[("pure-analyzer-model", None)],
        )];
        assert!(layering_violations(&allowed).is_empty());

        let forbidden = [package(
            "pure-analyzer-model",
            &[("pure-analyzer-resolve", None)],
        )];
        assert_eq!(
            layering_violations(&forbidden),
            ["pure-analyzer-model --(normal)--> pure-analyzer-resolve"]
        );
    }

    #[test]
    fn layering_violations_flags_a_reverse_edge() {
        // `pure-analyzer-lexer` is the DAG's base; it must never depend on
        // something built on top of it.
        let packages = [package(
            "pure-analyzer-lexer",
            &[("pure-analyzer-syntax", None)],
        )];
        assert_eq!(
            layering_violations(&packages),
            ["pure-analyzer-lexer --(normal)--> pure-analyzer-syntax"]
        );
    }

    #[test]
    fn layering_violations_flags_dev_and_build_edges_too() {
        // The gap `cargo-deny` misses: a dev- or build-dependency onto a
        // disallowed crate is still a layering violation.
        let packages = [
            package(
                "pure-analyzer-model",
                &[("pure-analyzer-analysis", Some("dev"))],
            ),
            package("pure-analyzer-resolve", &[("libpure", Some("build"))]),
        ];
        assert_eq!(
            layering_violations(&packages),
            [
                "pure-analyzer-model --(dev)--> pure-analyzer-analysis",
                "pure-analyzer-resolve --(build)--> libpure"
            ]
        );
    }

    #[test]
    fn workspace_member_classifier_covers_both_products_and_orchestration() {
        assert_eq!(
            workspace_member_class("pure-analyzer-parser"),
            Some(WorkspaceMemberClass::Analyzer)
        );
        assert_eq!(
            workspace_member_class(PURECARD_PACKAGE),
            Some(WorkspaceMemberClass::Purecard)
        );
        assert_eq!(
            workspace_member_class(ORCHESTRATION_PACKAGE),
            Some(WorkspaceMemberClass::Orchestration)
        );
        assert_eq!(
            workspace_member_class("fuzz"),
            Some(WorkspaceMemberClass::Analyzer)
        );
        assert_eq!(
            workspace_member_class("purecard-fuzz"),
            Some(WorkspaceMemberClass::Purecard)
        );
        assert_eq!(
            workspace_member_class("lints"),
            Some(WorkspaceMemberClass::Purecard)
        );
        assert_eq!(workspace_member_class("serde"), None);
    }

    #[test]
    fn unclassified_workspace_members_are_rejected_deterministically() {
        let packages = [
            package("z-new-product", &[]),
            package(ORCHESTRATION_PACKAGE, &[]),
            package(PURECARD_PACKAGE, &[]),
            package("pure-analyzer-lexer", &[]),
            package("a-new-product", &[]),
        ];
        assert_eq!(
            unclassified_workspace_members(&packages),
            ["a-new-product", "z-new-product"]
        );
    }

    #[test]
    fn cross_product_violations_cover_every_cargo_dependency_shape() {
        let packages = [
            package_with_dependencies(
                "pure-analyzer-parser",
                vec![
                    dependency(PURECARD_PACKAGE, None, false, None),
                    dependency(PURECARD_PACKAGE, Some("dev"), false, None),
                    dependency(PURECARD_PACKAGE, Some("build"), false, None),
                    dependency(PURECARD_PACKAGE, None, true, None),
                    dependency(PURECARD_PACKAGE, None, false, Some("decoder")),
                ],
            ),
            package_with_dependencies(
                PURECARD_PACKAGE,
                vec![dependency("pure-analyzer-parser", None, false, None)],
            ),
        ];
        let expected = [
            "pure-analyzer-parser --(build)--> pure-analyzer-purecard",
            "pure-analyzer-parser --(dev)--> pure-analyzer-purecard",
            "pure-analyzer-parser --(normal)--> pure-analyzer-purecard",
            "pure-analyzer-parser --(normal, optional)--> pure-analyzer-purecard",
            "pure-analyzer-parser --(normal, renamed as decoder)--> pure-analyzer-purecard",
            "pure-analyzer-purecard --(normal)--> pure-analyzer-parser",
        ];
        assert_eq!(cross_product_violations(&packages), expected);

        let mut reversed = packages.clone();
        reversed.reverse();
        assert_eq!(cross_product_violations(&reversed), expected);
    }

    #[test]
    fn excluded_paths_require_every_classified_boundary() -> Result<()> {
        let exclusions: Vec<toml::Value> = EXCLUDED_PACKAGE_BOUNDARIES
            .iter()
            .map(|(relative, _, _)| toml::Value::String((*relative).to_string()))
            .collect();
        let classified = classified_excluded_paths(&exclusions)?;
        let paths: Vec<&str> = classified.iter().map(|(relative, _)| *relative).collect();
        assert_eq!(
            paths,
            [
                "crates/pure-analyzer-purecard/fuzz",
                "crates/pure-analyzer-purecard/lints",
                "fuzz",
            ]
        );
        Ok(())
    }

    #[test]
    fn excluded_paths_fail_closed_when_an_expected_boundary_is_missing() -> Result<()> {
        let exclusions: Vec<toml::Value> = EXCLUDED_PACKAGE_BOUNDARIES
            .iter()
            .filter(|(relative, _, _)| *relative != "crates/pure-analyzer-purecard/lints")
            .map(|(relative, _, _)| toml::Value::String((*relative).to_string()))
            .collect();
        let error = match classified_excluded_paths(&exclusions) {
            Ok(_) => anyhow::bail!("expected a missing workspace.exclude path to fail"),
            Err(error) => error,
        };
        assert_eq!(
            error.to_string(),
            "root Cargo.toml workspace.exclude is missing expected product-boundary path(s): \
             `crates/pure-analyzer-purecard/lints`"
        );
        Ok(())
    }

    #[test]
    fn excluded_manifest_name_mismatch_diagnostic_is_stable() -> Result<()> {
        let error = match validate_excluded_package_name("fuzz", "fuzz", "renamed-fuzz") {
            Ok(()) => anyhow::bail!("expected a mismatched excluded package name to fail"),
            Err(error) => error,
        };
        assert_eq!(
            error.to_string(),
            "workspace.exclude path `fuzz` is classified as package `fuzz` \
             but its manifest declares `renamed-fuzz`"
        );
        Ok(())
    }

    #[test]
    fn excluded_manifest_dependencies_cover_kinds_aliases_and_targets() -> Result<()> {
        let excluded = manifest_package_value(
            r#"
[package]
name = "purecard-fuzz"

[dependencies]
decoder = { package = "pure-analyzer-model", path = "../../model", optional = true }

[dev-dependencies]
pure-analyzer-parser = { path = "../../parser" }

[target.'cfg(unix)'.build-dependencies]
pure-analyzer-analysis = { path = "../../analysis" }
"#,
            "fixture/Cargo.toml",
        )?;
        let packages = [
            excluded,
            package("pure-analyzer-model", &[]),
            package("pure-analyzer-parser", &[]),
            package("pure-analyzer-analysis", &[]),
        ];
        assert_eq!(
            cross_product_violations(&packages),
            [
                "purecard-fuzz --(build)--> pure-analyzer-analysis",
                "purecard-fuzz --(dev)--> pure-analyzer-parser",
                "purecard-fuzz --(normal, optional, renamed as decoder)--> pure-analyzer-model",
            ]
        );
        Ok(())
    }

    #[test]
    fn excluded_manifest_workspace_dependencies_fail_closed() {
        let result = manifest_package_value(
            r#"
[package]
name = "purecard-fuzz"

[dependencies]
decoder.workspace = true
"#,
            "fixture/Cargo.toml",
        );
        assert!(result.is_err());
    }

    #[test]
    fn product_boundary_ignores_product_named_dependencies_absent_from_workspace() {
        let packages = [package("pure-analyzer-lexer", &[(PURECARD_PACKAGE, None)])];
        assert!(cross_product_violations(&packages).is_empty());
    }

    #[test]
    fn product_boundary_allows_xtask_orchestration_and_external_dependencies() {
        let packages = [
            package("pure-analyzer-lexer", &[("logos", None)]),
            package(PURECARD_PACKAGE, &[("serde", None)]),
            package(
                ORCHESTRATION_PACKAGE,
                &[
                    ("pure-analyzer-lexer", None),
                    (PURECARD_PACKAGE, Some("dev")),
                    ("anyhow", None),
                ],
            ),
        ];
        assert!(cross_product_violations(&packages).is_empty());
        assert!(unclassified_workspace_members(&packages).is_empty());
        assert!(layering_violations(&packages).is_empty());
    }

    #[test]
    fn layering_diagnostic_cites_the_governing_adrs_by_failure_class() {
        assert_eq!(
            layering_diagnostic(
                &["analyzer-edge".to_string()],
                &["product-edge".to_string()],
                &["new-member".to_string()]
            ),
            "analysis-engine DAG violations (constitution §1, ADR-0003): analyzer-edge; \
             cross-product dependency violations (ADR-0004/ADR-0009): product-edge; \
             unclassified workspace members (ADR-0004/ADR-0009): new-member"
        );
    }

    /// Workspace root, derived from this crate's manifest dir (`<root>/xtask`).
    fn workspace_root() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .map(std::path::Path::to_path_buf)
            .unwrap_or_default()
    }

    /// Extract the set of recipe names declared in a `justfile`. A recipe line
    /// begins in column zero with the recipe name; `set …` directives and
    /// comments are not recipes.
    fn justfile_recipes(text: &str) -> std::collections::HashSet<String> {
        text.lines()
            .filter(|line| {
                line.starts_with(|c: char| c.is_ascii_lowercase()) && !line.starts_with("set ")
            })
            .filter_map(|line| {
                let name: String = line
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
                    .collect();
                (!name.is_empty()).then_some(name)
            })
            .collect()
    }

    /// Extract every `just <target>` invoked from a workflow `run:` step.
    fn workflow_just_targets(text: &str) -> Vec<String> {
        const MARKER: &str = "run:";
        const CALL: &str = "just ";
        text.lines()
            .filter(|line| line.contains(MARKER))
            .filter_map(|line| line.split_once(CALL))
            .map(|(_, rest)| {
                rest.chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
                    .collect::<String>()
            })
            .filter(|target| !target.is_empty())
            .collect()
    }

    #[test]
    fn justfile_recipes_parses_names_and_skips_directives() {
        let text = "set shell := [\"bash\"]\ndefault:\n    @just --list\ntest-scripts:\n    bun test scripts/\n";
        let recipes = justfile_recipes(text);
        assert!(recipes.contains("default"));
        assert!(recipes.contains("test-scripts"));
        assert!(!recipes.contains("set"));
    }

    #[test]
    fn workflow_just_targets_extracts_run_invocations() {
        let text =
            "      - name: x\n        run: just release-plz-check\n      - run: cargo xtask ci\n";
        assert_eq!(workflow_just_targets(text), ["release-plz-check"]);
    }

    /// The CI↔just bijection (constitution §2.4): every check CI reaches through
    /// `just <target>` must be a real recipe, so CI cannot invoke a gate that a
    /// contributor can't run locally by the same name. This is the class-closing
    /// gate for the missing `test-scripts` / `postponed-markers` targets.
    #[test]
    fn every_ci_just_target_is_a_real_recipe() {
        let root = workspace_root();
        let justfile = std::fs::read_to_string(root.join("justfile")).expect("read justfile");
        let recipes = justfile_recipes(&justfile);

        let workflows = std::fs::read_dir(root.join(".github/workflows")).expect("read workflows");
        let mut checked = 0usize;
        for entry in workflows {
            let path = entry.expect("dir entry").path();
            if path.extension().is_some_and(|e| e == "yml" || e == "yaml") {
                let text = std::fs::read_to_string(&path).expect("read workflow");
                for target in workflow_just_targets(&text) {
                    assert!(
                        recipes.contains(&target),
                        "workflow {} runs `just {target}`, which is not a justfile recipe",
                        path.display()
                    );
                    checked += 1;
                }
            }
        }
        assert!(checked > 0, "expected at least one `just` invocation in CI");
    }
}
