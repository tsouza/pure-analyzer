//! Implementations of each `xtask` subcommand.
//!
//! Each task shells out to the underlying tool via [`crate::process`] and
//! propagates exit codes, so `xtask` stays a thin, auditable orchestrator.

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
    run_cargo_steps(&[
        &["fmt", "--all", "--check"],
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
    println!("  cd \"{worktree}\" && just spec {name}");
    Ok(())
}

/// Template for a new `specs/<name>.md` file. `{name}` and `{date}` are
/// substituted by [`spec`].
const SPEC_TEMPLATE: &str = "\
# Spec: {name}

- Status: draft
- Created: {date}
- Owner:

## Problem
What user-visible problem does this solve? Why now?

## Goals
- [ ]

## Non-goals
-

## Design
How it works, which crate(s) it touches in the analysis-engine DAG (lexer /
syntax / parser / model / resolve / analysis / libpure / cli), and how it
respects the layering invariants.

## API / contract impact
Public API, proto, or OpenAPI changes (if any) and their stability impact.

## Testing plan
Unit / integration / chaos / mutation / fuzz coverage for this change.

## Risks & rollout
Failure modes, feature-flagging, and how we roll back.
";

/// Scaffold a feature spec at `specs/<name>.md` from [`SPEC_TEMPLATE`].
///
/// # Errors
///
/// Returns an error if `name` is empty, a spec already exists at that path, or
/// the file cannot be written.
pub fn spec(name: &str) -> Result<()> {
    validate_name(name, "spec")?;
    let out = format!("specs/{name}.md");
    if std::path::Path::new(&out).exists() {
        anyhow::bail!("spec already exists: {out}");
    }
    std::fs::create_dir_all("specs").context("creating specs/")?;

    let contents = render_spec(name, &today_utc_ymd());
    std::fs::write(&out, contents).with_context(|| format!("writing {out}"))?;
    println!("Wrote {out}");
    Ok(())
}

/// Render [`SPEC_TEMPLATE`] with `name` and `date` substituted.
fn render_spec(name: &str, date: &str) -> String {
    SPEC_TEMPLATE
        .replace("{name}", name)
        .replace("{date}", date)
}

/// Seconds in a day.
const SECS_PER_DAY: u64 = 86_400;
/// Days in one common year.
const DAYS_PER_YEAR: i64 = 365;
/// Years in a 400-year proleptic-Gregorian era — the leap cycle the algorithm
/// folds on.
const YEARS_PER_ERA: i64 = 400;
/// Days in a 400-year era (its `YEARS_PER_ERA` years plus 97 leap days).
const DAYS_PER_ERA: i64 = 146_097;
/// Days in a 4-year cycle — a leap correction in the year-of-era formula.
const DAYS_PER_4_YEARS: i64 = 1_460;
/// Days in a 100-year cycle — a leap correction in the year-of-era formula.
const DAYS_PER_100_YEARS: i64 = 36_524;
/// Days from the algorithm's shifted epoch (0000-03-01) to the Unix epoch.
const EPOCH_SHIFT_DAYS: i64 = 719_468;

/// Today's UTC date as `YYYY-MM-DD`, computed in-process — no shell-out to the
/// platform `date` binary (absent/inconsistent across OSes; constitution §2
/// "portable automation").
fn today_utc_ymd() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let (year, month, day) = civil_from_days((secs / SECS_PER_DAY) as i64);
    format!("{year:04}-{month:02}-{day:02}")
}

/// Convert days since the Unix epoch to a proleptic-Gregorian `(year, month,
/// day)`. Howard Hinnant's exact, dependency-free algorithm, documented at
/// <https://howardhinnant.github.io/date_algorithms.html>.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + EPOCH_SHIFT_DAYS;
    let era = (if z >= 0 { z } else { z - (DAYS_PER_ERA - 1) }) / DAYS_PER_ERA;
    let doe = z - era * DAYS_PER_ERA;
    let yoe = (doe - doe / DAYS_PER_4_YEARS + doe / DAYS_PER_100_YEARS - doe / (DAYS_PER_ERA - 1))
        / DAYS_PER_YEAR;
    let year = yoe + era * YEARS_PER_ERA;
    let doy = doe - (DAYS_PER_YEAR * yoe + yoe / 4 - yoe / 100);
    // Hinnant's month-from-day-of-year fit; 5/2/153/3/9 are the algorithm's
    // polynomial coefficients, meaningful only within it.
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

/// The analysis-engine crate DAG (design doc §3, constitution §1, ADR-0003):
/// for each enforced workspace crate, the set of internal crates it may
/// depend on, in any dependency kind. This is a DAG, not a linear rank —
/// `pure-analyzer-model` and `pure-analyzer-resolve` are siblings that both
/// build on `pure-analyzer-parser` but neither depends on the other — so
/// membership is checked against an explicit allow-set per crate rather than
/// an inward/outward rank comparison. `pure-analyzer-diagnostics` is a leaf
/// every parser-and-above crate may depend on; the lexer and syntax layers
/// may not (they sit below the diagnostics-consuming boundary).
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
    violations
}

/// Fail if any workspace crate depends on another enforced crate outside its
/// documented DAG edges — in **any** dependency kind, including dev- and
/// build-dependencies.
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
/// # Errors
///
/// Returns an error naming each offending edge (with its kind).
pub fn verify_layering() -> Result<()> {
    let json = run_stdout("cargo", &["metadata", "--no-deps", "--format-version", "1"])?;
    let meta: serde_json::Value =
        serde_json::from_str(&json).context("parsing `cargo metadata` output")?;
    let packages = meta["packages"]
        .as_array()
        .context("`cargo metadata` has no packages array")?;

    let violations = layering_violations(packages);
    if !violations.is_empty() {
        anyhow::bail!(
            "forbidden internal dependency edge (constitution §1, ADR-0003): the analysis-engine \
             DAG (lexer -> syntax -> parser -> {{model, resolve}} -> analysis -> libpure -> cli, \
             with diagnostics as a shared leaf) allows dependencies only along its documented \
             edges. Offending edges: {}",
            violations.join(", ")
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
    fn civil_from_days_matches_known_anchors() {
        assert_eq!(civil_from_days(0), (1970, 1, 1)); // Unix epoch
        assert_eq!(civil_from_days(10_957), (2000, 1, 1)); // 30 years + 7 leap days
        assert_eq!(civil_from_days(11_016), (2000, 2, 29)); // exercises the leap day
        assert_eq!(civil_from_days(-1), (1969, 12, 31)); // day before the epoch
    }

    #[test]
    fn today_utc_ymd_is_well_formed() {
        let today = today_utc_ymd();
        assert_eq!(today.len(), 10);
        assert_eq!(today.matches('-').count(), 2);
        assert!(today.starts_with("20"));
    }

    #[test]
    fn render_spec_substitutes_name_and_date() {
        let out = render_spec("widget", "2026-07-05");
        assert!(out.contains("# Spec: widget"));
        assert!(out.contains("Created: 2026-07-05"));
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
    fn package(name: &str, deps: &[(&str, Option<&str>)]) -> serde_json::Value {
        let dependencies: Vec<serde_json::Value> = deps
            .iter()
            .map(|(dep_name, kind)| {
                let mut dep = serde_json::Map::new();
                dep.insert("name".to_string(), serde_json::Value::from(*dep_name));
                dep.insert(
                    "kind".to_string(),
                    kind.map_or(serde_json::Value::Null, serde_json::Value::from),
                );
                serde_json::Value::Object(dep)
            })
            .collect();
        let mut package = serde_json::Map::new();
        package.insert("name".to_string(), serde_json::Value::from(name));
        package.insert(
            "dependencies".to_string(),
            serde_json::Value::Array(dependencies),
        );
        serde_json::Value::Object(package)
    }

    #[test]
    fn layering_violations_allows_the_documented_dag_edges() {
        // The real workspace shape: every edge follows the DAG (design doc §3).
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
