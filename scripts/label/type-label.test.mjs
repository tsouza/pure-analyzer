import { describe, expect, test } from "bun:test";

import { TYPE_TO_LABEL, labelsForTitle } from "./type-label.mjs";

describe("labelsForTitle", () => {
  test("maps every bare Conventional-Commit type to its label", () => {
    expect(labelsForTitle("feat: add a walk recipe")).toEqual(["enhancement"]);
    expect(labelsForTitle("fix: cursor overflow")).toEqual(["bug"]);
    expect(labelsForTitle("docs: tidy README")).toEqual(["documentation"]);
    expect(labelsForTitle("ci: pin actionlint")).toEqual(["ci"]);
    expect(labelsForTitle("test: add fixture")).toEqual(["test"]);
    expect(labelsForTitle("refactor: extract emitter")).toEqual(["refactor"]);
    expect(labelsForTitle("perf: prune scan")).toEqual(["performance"]);
    expect(labelsForTitle("chore: tidy")).toEqual(["chore"]);
    expect(labelsForTitle("build: bump goreleaser")).toEqual(["build"]);
    expect(labelsForTitle("revert: undo #123")).toEqual(["revert"]);
  });

  test("style is cosmetic-only and carries no label", () => {
    expect(labelsForTitle("style: gofmt")).toEqual([]);
  });

  test("any *(deps) scope overrides the bare type to dependencies", () => {
    expect(labelsForTitle("chore(deps): bump x from 1 to 2")).toEqual(["dependencies"]);
    expect(labelsForTitle("ci(deps): bump action")).toEqual(["dependencies"]);
    expect(labelsForTitle("fix(deps): pin transitive")).toEqual(["dependencies"]);
    expect(labelsForTitle("build(deps): bump toolchain")).toEqual(["dependencies"]);
  });

  test("a non-deps scope falls through to the bare type, including (release)", () => {
    // This repo's release-plz PRs are titled plain `chore: release vX.Y.Z`
    // (see PR #342) with no scope, so `chore(release)` has no special
    // override here (unlike cerberus, whose release-plz config emits that
    // scope) — it is just a chore(<other>) falling through to `chore`.
    expect(labelsForTitle("chore(release): v1.2.3")).toEqual(["chore"]);
    expect(labelsForTitle("chore: release v1.2.3")).toEqual(["chore"]);
    expect(labelsForTitle("chore(ci): tidy")).toEqual(["chore"]);
  });

  test("the breaking-change marker and a scope do not break parsing", () => {
    expect(labelsForTitle("feat!: breaking")).toEqual(["enhancement"]);
    expect(labelsForTitle("feat(purecard)!: breaking scoped")).toEqual(["enhancement"]);
  });

  test("the type token is case-insensitive", () => {
    expect(labelsForTitle("FIX: shouty")).toEqual(["bug"]);
  });

  test("yields nothing for a title without a Conventional-Commit header", () => {
    expect(labelsForTitle("")).toEqual([]);
    expect(labelsForTitle("no conventional prefix here")).toEqual([]);
    expect(labelsForTitle("wibble: unknown type")).toEqual([]);
    expect(labelsForTitle("Merge branch main")).toEqual([]);
    expect(labelsForTitle(null)).toEqual([]);
    expect(labelsForTitle(undefined)).toEqual([]);
  });

  test("TYPE_TO_LABEL is non-empty (anti-vacuity: the mapping table itself must never be emptied)", () => {
    expect(Object.keys(TYPE_TO_LABEL).length).toBeGreaterThan(0);
  });
});

describe("CLI --self-test", () => {
  test("exits 0 and prints its own assertion pass", () => {
    const proc = Bun.spawnSync([process.execPath, `${import.meta.dir}/type-label.mjs`, "--self-test"]);
    expect(proc.exitCode).toBe(0);
    expect(proc.stdout.toString()).toMatch(/all assertions passed/);
  });
});
