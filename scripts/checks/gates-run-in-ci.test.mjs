// Unit tests for the pure exports of the CI-reachability gate.
import { expect, test } from "bun:test";
import {
  gateScripts,
  hookOnlyGates,
  parseRecipes,
  scriptsRunBy,
  scriptsRunInCi,
} from "./gates-run-in-ci.mjs";

const JUSTFILE = [
  "set shell := [\"bash\", \"-euo\", \"pipefail\", \"-c\"]",
  "",
  "# A comment line, not a recipe.",
  "postponed-markers:",
  "    bun scripts/checks/postponed-markers.mjs --all",
  "",
  "generated-paths-gated:",
  "    bun scripts/checks/generated-paths-gated.mjs",
  "",
  "fuzz target seconds:",
  "    cargo fuzz run {{ target }}",
  "",
  "ci: generated-paths-gated",
  "    cargo xtask ci",
  "",
].join("\n");

test("parses recipe names, prerequisites, and bodies", () => {
  const recipes = parseRecipes(JUSTFILE);
  expect([...recipes.keys()]).toEqual([
    "postponed-markers",
    "generated-paths-gated",
    "fuzz",
    "ci",
  ]);
  expect(recipes.get("ci").prerequisites).toEqual(["generated-paths-gated"]);
  expect(recipes.get("postponed-markers").prerequisites).toEqual([]);
});

test("a `:=` assignment is not a recipe", () => {
  expect(parseRecipes(JUSTFILE).has("set")).toBe(false);
});

test("a recipe runs the scripts in its own body", () => {
  const recipes = parseRecipes(JUSTFILE);
  expect([...scriptsRunBy(recipes, "postponed-markers")]).toEqual([
    "scripts/checks/postponed-markers.mjs",
  ]);
});

test("a recipe also runs its prerequisites' scripts", () => {
  const recipes = parseRecipes(JUSTFILE);
  expect([...scriptsRunBy(recipes, "ci")]).toEqual([
    "scripts/checks/generated-paths-gated.mjs",
  ]);
});

test("an unknown recipe runs nothing", () => {
  expect(scriptsRunBy(parseRecipes(JUSTFILE), "absent").size).toBe(0);
});

test("a prerequisite cycle terminates", () => {
  const recipes = parseRecipes(
    ["a: b", "    bun scripts/checks/a.mjs", "b: a", "    echo b", ""].join(
      "\n",
    ),
  );
  expect([...scriptsRunBy(recipes, "a")]).toEqual(["scripts/checks/a.mjs"]);
});

test("a workflow step invoking the script directly makes it reachable", () => {
  const workflow = "      - run: bun scripts/checks/postponed-markers.mjs\n";
  expect(scriptsRunInCi(JUSTFILE, workflow)).toContain(
    "scripts/checks/postponed-markers.mjs",
  );
});

test("a workflow step invoking a `just` recipe makes its scripts reachable", () => {
  const workflow = "        run: just ci\n";
  expect(scriptsRunInCi(JUSTFILE, workflow)).toContain(
    "scripts/checks/generated-paths-gated.mjs",
  );
});

test("a `just` recipe no workflow invokes leaves its script unreachable", () => {
  const reachable = scriptsRunInCi(JUSTFILE, "        run: just ci\n");
  expect(reachable).not.toContain("scripts/checks/postponed-markers.mjs");
});

test("a longer recipe name is not matched by a prefix invocation", () => {
  const justfile = [
    "lint:",
    "    cargo clippy",
    "lint-stale:",
    "    bun scripts/checks/stale.mjs",
    "",
  ].join("\n");
  expect(scriptsRunInCi(justfile, "        run: just lint\n")).not.toContain(
    "scripts/checks/stale.mjs",
  );
});

test("hook-only gates are exactly the unreachable scripts", () => {
  const scripts = ["scripts/checks/a.mjs", "scripts/checks/b.mjs"];
  const reachable = new Set(["scripts/checks/a.mjs"]);
  expect(hookOnlyGates(scripts, reachable)).toEqual(["scripts/checks/b.mjs"]);
});

test("gate scripts exclude unit tests and non-check paths", () => {
  expect(
    gateScripts([
      "scripts/checks/a.mjs",
      "scripts/checks/a.test.mjs",
      "scripts/checks/README.md",
      "scripts/lib/ci.mjs",
    ]),
  ).toEqual(["scripts/checks/a.mjs"]);
});
