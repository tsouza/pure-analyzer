import { readFileSync } from "node:fs";

import { expect, test } from "bun:test";

import {
  CI_WORKFLOW,
  GENERATED_ROOT_DECLARATIONS,
  codeFilterGlobs,
  declaredPath,
  isGated,
  ungatedRoots,
} from "./generated-paths-gated.mjs";

test("reads a generated root from its Rust declaration", () => {
  expect(
    declaredPath('const EXPLAIN_DIRECTORY: &str = "docs/explain";', "EXPLAIN_DIRECTORY"),
  ).toBe("docs/explain");
  expect(declaredPath("let x = 1;", "EXPLAIN_DIRECTORY")).toBeUndefined();
});

test("a prefix glob gates the directories beneath it", () => {
  expect(isGated(["docs/**"], "docs/explain")).toBe(true);
  expect(isGated(["docs/explain/**"], "docs/explain")).toBe(true);
  expect(isGated(["**/*.rs", "crates/**"], "docs/explain")).toBe(false);
  expect(isGated(["doc/**"], "docs/explain")).toBe(false);
  expect(isGated(["docs/explain/**"], "docs")).toBe(false);
});

test("reports a generated root the code filter leaves ungated", () => {
  const globs = ["**/*.rs", "crates/**", "xtask/**"];
  expect(ungatedRoots(globs, ["docs/explain"])).toEqual(["docs/explain"]);
  expect(ungatedRoots([...globs, "docs/**"], ["docs/explain"])).toEqual([]);
});

test("the committed workflow gates every committed generated root", () => {
  const globs = codeFilterGlobs(readFileSync(CI_WORKFLOW, "utf8"));
  expect(globs).toContain("crates/**");
  const roots = GENERATED_ROOT_DECLARATIONS.map(({ source, constant }) =>
    declaredPath(readFileSync(source, "utf8"), constant),
  );
  expect(roots).toEqual(["docs/explain"]);
  expect(ungatedRoots(globs, roots)).toEqual([]);
});
