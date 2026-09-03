import { describe, expect, test } from "bun:test";
import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { assertTablesUsable, isBotAuthored, labelsForPullRequest } from "./pr-label.mjs";

const CLI = join(import.meta.dir, "pr-label.mjs");

function runCLI(env, args = []) {
  return Bun.spawnSync([process.execPath, CLI, ...args], { env: { ...process.env, ...env } });
}

function fixtureFile(name, prs) {
  const dir = mkdtempSync(join(tmpdir(), "pr-label-"));
  const file = join(dir, name);
  writeFileSync(file, JSON.stringify(prs));
  return file;
}

const out = (proc) => proc.stdout.toString() + proc.stderr.toString();

describe("isBotAuthored", () => {
  test("recognizes a login ending [bot]", () => {
    expect(isBotAuthored({ user: { login: "dependabot[bot]" } })).toBe(true);
    expect(isBotAuthored({ user: { login: "renovate[bot]" } })).toBe(true);
  });

  test("does not flag a human login, or a missing user", () => {
    expect(isBotAuthored({ user: { login: "tsouza" } })).toBe(false);
    expect(isBotAuthored({})).toBe(false);
    expect(isBotAuthored(undefined)).toBe(false);
  });
});

describe("labelsForPullRequest", () => {
  test("skips a bot-authored PR entirely, regardless of its title", () => {
    const d = labelsForPullRequest({ number: 8, title: "ci(deps): bump CodSpeedHQ/action from 4 to 5", user: { login: "dependabot[bot]" }, labels: [] });
    expect(d.botSkipped).toBe(true);
    expect(d.missing).toEqual([]);
  });

  test("proposes the CC-derived label for a human-authored PR", () => {
    const d = labelsForPullRequest({ number: 364, title: "fix(purecard)!: narrow arm-R colName positions", user: { login: "tsouza" }, labels: [] });
    expect(d.want).toEqual(["bug"]);
    expect(d.missing).toEqual(["bug"]);
  });

  test("is IDEMPOTENT — a PR that already carries its label proposes nothing", () => {
    const d = labelsForPullRequest({
      number: 364,
      title: "fix(purecard)!: narrow arm-R colName positions",
      user: { login: "tsouza" },
      labels: [{ name: "bug" }],
    });
    expect(d.missing).toEqual([]);
  });

  test("a title with no Conventional-Commit prefix proposes nothing", () => {
    const d = labelsForPullRequest({ number: 1, title: "Merge branch main", user: { login: "tsouza" }, labels: [] });
    expect(d.want).toEqual([]);
    expect(d.missing).toEqual([]);
  });

  test("the *(deps) scope override reaches a human-authored PR too", () => {
    const d = labelsForPullRequest({ number: 2, title: "fix(deps): pin a transitive dependency", user: { login: "tsouza" }, labels: [] });
    expect(d.missing).toEqual(["dependencies"]);
  });
});

describe("assertTablesUsable", () => {
  test("passes on the live shared type table", () => {
    expect(assertTablesUsable()).toEqual([]);
  });
});

const DRY = { PR_LABEL_MODE: "backfill", PR_LABEL_DRY_RUN: "1" };

describe("CLI", () => {
  test("dry-run exits 0 and reports a per-PR line for a real fixture", () => {
    const file = fixtureFile("ok.json", [
      { number: 364, title: "fix(purecard)!: narrow arm-R colName positions", user: { login: "tsouza" }, labels: [] },
    ]);
    const proc = runCLI({ ...DRY, PR_LABEL_FIXTURE: file });
    expect(proc.exitCode).toBe(0);
    expect(out(proc)).toMatch(/PR #364 DRY-RUN would add \[bug\]/);
    expect(out(proc)).toMatch(/1 PR\(s\) scanned, 1 PR\(s\) would be labeled/);
  });

  test("skips a bot-authored PR and reports it, without failing the run", () => {
    const file = fixtureFile("bot.json", [
      { number: 8, title: "ci(deps): bump CodSpeedHQ/action from 4 to 5", user: { login: "dependabot[bot]" }, labels: [] },
    ]);
    const proc = runCLI({ ...DRY, PR_LABEL_FIXTURE: file });
    expect(proc.exitCode).toBe(0);
    expect(out(proc)).toMatch(/bot-authored \(dependabot\[bot\]\)/);
    expect(out(proc)).toMatch(/1 bot-skipped/);
  });

  test("reports zero pending labels for an already-labeled fixture (idempotent)", () => {
    const file = fixtureFile("done.json", [
      { number: 364, title: "fix(purecard)!: narrow arm-R colName positions", user: { login: "tsouza" }, labels: [{ name: "bug" }] },
    ]);
    const proc = runCLI({ ...DRY, PR_LABEL_FIXTURE: file });
    expect(proc.exitCode).toBe(0);
    expect(out(proc)).toMatch(/already carries: bug/);
  });

  test("FAILS on a vacuous run — zero PRs processed", () => {
    const file = fixtureFile("empty.json", []);
    const proc = runCLI({ ...DRY, PR_LABEL_FIXTURE: file });
    expect(proc.exitCode).toBe(1);
    expect(out(proc)).toMatch(/processed ZERO pull requests/);
  });

  test("rejects an unknown mode and a fixture outside dry-run", () => {
    const file = fixtureFile("ok2.json", [{ number: 1, title: "fix: x", user: { login: "tsouza" }, labels: [] }]);
    const bad = runCLI({ PR_LABEL_MODE: "sideways", PR_LABEL_FIXTURE: file, PR_LABEL_DRY_RUN: "1" });
    expect(bad.exitCode).toBe(1);
    expect(out(bad)).toMatch(/PR_LABEL_MODE must be/);

    const wet = runCLI({ PR_LABEL_MODE: "backfill", PR_LABEL_FIXTURE: file, GITHUB_REPOSITORY: "o/r", GITHUB_TOKEN: "t" });
    expect(wet.exitCode).toBe(1);
    expect(out(wet)).toMatch(/dry-run-only input/);
  });

  test("--check-tables proves the mapping table is usable", () => {
    const proc = runCLI({}, ["--check-tables"]);
    expect(proc.exitCode).toBe(0);
    expect(out(proc)).toMatch(/mapping table usable/);
  });
});
