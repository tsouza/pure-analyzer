#!/usr/bin/env bun
// Plan a fail-closed incremental cargo-mutants run. Direct-PR CI may defer
// known full fallbacks to nightly while keeping planner-integrity failures hard.
import { appendFile, mkdir, open, rename, rm } from "node:fs/promises";
import { dirname, join } from "node:path";

import { notice } from "./lib/ci.mjs";
import { repoRoot } from "./lib/git.mjs";
import { runCommand } from "./lib/process.mjs";

export const FULL_MUTATION_SHARDS = 12;
export const MUTANTS_PER_DIFF_SHARD = 75;
export const PR_DIFF_MUTANTS_PER_SHARD = 12;
export const PR_DIFF_MAX_SHARDS = 3;
export const MUTATION_COMMAND_TIMEOUT_SECONDS = "120";
export const PLANNER_COMMAND_TIMEOUT_MS = 2 * 60 * 1_000;
export const PLANNER_COMMAND_MAX_BUFFER_BYTES = 8 * 1024 * 1024;
export const FFI_SOURCE = "crates/pure-analyzer-purecard/src/ffi.rs";
export const MUTATION_DIFF_PATH = "target/mutation-scope.diff";

const GIT_SHA = /^(?:[0-9a-f]{40}|[0-9a-f]{64})$/;
const SHA256 = /^[0-9a-f]{64}$/;
const DOCUMENTATION_ROOT = "docs/";
const ROOT_DOCUMENTATION_FILES = new Set([
  "CLAUDE.md",
  "CODE_OF_CONDUCT.md",
  "CONTRIBUTING.md",
  "README.md",
  "SECURITY.md",
  "constitution.md",
]);
const PRODUCTION_RUST = /^crates\/[^/]+\/src\/.+\.rs$/;
// Test-only paths, and source files declaring inline test attributes, take full.
const TEST_ONLY_RUST = /^crates\/[^/]+\/(?:tests\/.+\.rs$|src\/(?:tests?(?:\/|\.rs$)|test_[^/]*\.rs$|[^/]*_tests?\.rs$))/;
const INLINE_TEST_ATTRIBUTE = /#\s*\[\s*(?:(?:[A-Za-z_]\w*::)*\w*test\w*\b[^\]]*|cfg(?:_attr)?\s*\([\s\S]*?\btest\b[\s\S]*?\))\s*\]/;
const INLINE_TEST_MACRO = /\b(?:\w*test\w*|quickcheck)\s*!/;
const RUSTDOC_FENCE = /```/;
const DIFF_OPTIONS = ["--no-ext-diff", "--no-renames", "--no-textconv", "--unified=0"];
const USAGE = "usage: bun scripts/mutation-scope.mjs <plan|prepare>";
const DEFERRED_FULL_REASONS = new Set([
  "documentation-change",
  "empty-change-set",
  "inline-test-surface",
  "non-production-or-configuration-change",
  "non-pull-request-event",
  "rename-delete-or-type-change",
  "test-only-change-set",
  "zero-diff-mutant-list",
]);

/** Parse NUL-delimited `git diff --name-status -z` output. */
export function parseNameStatus(output) {
  const fields = output.split("\0");
  const changes = [];
  let index = 0;

  const next = () => {
    const field = fields[index++];
    if (!field) throw new Error("malformed NUL-delimited git name-status output");
    return field;
  };

  while (index < fields.length - 1) {
    const status = next();
    const kind = status[0];
    if (!kind) throw new Error("git name-status record has no status");
    const paths = kind === "R" || kind === "C" ? [next(), next()] : [next()];
    changes.push({ status, kind, paths });
  }
  if (index !== fields.length - 1 || fields.at(-1) !== "") {
    throw new Error("malformed NUL-delimited git name-status terminator");
  }
  return changes;
}

/** Whether a path belongs to repository documentation. */
export function isDocumentationPath(path) {
  return path.startsWith(DOCUMENTATION_ROOT) || ROOT_DOCUMENTATION_FILES.has(path);
}

/** Whether a value is a complete lowercase SHA-1 or SHA-256 object name. */
export function isGitSha(value) {
  return GIT_SHA.test(value);
}

/** Whether a changed path is production Rust eligible for an incremental mutant pass. */
export function isDiffEligibleRustPath(path) {
  return (
    path !== FFI_SOURCE &&
    PRODUCTION_RUST.test(path) &&
    !TEST_ONLY_RUST.test(path)
  );
}

/** Whether a Rust path is test-only rather than a production mutation target. */
export function isTestOnlyRustPath(path) {
  return TEST_ONLY_RUST.test(path);
}

/** Whether either revision of an eligible source change declares a test surface. */
export async function hasInlineTestSurface(
  root,
  mergeBase,
  headSha,
  changes,
  readSource = readRevisionSource,
) {
  for (const change of changes) {
    const [path] = change.paths;
    if (!isDiffEligibleRustPath(path)) continue;

    const headSource = await readSource(root, headSha, path);
    if (hasTestSurface(headSource)) return true;
    if (change.kind !== "A") {
      const baseSource = await readSource(root, mergeBase, path);
      if (hasTestSurface(baseSource)) return true;
    }
  }
  return false;
}

function hasTestSurface(source) {
  return (
    INLINE_TEST_ATTRIBUTE.test(source) ||
    INLINE_TEST_MACRO.test(source) ||
    RUSTDOC_FENCE.test(source)
  );
}

/** Classify a checked-out change set, including conservative inline-test detection. */
export async function classifyCheckedOutChanges(
  root,
  mergeBase,
  headSha,
  changes,
  readSource = readRevisionSource,
) {
  const classification = classifyChanges(changes);
  if (classification.scope !== "diff") return classification;
  try {
    if (await hasInlineTestSurface(root, mergeBase, headSha, changes, readSource)) {
      return { scope: "full", reason: "inline-test-surface" };
    }
  } catch {
    return { scope: "full", reason: "inline-test-inspection-failed" };
  }
  return classification;
}

/** Classify changed paths without ever treating an unknown shape as incremental. */
export function classifyChanges(changes) {
  if (changes.length === 0) return { scope: "full", reason: "empty-change-set" };
  const nonAddModify = changes.find(
    ({ kind }) => kind !== "A" && kind !== "M",
  );
  if (nonAddModify) {
    return { scope: "full", reason: "rename-delete-or-type-change" };
  }

  const paths = changes.flatMap(({ paths }) => paths);
  if (paths.some(isDocumentationPath)) {
    // Rust permits source files and build scripts to consume arbitrary files.
    // A path-based documentation exemption cannot prove a documentation change
    // is inert, including when it accompanies an otherwise eligible Rust diff.
    return { scope: "full", reason: "documentation-change" };
  }

  // A test-only Rust path (its own `tests/` file, or an inline `src/tests.rs`
  // sibling) never contributes a mutant itself — cargo-mutants only mutates
  // production code — so its presence alongside an eligible production diff
  // must not force the whole PR to the full/deferred-to-skip floor. That was
  // the single largest driver of #276: the overwhelmingly common PR shape
  // (production `.rs` plus its own test file) always fell through this
  // escape hatch. Any OTHER ineligible path (docs, `Cargo.toml`, a workflow
  // file, the FFI boundary, an unrelated crate's test-only path shape that
  // isn't paired with production Rust) still forces `full`.
  const ineligiblePath = paths.find(
    (path) => !isDiffEligibleRustPath(path) && !isTestOnlyRustPath(path),
  );
  if (ineligiblePath) {
    return { scope: "full", reason: "non-production-or-configuration-change" };
  }

  if (!paths.some(isDiffEligibleRustPath)) {
    return { scope: "full", reason: "test-only-change-set" };
  }

  const reason = paths.some(isTestOnlyRustPath)
    ? "production-rust-and-tests"
    : "production-rust-only";
  return { scope: "diff", reason };
}

/** Number of balanced round-robin shards needed for a nonempty incremental set. */
export function diffShardCount(mutantCount) {
  if (!Number.isSafeInteger(mutantCount) || mutantCount < 1) return 0;
  return Math.min(
    FULL_MUTATION_SHARDS,
    Math.max(1, Math.ceil(mutantCount / MUTANTS_PER_DIFF_SHARD)),
  );
}

/** Number of shards for the bounded direct-PR mutation lane, or zero when deferred. */
export function prDiffShardCount(mutantCount) {
  if (!Number.isSafeInteger(mutantCount) || mutantCount < 1) return 0;
  const shardTotal = Math.ceil(mutantCount / PR_DIFF_MUTANTS_PER_SHARD);
  return shardTotal <= PR_DIFF_MAX_SHARDS ? shardTotal : 0;
}

/** Create the GitHub Actions matrix, including a sentinel for a skipped run. */
export function mutationMatrix(scope, total) {
  const shardTotal = scope === "skip" ? 1 : total;
  const report = scope === "diff" ? "diff" : scope === "full" ? "default" : "skip";
  const diagnostics = scope === "diff" ? "diff-shard" : scope === "full" ? "shard" : "skip";
  return {
    include: Array.from({ length: shardTotal }, (_, index) => ({
      diagnostics,
      index,
      report,
      scope,
      total: shardTotal,
    })),
  };
}

function fullPlan(reason) {
  return {
    scope: "full",
    reason,
    mergeBase: "",
    headSha: "",
    diffSha256: "",
    mutantCount: 0,
    matrix: mutationMatrix("full", FULL_MUTATION_SHARDS),
  };
}

function skippedPlan(reason) {
  return {
    scope: "skip",
    reason,
    mergeBase: "",
    headSha: "",
    diffSha256: "",
    mutantCount: 0,
    matrix: mutationMatrix("skip", 1),
  };
}

/** Return an event-level plan when incremental planning is categorically inapplicable. */
export function eventFallbackPlan(eventName, draft) {
  if (eventName !== "pull_request") return fullPlan("non-pull-request-event");
  if (draft === "true") return skippedPlan("draft-pull-request");
  return undefined;
}

/**
 * Apply the bounded direct-PR lane, deferring full proofs to a valid sentinel.
 *
 * A deferred `full` plan collapses to `skip` rather than running any
 * full-workspace coverage inline: an earlier version of this function ran
 * one rotating full-workspace shard instead, but that surfaced two
 * pre-existing problems live on PR #439 — a single full-workspace shard can
 * exceed the job's wall-time budget, and cargo-mutants reports each
 * surviving mutant as a `##[warning]` annotation that the repo-wide
 * warnings-are-errors sweep (`no-ci-warnings.mjs`) then hard-fails on,
 * which would make ci-gate red on every PR until the whole pre-existing
 * surviving-mutant backlog is cleared. See the follow-up issue for the
 * bounded-full-workspace-coverage design this still owes #276.
 */
export function deferFullPlan(plan, deferFull = false) {
  if (!deferFull) return plan;
  if (plan.scope === "full") {
    if (!DEFERRED_FULL_REASONS.has(plan.reason)) {
      throw new Error(`full mutation plan is not safely deferrable: ${plan.reason}`);
    }
    return skippedPlan(`deferred-${plan.reason}`);
  }
  if (plan.scope !== "diff") return plan;

  const shardTotal = prDiffShardCount(plan.mutantCount);
  if (shardTotal === 0) return skippedPlan("deferred-pr-diff-budget-exceeded");
  return { ...plan, matrix: mutationMatrix("diff", shardTotal) };
}

/** Turn a classified change set and list result into a fail-closed plan. */
export function planFromClassification(classification, details = {}) {
  if (classification.scope === "skip") return skippedPlan(classification.reason);
  if (classification.scope !== "diff") return fullPlan(classification.reason);

  const mutantCount = details.mutantCount;
  if (!Number.isSafeInteger(mutantCount) || mutantCount < 0) {
    return fullPlan("invalid-diff-mutant-list");
  }
  if (mutantCount === 0) return fullPlan("zero-diff-mutant-list");
  const shardTotal = diffShardCount(mutantCount);
  if (
    !isGitSha(details.mergeBase ?? "") ||
    !isGitSha(details.headSha ?? "") ||
    !SHA256.test(details.diffSha256 ?? "")
  ) {
    return fullPlan("invalid-diff-metadata");
  }
  return {
    scope: "diff",
    reason: classification.reason,
    mergeBase: details.mergeBase,
    headSha: details.headSha,
    diffSha256: details.diffSha256,
    mutantCount: details.mutantCount,
    matrix: mutationMatrix("diff", shardTotal),
  };
}

export async function run(command, cwd, options = {}) {
  const result = await runCommand(command, {
    ...options,
    cwd,
    killSignal: "SIGKILL",
    maxBuffer: options.maxBuffer ?? PLANNER_COMMAND_MAX_BUFFER_BYTES,
    timeoutMs: options.timeoutMs ?? PLANNER_COMMAND_TIMEOUT_MS,
  });
  if (result.outputLimitExceeded) {
    throw new Error(`\`${command.join(" ")}\` output exceeded the configured limit`);
  }
  if (result.code !== 0) {
    throw commandError(command, result);
  }
  return result.stdout;
}

function commandError(command, result) {
  const detail = result.stderr.trim() || result.stdout.trim() || "no command output";
  return new Error(`\`${command.join(" ")}\` exited ${result.code}: ${detail.slice(0, 1_000)}`);
}

async function readRevisionSource(root, revision, path) {
  return run(
    ["git", "show", `${revision}:${path}`],
    root,
    mutationListRunOptions(),
  );
}

async function checkedOutHead(root, headSha) {
  if (!isGitSha(headSha)) throw new Error("missing or invalid pull-request head SHA");
  const actual = (await run(["git", "rev-parse", "HEAD"], root)).trim();
  if (actual !== headSha) {
    throw new Error("checkout HEAD does not match the event-pinned pull-request head SHA");
  }
  return actual;
}

async function mergeBase(root, baseSha, headSha) {
  if (!isGitSha(baseSha)) throw new Error("missing or invalid pull-request base SHA");
  const resolved = (await run(["git", "merge-base", baseSha, headSha], root)).trim();
  if (!isGitSha(resolved)) throw new Error("git merge-base returned an invalid SHA");
  return resolved;
}

async function changedPaths(root, baseSha, headSha) {
  const output = await run(
    [
      "git",
      "diff",
      "--name-status",
      "-z",
      "--no-ext-diff",
      "--no-renames",
      "--no-textconv",
      baseSha,
      headSha,
    ],
    root,
  );
  return parseNameStatus(output);
}

/** Write the exact zero-context diff used by cargo-mutants and return its digest. */
export async function writeDiff(root, baseSha, headSha, outputPath, execute = runCommand) {
  await mkdir(dirname(outputPath), { recursive: true });
  const temporaryPath = `${outputPath}.${crypto.randomUUID()}.tmp`;
  const file = await open(temporaryPath, "wx");
  const hasher = new Bun.CryptoHasher("sha256");
  let byteLength = 0;
  let closed = false;
  let published = false;

  try {
    const result = await execute(
      ["git", "diff", ...DIFF_OPTIONS, baseSha, headSha, "--"],
      {
        cwd: root,
        ...mutationListRunOptions(),
        onStdout: async (chunk) => {
          let offset = 0;
          while (offset < chunk.byteLength) {
            const { bytesWritten } = await file.write(chunk.subarray(offset));
            if (bytesWritten === 0) {
              throw new Error("writing mutation diff made no progress");
            }
            offset += bytesWritten;
          }
          hasher.update(chunk);
          byteLength += chunk.byteLength;
        },
      },
    );
    if (result.outputLimitExceeded) {
      throw new Error("generated mutation diff exceeded the configured output limit");
    }
    if (result.code !== 0) throw commandError(["git", "diff"], result);
    if (byteLength === 0) throw new Error("generated mutation diff is empty");
    await file.close();
    closed = true;
    await rename(temporaryPath, outputPath);
    published = true;
    return hasher.digest("hex");
  } finally {
    if (!closed) await file.close().catch(() => {});
    if (!published) await rm(temporaryPath, { force: true }).catch(() => {});
  }
}

/** Build the listing command from the same workspace scope as mutation execution. */
export function mutationListCommand(diffPath) {
  return [
    "cargo",
    "mutants",
    "--workspace",
    "--exclude",
    FFI_SOURCE,
    "--in-place",
    "--timeout",
    MUTATION_COMMAND_TIMEOUT_SECONDS,
    "--in-diff",
    diffPath,
    "--list",
    "--json",
  ];
}

/** Fixed resource limits for every planner subprocess. */
export function mutationListRunOptions() {
  return {
    timeoutMs: PLANNER_COMMAND_TIMEOUT_MS,
    maxBuffer: PLANNER_COMMAND_MAX_BUFFER_BYTES,
  };
}

async function listedMutantCount(root, diffPath) {
  const output = await run(
    mutationListCommand(diffPath),
    root,
    mutationListRunOptions(),
  );
  const mutants = JSON.parse(output);
  if (!Array.isArray(mutants)) throw new Error("cargo-mutants list output was not an array");
  return mutants.length;
}

async function writeOutput(key, value) {
  const line = `${key}=${value}\n`;
  if (process.env.GITHUB_OUTPUT) {
    await appendFile(process.env.GITHUB_OUTPUT, line);
  } else {
    process.stdout.write(line);
  }
}

async function emitPlan(plan) {
  for (const [key, value] of [
    ["scope", plan.scope],
    ["reason", plan.reason],
    ["merge_base", plan.mergeBase],
    ["head_sha", plan.headSha],
    ["diff_sha256", plan.diffSha256],
    ["mutant_count", String(plan.mutantCount)],
    ["matrix", JSON.stringify(plan.matrix)],
  ]) {
    await writeOutput(key, value);
  }
  notice(
    `mutation scope=${plan.scope} reason=${plan.reason} mutants=${plan.mutantCount}`,
  );
}

export async function planFromEnvironment(environment = process.env, dependencies = {}) {
  const {
    checkedOutHead: verifyHead = checkedOutHead,
    changedPaths: listChangedPaths = changedPaths,
    classifyCheckedOutChanges: classify = classifyCheckedOutChanges,
    listedMutantCount: countMutants = listedMutantCount,
    mergeBase: findMergeBase = mergeBase,
    repoRoot: findRepoRoot = repoRoot,
    writeDiff: writeMutationDiff = writeDiff,
  } = dependencies;
  const deferFull = environment.MUTATION_DEFER_FULL === "true";
  const applyDeferral = (plan) => deferFullPlan(plan, deferFull);
  const eventFallback = eventFallbackPlan(
    environment.GITHUB_EVENT_NAME,
    environment.MUTATION_PR_DRAFT,
  );
  if (eventFallback) return applyDeferral(eventFallback);

  try {
    const root = await findRepoRoot();
    const headSha = await verifyHead(root, environment.MUTATION_HEAD_SHA ?? "");
    const baseSha = await findMergeBase(root, environment.MUTATION_BASE_SHA ?? "", headSha);
    const changes = await listChangedPaths(root, baseSha, headSha);
    const classification = await classify(
      root,
      baseSha,
      headSha,
      changes,
    );
    if (classification.scope !== "diff") {
      return applyDeferral(planFromClassification(classification));
    }

    const diffPath = join(environment.RUNNER_TEMP ?? root, "mutation-scope.diff");
    const diffSha256 = await writeMutationDiff(root, baseSha, headSha, diffPath);
    const mutantCount = await countMutants(root, diffPath);
    return applyDeferral(
      planFromClassification(classification, {
        diffSha256,
        headSha,
        mergeBase: baseSha,
        mutantCount,
      }),
    );
  } catch (error) {
    if (deferFull) {
      notice("incremental mutation planning failed; direct-PR CI fails closed");
      throw error;
    }
    notice("incremental mutation planning failed closed");
    return fullPlan("invalid-diff-or-mutant-list");
  }
}

/** Regenerate and verify the patch in a matrix worker before it mutates anything. */
export async function prepareVerifiedDiffFromEnvironment() {
  const baseSha = process.env.MUTATION_MERGE_BASE ?? "";
  const headSha = process.env.MUTATION_HEAD_SHA ?? "";
  const expectedDigest = process.env.MUTATION_DIFF_SHA256 ?? "";
  if (!isGitSha(baseSha) || !isGitSha(headSha) || !SHA256.test(expectedDigest)) {
    throw new Error("missing verified incremental mutation scope metadata");
  }
  const root = await repoRoot();
  await checkedOutHead(root, headSha);
  const diffPath = join(root, MUTATION_DIFF_PATH);
  const actualDigest = await writeDiff(root, baseSha, headSha, diffPath);
  if (actualDigest !== expectedDigest) {
    throw new Error("worker mutation diff does not match the planned merge-base diff");
  }
  notice(`verified incremental mutation diff ${actualDigest}`);
  return diffPath;
}

async function entrypoint(argv) {
  if (argv.length !== 1) throw new Error(USAGE);
  if (argv[0] === "plan") return emitPlan(await planFromEnvironment());
  if (argv[0] === "prepare") return prepareVerifiedDiffFromEnvironment();
  throw new Error(USAGE);
}

if (import.meta.main) {
  try {
    await entrypoint(process.argv.slice(2));
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    console.error(message);
    process.exitCode = 1;
  }
}
