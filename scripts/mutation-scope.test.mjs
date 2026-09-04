import { expect, test } from "bun:test";
import { mkdtemp, readdir, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import {
  FFI_SOURCE,
  FULL_MUTATION_SHARDS,
  MUTATION_COMMAND_TIMEOUT_SECONDS,
  PLANNER_COMMAND_MAX_BUFFER_BYTES,
  PR_DIFF_MAX_SHARDS,
  PR_DIFF_MUTANTS_PER_SHARD,
  MUTANTS_PER_DIFF_SHARD,
  classifyCheckedOutChanges,
  classifyChanges,
  deferFullPlan,
  diffShardCount,
  eventFallbackPlan,
  isDiffEligibleRustPath,
  isDocumentationPath,
  isGitSha,
  isTestOnlyRustPath,
  hasInlineTestSurface,
  mutationMatrix,
  mutationListCommand,
  mutationListRunOptions,
  parseNameStatus,
  planFromEnvironment,
  planFromClassification,
  prDiffShardCount,
  run,
  writeDiff,
} from "./mutation-scope.mjs";

const changed = (path, status = "M") => ({ kind: status[0], paths: [path], status });
const mergeBase = "a".repeat(40);
const headSha = "c".repeat(40);
const diffSha256 = "b".repeat(64);

test("parses NUL-safe add, modify, and rename records", () => {
  expect(
    parseNameStatus(
      "M\0crates/pure-analyzer-model/src/loader.rs\0A\0README.md\0R100\0before.md\0after.md\0",
    ),
  ).toEqual([
    changed("crates/pure-analyzer-model/src/loader.rs"),
    changed("README.md", "A"),
    { kind: "R", paths: ["before.md", "after.md"], status: "R100" },
  ]);
  expect(() => parseNameStatus("M\0missing-its-path\0A")).toThrow("malformed");
});

test("recognises only the explicit documentation root and files", () => {
  expect(isDocumentationPath("README.md")).toBeTrue();
  expect(isDocumentationPath("docs/architecture/ci.md")).toBeTrue();
  expect(
    isDocumentationPath(
      "crates/pure-analyzer-purecard/corpus/schemas/oos_ctx_dog_kennels.md",
    ),
  ).toBeFalse();
  expect(isDocumentationPath("crates/pure-analyzer-purecard/docs/README.md")).toBeFalse();
  expect(isDocumentationPath(".github/workflows/ci.yml")).toBeFalse();
  expect(isDocumentationPath("Cargo.toml")).toBeFalse();
});

test("accepts only complete lowercase SHA-1 or SHA-256 object names", () => {
  expect(isGitSha("a".repeat(40))).toBeTrue();
  expect(isGitSha("b".repeat(64))).toBeTrue();
  for (const invalid of [
    "a".repeat(39),
    "a".repeat(41),
    "a".repeat(63),
    "a".repeat(65),
    "A".repeat(40),
  ]) {
    expect(isGitSha(invalid)).toBeFalse();
  }
});

test("allows only crate production Rust excluding the dedicated FFI boundary", () => {
  expect(isDiffEligibleRustPath("crates/pure-analyzer-model/src/loader.rs")).toBeTrue();
  expect(isDiffEligibleRustPath("crates/pure-analyzer-model/tests/loader.rs")).toBeFalse();
  expect(isDiffEligibleRustPath("crates/pure-analyzer-model/src/tests.rs")).toBeFalse();
  expect(isDiffEligibleRustPath("crates/pure-analyzer-model/src/loader_test.rs")).toBeFalse();
  expect(isDiffEligibleRustPath("xtask/src/tasks.rs")).toBeFalse();
  expect(isDiffEligibleRustPath(FFI_SOURCE)).toBeFalse();
});

test("recognises conventional crate test-only Rust paths", () => {
  for (const path of [
    "crates/pure-analyzer-model/tests/loader.rs",
    "crates/pure-analyzer-model/src/tests.rs",
    "crates/pure-analyzer-model/src/tests/loader.rs",
    "crates/pure-analyzer-model/src/test_loader.rs",
    "crates/pure-analyzer-model/src/loader_test.rs",
  ]) {
    expect(isTestOnlyRustPath(path)).toBeTrue();
  }
});

test("finds inline test attributes in both revisions of eligible production sources", async () => {
  const baseRevision = "base";
  const headRevision = "head";
  const sources = new Map([
    [
      `${baseRevision}:crates/pure-analyzer-model/src/loader.rs`,
      "pub fn loader() {}",
    ],
    [
      `${headRevision}:crates/pure-analyzer-model/src/loader.rs`,
      "pub fn loader() {}",
    ],
    [
      `${baseRevision}:crates/pure-analyzer-model/src/model.rs`,
      "pub fn model() {}",
    ],
    [
      `${headRevision}:crates/pure-analyzer-model/src/model.rs`,
      "#[cfg_attr(all(test, feature = \"nightly\"), allow(dead_code))]\nmod model {}",
    ],
    [
      `${baseRevision}:crates/pure-analyzer-model/src/removed.rs`,
      "#[cfg(all(test, feature = \"nightly\"))]\nmod tests {}",
    ],
    [
      `${headRevision}:crates/pure-analyzer-model/src/removed.rs`,
      "pub fn production_only() {}",
    ],
    [
      `${baseRevision}:crates/pure-analyzer-model/src/standalone.rs`,
      "pub fn standalone() {}",
    ],
    [
      `${headRevision}:crates/pure-analyzer-model/src/standalone.rs`,
      "#[test]\nfn standalone_test() {}",
    ],
    [
      `${headRevision}:crates/pure-analyzer-model/src/added.rs`,
      "pub fn added() {}",
    ],
  ]);
  const readSource = async (_root, revision, path) => {
    const source = sources.get(`${revision}:${path}`);
    if (source === undefined) throw new Error("source is absent");
    return source;
  };

  await expect(
    hasInlineTestSurface(
      "/workspace",
      baseRevision,
      headRevision,
      [
        changed("crates/pure-analyzer-model/src/loader.rs"),
        changed("crates/pure-analyzer-model/src/model.rs"),
      ],
      readSource,
    ),
  ).resolves.toBeTrue();
  await expect(
    hasInlineTestSurface(
      "/workspace",
      baseRevision,
      headRevision,
      [changed("crates/pure-analyzer-model/src/loader.rs")],
      readSource,
    ),
  ).resolves.toBeFalse();
  await expect(
    classifyCheckedOutChanges(
      "/workspace",
      baseRevision,
      headRevision,
      [changed("crates/pure-analyzer-model/src/removed.rs")],
      readSource,
    ),
  ).resolves.toEqual({ reason: "inline-test-surface", scope: "full" });
  await expect(
    classifyCheckedOutChanges(
      "/workspace",
      baseRevision,
      headRevision,
      [changed("crates/pure-analyzer-model/src/added.rs", "A")],
      readSource,
    ),
  ).resolves.toEqual({ reason: "production-rust-only", scope: "diff" });
  await expect(
    classifyCheckedOutChanges(
      "/workspace",
      baseRevision,
      headRevision,
      [changed("crates/pure-analyzer-model/src/standalone.rs")],
      readSource,
    ),
  ).resolves.toEqual({ reason: "inline-test-surface", scope: "full" });
  await expect(
    classifyCheckedOutChanges(
      "/workspace",
      baseRevision,
      headRevision,
      [changed("crates/pure-analyzer-model/src/loader.rs")],
      async () => {
        throw new Error("source is unreadable");
      },
    ),
  ).resolves.toEqual({ reason: "inline-test-inspection-failed", scope: "full" });
});

test("finds attribute, macro, and doctest surfaces in either source revision", async () => {
  const baseRevision = "base";
  const headRevision = "head";
  const surfaces = [
    {
      base: "pub fn production_only() {}",
      head: "#[tokio::test]\nasync fn asynchronous() {}",
      path: "crates/pure-analyzer-model/src/tokio.rs",
    },
    {
      base: "#[rstest]\nfn parameterized() {}",
      head: "pub fn production_only() {}",
      path: "crates/pure-analyzer-model/src/rstest.rs",
    },
    {
      base: "pub fn production_only() {}",
      head: "#[test_case(1)]\nfn table_driven(_: u8) {}",
      path: "crates/pure-analyzer-model/src/cases.rs",
    },
    {
      base: "#[proptest]\nfn generated() {}",
      head: "pub fn production_only() {}",
      path: "crates/pure-analyzer-model/src/proptest_attribute.rs",
    },
    {
      base: "pub fn production_only() {}",
      head: "proptest! { #[test] fn generated() {} }",
      path: "crates/pure-analyzer-model/src/proptest_macro.rs",
    },
    {
      base: "quickcheck! { fn generated() -> bool { true } }",
      head: "pub fn production_only() {}",
      path: "crates/pure-analyzer-model/src/quickcheck_macro.rs",
    },
    {
      base: "/**\n * ```rust\n * assert!(true);\n * ```\n */",
      head: "pub fn production_only() {}",
      path: "crates/pure-analyzer-model/src/block_doctest.rs",
    },
    {
      base: "pub fn production_only() {}",
      head: "/// ```rust\n/// assert!(true);\n/// ```\npub fn documented() {}",
      path: "crates/pure-analyzer-model/src/line_doctest.rs",
    },
    {
      base: "pub fn production_only() {}",
      head: "#[doc = \"```rust\\nassert!(true);\\n```\"]\npub fn documented() {}",
      path: "crates/pure-analyzer-model/src/attribute_doctest.rs",
    },
  ];
  const sources = new Map(
    surfaces.flatMap(({ base, head, path }) => [
      [`${baseRevision}:${path}`, base],
      [`${headRevision}:${path}`, head],
    ]),
  );
  const readSource = async (_root, revision, path) => {
    const source = sources.get(`${revision}:${path}`);
    if (source === undefined) throw new Error("source is absent");
    return source;
  };

  for (const { path } of surfaces) {
    await expect(
      hasInlineTestSurface(
        "/workspace",
        baseRevision,
        headRevision,
        [changed(path)],
        readSource,
      ),
    ).resolves.toBeTrue();
  }
});

// Note: this is the classifier's PRE-deferral verdict only. On a real PR,
// `MUTATION_DEFER_FULL=true` rewrites this `full` into `skip` (see "bounds
// deferred direct-PR mutation plans..." below) — it is never a guarantee
// that a full mutation pass runs inline.
test("classifies production Rust plus documentation as full before PR-lane deferral", () => {
  expect(
    classifyChanges([
      changed("crates/pure-analyzer-model/src/loader.rs"),
      changed("docs/architecture/ci.md"),
    ]),
  ).toEqual({ reason: "documentation-change", scope: "full" });
});

// The overwhelmingly common PR shape — production Rust plus its own test
// file(s) — must stay in `diff` scope: a test-only path never contributes a
// mutant itself, so its presence must not push the whole PR to the
// full/deferred-to-skip floor. Before this fix, EVERY such PR was classified
// `full` here (see #276: 60/60 recent PR runs skipped mutation entirely).
// This is the test #310 asked for: a realistic multi-file diff (production
// Rust + its test) must yield `scope: "diff"`, not `full`.
test("keeps production Rust plus its own test-only Rust changes in diff scope", () => {
  expect(
    classifyChanges([
      changed("crates/pure-analyzer-model/src/loader.rs"),
      changed("crates/pure-analyzer-model/tests/loader.rs"),
      changed("crates/pure-analyzer-model/src/tests.rs"),
    ]),
  ).toEqual({
    reason: "production-rust-and-tests",
    scope: "diff",
  });
});

// Same pre-deferral caveat as above.
test("classifies documentation-only changes as full before PR-lane deferral", () => {
  expect(
    classifyChanges([changed("README.md"), changed("docs/architecture/ci.md", "A")]),
  ).toEqual({ reason: "documentation-change", scope: "full" });
});

test("fails closed for test-only-with-no-production, configuration, FFI, renames, deletions, and an empty diff", () => {
  for (const changes of [
    [changed("crates/pure-analyzer-model/tests/loader.rs")],
    [
      changed(
        "crates/pure-analyzer-purecard/corpus/schemas/oos_ctx_dog_kennels.md",
      ),
    ],
    [changed("Cargo.toml")],
    [changed(FFI_SOURCE)],
    [{ kind: "R", paths: ["before.rs", "after.rs"], status: "R100" }],
    [{ kind: "D", paths: ["docs/old.md"], status: "D" }],
    [],
  ]) {
    expect(classifyChanges(changes).scope).toBe("full");
  }
});

test("classifies a test-only change set (no production Rust) as full", () => {
  expect(classifyChanges([changed("crates/pure-analyzer-model/tests/loader.rs")])).toEqual({
    reason: "test-only-change-set",
    scope: "full",
  });
});

test("keeps a mix of production Rust and one ineligible path on the full mutation floor", () => {
  expect(
    classifyChanges([
      changed("crates/pure-analyzer-model/src/loader.rs"),
      changed("crates/pure-analyzer-model/tests/loader.rs"),
      changed("Cargo.toml"),
    ]),
  ).toEqual({ reason: "non-production-or-configuration-change", scope: "full" });
});

test("keeps non-PR events on the full mutation floor", () => {
  expect(eventFallbackPlan("merge_group", "false")).toMatchObject({
    reason: "non-pull-request-event",
    scope: "full",
  });
  expect(eventFallbackPlan("push", "false")).toMatchObject({ scope: "full" });
  expect(eventFallbackPlan("pull_request", "true")).toMatchObject({ scope: "skip" });
  expect(eventFallbackPlan("pull_request", "false")).toBeUndefined();
});

test("bounds deferred direct-PR mutation plans and preserves a valid sentinel", () => {
  const full = eventFallbackPlan("merge_group", "false");
  const deferred = deferFullPlan(full, true);
  expect(deferred).toMatchObject({
    reason: "deferred-non-pull-request-event",
    scope: "skip",
  });
  expect(deferred.matrix).toEqual(mutationMatrix("skip", 1));

  // Every other deferrable full reason gets the same skip treatment.
  for (const reason of [
    "documentation-change",
    "empty-change-set",
    "inline-test-surface",
    "rename-delete-or-type-change",
    "test-only-change-set",
  ]) {
    expect(deferFullPlan({ ...full, reason }, true).scope).toBe("skip");
  }

  expect(prDiffShardCount(0)).toBe(0);
  expect(prDiffShardCount(PR_DIFF_MUTANTS_PER_SHARD)).toBe(1);
  expect(prDiffShardCount(PR_DIFF_MUTANTS_PER_SHARD + 1)).toBe(2);
  expect(prDiffShardCount(PR_DIFF_MUTANTS_PER_SHARD * PR_DIFF_MAX_SHARDS)).toBe(
    PR_DIFF_MAX_SHARDS,
  );
  expect(prDiffShardCount(PR_DIFF_MUTANTS_PER_SHARD * PR_DIFF_MAX_SHARDS + 1)).toBe(0);

  const bounded = deferFullPlan(
    planFromClassification(
      { reason: "production-rust-only", scope: "diff" },
      { diffSha256, headSha, mergeBase, mutantCount: PR_DIFF_MUTANTS_PER_SHARD + 1 },
    ),
    true,
  );
  expect(bounded.matrix).toEqual(mutationMatrix("diff", 2));

  const oversized = deferFullPlan(
    planFromClassification(
      { reason: "production-rust-only", scope: "diff" },
      {
        diffSha256,
        headSha,
        mergeBase,
        mutantCount: PR_DIFF_MUTANTS_PER_SHARD * PR_DIFF_MAX_SHARDS + 1,
      },
    ),
    true,
  );
  expect(oversized).toMatchObject({
    reason: "deferred-pr-diff-budget-exceeded",
    scope: "skip",
  });
  expect(oversized.matrix).toEqual(mutationMatrix("skip", 1));
  const zero = planFromClassification(
    { reason: "production-rust-only", scope: "diff" },
    { diffSha256, headSha, mergeBase, mutantCount: 0 },
  );
  expect(deferFullPlan(zero, true)).toMatchObject({
    reason: "deferred-zero-diff-mutant-list",
    scope: "skip",
  });
  const invalid = planFromClassification(
    { reason: "production-rust-only", scope: "diff" },
    { diffSha256, headSha, mergeBase, mutantCount: Number.NaN },
  );
  expect(() => deferFullPlan(invalid, true)).toThrow("not safely deferrable");
  expect(() =>
    deferFullPlan({ ...full, reason: "inline-test-inspection-failed" }, true),
  ).toThrow("not safely deferrable");
  expect(deferFullPlan(full)).toBe(full);
});

test("deferred CI planning emits sentinels but surfaces planner failures", async () => {
  const environment = {
    GITHUB_EVENT_NAME: "pull_request",
    MUTATION_BASE_SHA: mergeBase,
    MUTATION_DEFER_FULL: "true",
    MUTATION_HEAD_SHA: headSha,
    MUTATION_PR_DRAFT: "false",
    RUNNER_TEMP: "/tmp",
  };
  const dependencies = {
    checkedOutHead: async () => headSha,
    changedPaths: async () => [changed("Cargo.toml")],
    mergeBase: async () => mergeBase,
    repoRoot: async () => "/workspace",
  };

  await expect(
    planFromEnvironment(environment, {
      ...dependencies,
      classifyCheckedOutChanges: async () => ({
        reason: "non-production-or-configuration-change",
        scope: "full",
      }),
    }),
  ).resolves.toMatchObject({
    reason: "deferred-non-production-or-configuration-change",
    scope: "skip",
  });

  await expect(
    planFromEnvironment(environment, {
      ...dependencies,
      classifyCheckedOutChanges: async () => ({
        reason: "production-rust-only",
        scope: "diff",
      }),
      listedMutantCount: async () => PR_DIFF_MUTANTS_PER_SHARD * PR_DIFF_MAX_SHARDS + 1,
      writeDiff: async () => diffSha256,
    }),
  ).resolves.toMatchObject({ reason: "deferred-pr-diff-budget-exceeded", scope: "skip" });

  await expect(
    planFromEnvironment(environment, {
      repoRoot: async () => {
        throw new Error("planner unavailable");
      },
    }),
  ).rejects.toThrow("planner unavailable");

  await expect(
    planFromEnvironment({
      GITHUB_EVENT_NAME: "merge_group",
      MUTATION_DEFER_FULL: "true",
    }),
  ).resolves.toMatchObject({
    reason: "deferred-non-pull-request-event",
    scope: "skip",
  });
});

// This is the end-to-end regression test #310 asked for: it calls the same
// `planFromEnvironment` entry point CI's `mutation-plan` job actually
// invokes, with `MUTATION_DEFER_FULL=true` exactly as the PR lane sets it,
// over the realistic "production Rust + its own test file" diff shape that
// #276 showed was 100% of PR runs falling through to `skip`. Had this test
// existed, #276 could not have gone unnoticed for 60 consecutive PR runs.
test("end-to-end PR lane runs a bounded diff pass for a realistic production-plus-test diff, not a skip", async () => {
  const environment = {
    GITHUB_EVENT_NAME: "pull_request",
    MUTATION_BASE_SHA: mergeBase,
    MUTATION_DEFER_FULL: "true",
    MUTATION_HEAD_SHA: headSha,
    MUTATION_PR_DRAFT: "false",
    RUNNER_TEMP: "/tmp",
  };
  const changes = [
    changed("crates/pure-analyzer-model/src/loader.rs"),
    changed("crates/pure-analyzer-model/tests/loader.rs"),
  ];

  const plan = await planFromEnvironment(environment, {
    checkedOutHead: async () => headSha,
    changedPaths: async () => changes,
    // Exercises the REAL classifier (not a hand-built classification), so a
    // regression in `classifyChanges` itself would fail this test.
    classifyCheckedOutChanges: async (_root, _base, _head, changedPaths) =>
      classifyChanges(changedPaths),
    listedMutantCount: async () => PR_DIFF_MUTANTS_PER_SHARD,
    mergeBase: async () => mergeBase,
    repoRoot: async () => "/workspace",
    writeDiff: async () => diffSha256,
  });

  expect(plan.scope).toBe("diff");
  expect(plan.reason).toBe("production-rust-and-tests");
  expect(plan.matrix).toEqual(mutationMatrix("diff", 1));
  expect(plan.matrix.include[0].scope).toBe("diff");
});

test("never emits a changed path as an Actions output reason", () => {
  expect(
    classifyChanges([changed("crates/pure-analyzer-model/tests/new\noutput=value.rs")]),
  ).toEqual({
    reason: "non-production-or-configuration-change",
    scope: "full",
  });
});

test("sizes incremental shards from the listed mutant count and caps at full width", () => {
  expect(diffShardCount(0)).toBe(0);
  expect(diffShardCount(1)).toBe(1);
  expect(diffShardCount(MUTANTS_PER_DIFF_SHARD)).toBe(1);
  expect(diffShardCount(MUTANTS_PER_DIFF_SHARD + 1)).toBe(2);
  expect(diffShardCount(Number.MAX_SAFE_INTEGER)).toBe(FULL_MUTATION_SHARDS);
});

// Asserts the command's observable CONTRACT (workspace scope, FFI exclusion,
// list-only, diff-path plumbing) instead of reconstructing its literal
// argv — a prior version of this test just restated `mutationListCommand`'s
// body element-for-element, so it could never fail on a real regression
// (#310).
test("lists the same workspace scope as the mutation runner without unsupported flags", () => {
  const diffPath = "target/mutation-scope.diff";
  const command = mutationListCommand(diffPath);

  expect(command.slice(0, 2)).toEqual(["cargo", "mutants"]);
  expect(command).toContain("--workspace");
  expect(command).toContain("--in-place");

  const exclude = command.indexOf("--exclude");
  expect(exclude).toBeGreaterThan(-1);
  expect(command[exclude + 1]).toBe(FFI_SOURCE);

  const timeout = command.indexOf("--timeout");
  expect(timeout).toBeGreaterThan(-1);
  expect(command[timeout + 1]).toBe(MUTATION_COMMAND_TIMEOUT_SECONDS);

  const inDiff = command.indexOf("--in-diff");
  expect(inDiff).toBeGreaterThan(-1);
  expect(command[inDiff + 1]).toBe(diffPath);

  // A listing command must never itself apply or check out a mutant.
  expect(command.slice(-2)).toEqual(["--list", "--json"]);
  expect(command).not.toContain("--check");
  expect(command).not.toContain("--baseline");

  // The diff path is the only thing that varies with its argument.
  const otherCommand = mutationListCommand("target/other.diff");
  expect(otherCommand[inDiff + 1]).toBe("target/other.diff");
  const withoutDiffArg = (cmd) => cmd.filter((_, i) => i !== inDiff + 1);
  expect(withoutDiffArg(otherCommand)).toEqual(withoutDiffArg(command));
});

// Behavioral replacement for a prior version of this test that only
// re-asserted `PLANNER_COMMAND_TIMEOUT_MS`/`PLANNER_COMMAND_MAX_BUFFER_BYTES`
// equal their own literal definitions (a change-detector that could never
// fail on a real defect — #310). This instead proves `run()` actually
// ENFORCES its default bound when no caller override is given, by sizing a
// real subprocess's output off `mutationListRunOptions()` and observing it
// get rejected.
test("run() enforces the planner's own default output-size bound with no override", async () => {
  const { maxBuffer } = mutationListRunOptions();
  await expect(
    run(
      [process.execPath, "-e", `process.stdout.write("x".repeat(${maxBuffer + 1}));`],
      process.cwd(),
    ),
  ).rejects.toThrow("output exceeded the configured limit");
});

test("fails closed when bounded planner output exceeds its cap", async () => {
  const maxBuffer = 64 * 1024;
  await expect(
    run(
      [
        process.execPath,
        "-e",
        `process.stdout.write("x".repeat(${3 * 1024 * 1024}));`,
      ],
      process.cwd(),
      { maxBuffer },
    ),
  ).rejects.toThrow("output exceeded the configured limit");
});

test("writes the diff atomically and discards a capped stream", async () => {
  const directory = await mkdtemp(join(tmpdir(), "mutation-scope-"));
  const outputPath = join(directory, "scope.diff");
  const mergeBase = "a".repeat(40);
  const headSha = "b".repeat(40);
  let command;
  try {
    await writeFile(outputPath, "stale diff");
    await expect(
      writeDiff(
        "/workspace",
        mergeBase,
        headSha,
        outputPath,
        async (nextCommand, options) => {
          command = nextCommand;
          expect(options.maxBuffer).toBe(PLANNER_COMMAND_MAX_BUFFER_BYTES);
          await options.onStdout(new Uint8Array(64 * 1024));
          return {
            code: 137,
            outputLimitExceeded: true,
            stderr: "",
            stdout: "",
          };
        },
      ),
    ).rejects.toThrow("generated mutation diff exceeded the configured output limit");
    expect(command).toEqual([
      "git",
      "diff",
      "--no-ext-diff",
      "--no-renames",
      "--no-textconv",
      "--unified=0",
      mergeBase,
      headSha,
      "--",
    ]);
    await expect(readFile(outputPath, "utf8")).resolves.toBe("stale diff");
    expect(await readdir(directory)).toEqual(["scope.diff"]);
  } finally {
    await rm(directory, { force: true, recursive: true });
  }
});

test("requires event-pinned head metadata and a nonempty incremental list", () => {
  const classification = { reason: "production-rust-only", scope: "diff" };
  expect(planFromClassification(classification, { mutantCount: 0 })).toMatchObject({
    reason: "zero-diff-mutant-list",
    scope: "full",
  });
  expect(planFromClassification(classification, { mutantCount: Number.NaN })).toMatchObject({
    reason: "invalid-diff-mutant-list",
    scope: "full",
  });
  expect(
    planFromClassification(classification, {
      diffSha256,
      mergeBase,
      mutantCount: 1,
    }),
  ).toMatchObject({
    reason: "invalid-diff-metadata",
    scope: "full",
  });
  expect(
    planFromClassification(classification, {
      diffSha256,
      headSha,
      mergeBase,
      mutantCount: 151,
    }),
  ).toMatchObject({
    diffSha256,
    headSha,
    mergeBase,
    mutantCount: 151,
    scope: "diff",
  });
  expect(
    planFromClassification(classification, {
      diffSha256,
      headSha,
      mergeBase,
      mutantCount: 151,
    }).matrix,
  ).toEqual(mutationMatrix("diff", 3));
});

test("uses a sentinel matrix for skips and retains every full shard for fallbacks", () => {
  expect(mutationMatrix("skip", 0)).toEqual({
    include: [
      {
        diagnostics: "skip",
        index: 0,
        report: "skip",
        scope: "skip",
        total: 1,
      },
    ],
  });
  expect(mutationMatrix("full", FULL_MUTATION_SHARDS).include).toHaveLength(
    FULL_MUTATION_SHARDS,
  );
});
