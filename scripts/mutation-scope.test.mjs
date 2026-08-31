import { expect, test } from "bun:test";

import {
  FFI_SOURCE,
  FULL_MUTATION_SHARDS,
  MUTATION_COMMAND_TIMEOUT_SECONDS,
  MUTATION_LIST_MAX_BUFFER_BYTES,
  MUTATION_LIST_TIMEOUT_MS,
  MUTANTS_PER_DIFF_SHARD,
  classifyCheckedOutChanges,
  classifyChanges,
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
  planFromClassification,
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

test("uses diff scope for production Rust plus harmless documentation", () => {
  expect(
    classifyChanges([
      changed("crates/pure-analyzer-model/src/loader.rs"),
      changed("docs/architecture/ci.md"),
    ]),
  ).toEqual({ reason: "production-rust-only", scope: "diff" });
});

test("fails closed when production and test Rust paths mix", () => {
  expect(
    classifyChanges([
      changed("crates/pure-analyzer-model/src/loader.rs"),
      changed("crates/pure-analyzer-model/tests/loader.rs"),
      changed("crates/pure-analyzer-model/src/tests.rs"),
    ]),
  ).toEqual({
    reason: "non-production-or-configuration-change",
    scope: "full",
  });
});

test("skips documentation-only changes", () => {
  expect(
    classifyChanges([changed("README.md"), changed("docs/architecture/ci.md", "A")]),
  ).toEqual({ reason: "documentation-only", scope: "skip" });
});

test("fails closed for test-only, configuration, FFI, renames, deletions, and an empty diff", () => {
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

test("keeps non-PR events on the full mutation floor", () => {
  expect(eventFallbackPlan("merge_group", "false")).toMatchObject({
    reason: "non-pull-request-event",
    scope: "full",
  });
  expect(eventFallbackPlan("push", "false")).toMatchObject({ scope: "full" });
  expect(eventFallbackPlan("pull_request", "true")).toMatchObject({ scope: "skip" });
  expect(eventFallbackPlan("pull_request", "false")).toBeUndefined();
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

test("lists the same workspace scope as the mutation runner without unsupported flags", () => {
  expect(mutationListCommand("target/mutation-scope.diff")).toEqual([
    "cargo",
    "mutants",
    "--workspace",
    "--exclude",
    FFI_SOURCE,
    "--in-place",
    "--timeout",
    MUTATION_COMMAND_TIMEOUT_SECONDS,
    "--in-diff",
    "target/mutation-scope.diff",
    "--list",
    "--json",
  ]);
});

test("bounds the planner mutant list subprocess", () => {
  expect(mutationListRunOptions()).toEqual({
    maxBuffer: MUTATION_LIST_MAX_BUFFER_BYTES,
    timeoutMs: MUTATION_LIST_TIMEOUT_MS,
  });
  expect(MUTATION_LIST_TIMEOUT_MS).toBe(2 * 60 * 1_000);
  expect(MUTATION_LIST_MAX_BUFFER_BYTES).toBe(8 * 1024 * 1024);
});

test("requires event-pinned head metadata and a nonempty incremental list", () => {
  const classification = { reason: "production-rust-only", scope: "diff" };
  expect(planFromClassification(classification, { mutantCount: 0 }).scope).toBe("full");
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
