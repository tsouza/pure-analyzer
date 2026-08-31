#!/usr/bin/env bun
// Plan a fail-closed incremental cargo-mutants run for a pull request. Direct
// PR runs use the event head; merge groups stay on the full synthetic candidate.
import { appendFile, mkdir, open, rename, rm } from "node:fs/promises";
import { dirname, join, posix } from "node:path";

import { notice } from "./lib/ci.mjs";
import { repoRoot } from "./lib/git.mjs";
import { runCommand } from "./lib/process.mjs";

export const FULL_MUTATION_SHARDS = 12;
export const MUTANTS_PER_DIFF_SHARD = 75;
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
const INCLUDE_FILE_CALL = /\binclude_(?:str|bytes)!\s*\(([\s\S]*?)\)/g;
// `git grep -E` accepts POSIX ERE rather than JavaScript regular expressions.
export const INCLUDE_FILE_GREP_PATTERN = "include_(str|bytes)!";
const USAGE = "usage: bun scripts/mutation-scope.mjs <plan|prepare>";

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

/** Whether a path is documentation only, with no executable/configuration meaning. */
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
  hasIncludedDocumentation = hasIncludedDocumentationSurface,
) {
  const classification = classifyChanges(changes);
  const hasDocumentationChange = changes
    .flatMap(({ paths }) => paths)
    .some(isDocumentationPath);
  if (classification.scope !== "full" && hasDocumentationChange) {
    try {
      if (await hasIncludedDocumentation(root, headSha, changes)) {
        return { scope: "full", reason: "included-documentation-surface" };
      }
    } catch {
      return { scope: "full", reason: "included-documentation-inspection-failed" };
    }
  }
  if (classification.scope === "skip") return classification;
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

async function hasIncludedDocumentationSurface(root, headSha, changes) {
  const documentationPaths = new Set(
    changes.flatMap(({ paths }) => paths).filter(isDocumentationPath),
  );
  const command = includeFileSearchCommand(headSha);
  const result = await runCommand(command, {
    cwd: root,
    ...mutationListRunOptions(),
  });
  if (result.outputLimitExceeded) {
    throw new Error("included-file source inventory exceeded the configured output limit");
  }
  if (result.code === 1) return false;
  if (result.code !== 0) throw commandError(command, result);

  const prefix = `${headSha}:`;
  for (const entry of result.stdout.split("\0")) {
    if (!entry) continue;
    if (!entry.startsWith(prefix)) {
      throw new Error("git grep returned an invalid revision-qualified source path");
    }
    const sourcePath = entry.slice(prefix.length);
    const source = await readRevisionSource(root, headSha, sourcePath);
    if (sourceIncludesDocumentation(source, sourcePath, documentationPaths)) return true;
  }
  return false;
}

/** Build the revision-qualified POSIX ERE search used for include-file inventory. */
export function includeFileSearchCommand(headSha) {
  return [
    "git",
    "grep",
    "-l",
    "-z",
    "--full-name",
    "-E",
    INCLUDE_FILE_GREP_PATTERN,
    headSha,
    "--",
    "*.rs",
  ];
}

export function sourceIncludesDocumentation(source, sourcePath, documentationPaths) {
  for (const match of source.matchAll(INCLUDE_FILE_CALL)) {
    const literal = /^"([^"\\]*)"$/.exec(match[1].trim());
    if (!literal) return true;
    // `include_str!` accepts absolute paths too. They cannot be resolved against
    // the repository's changed-path inventory, so treating one as harmless
    // would let a documentation change bypass mutation testing.
    if (posix.isAbsolute(literal[1])) return true;
    const includedPath = posix.normalize(posix.join(posix.dirname(sourcePath), literal[1]));
    // Likewise, never reason incrementally about an include that escapes the
    // checked-out tree. Git's source inventory is deliberately repo-relative.
    if (includedPath === ".." || includedPath.startsWith("../")) return true;
    if (documentationPaths.has(includedPath)) return true;
  }
  return false;
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

  const nonDocumentationPaths = changes
    .flatMap(({ paths }) => paths)
    .filter((path) => !isDocumentationPath(path));
  if (nonDocumentationPaths.length === 0) {
    return { scope: "skip", reason: "documentation-only" };
  }

  const ineligiblePath = nonDocumentationPaths.find(
    (path) => !isDiffEligibleRustPath(path),
  );
  if (ineligiblePath) {
    return { scope: "full", reason: "non-production-or-configuration-change" };
  }
  return { scope: "diff", reason: "production-rust-only" };
}

/** Number of balanced round-robin shards needed for a nonempty incremental set. */
export function diffShardCount(mutantCount) {
  if (!Number.isSafeInteger(mutantCount) || mutantCount < 1) return 0;
  return Math.min(
    FULL_MUTATION_SHARDS,
    Math.max(1, Math.ceil(mutantCount / MUTANTS_PER_DIFF_SHARD)),
  );
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

/** Turn a classified change set and list result into a fail-closed plan. */
export function planFromClassification(classification, details = {}) {
  if (classification.scope === "skip") return skippedPlan(classification.reason);
  if (classification.scope !== "diff") return fullPlan(classification.reason);

  const shardTotal = diffShardCount(details.mutantCount);
  if (shardTotal === 0) return fullPlan("zero-or-invalid-diff-mutant-list");
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

async function planFromEnvironment() {
  const eventFallback = eventFallbackPlan(
    process.env.GITHUB_EVENT_NAME,
    process.env.MUTATION_PR_DRAFT,
  );
  if (eventFallback) return eventFallback;

  try {
    const root = await repoRoot();
    const headSha = await checkedOutHead(root, process.env.MUTATION_HEAD_SHA ?? "");
    const baseSha = await mergeBase(root, process.env.MUTATION_BASE_SHA ?? "", headSha);
    const changes = await changedPaths(root, baseSha, headSha);
    const classification = await classifyCheckedOutChanges(
      root,
      baseSha,
      headSha,
      changes,
    );
    if (classification.scope !== "diff") return planFromClassification(classification);

    const diffPath = join(process.env.RUNNER_TEMP ?? root, "mutation-scope.diff");
    const diffSha256 = await writeDiff(root, baseSha, headSha, diffPath);
    const mutantCount = await listedMutantCount(root, diffPath);
    return planFromClassification(classification, {
      diffSha256,
      headSha,
      mergeBase: baseSha,
      mutantCount,
    });
  } catch (error) {
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
