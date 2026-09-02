#!/usr/bin/env bun
// Reject a repository gate that only a local git hook can run.
//
// Constitution §2 (PROTECTED) requires every invariant to be checked pre-merge
// and reproduced in CI: a check wired into `lefthook.yml` alone is skipped by
// `--no-verify`, by a clone that never ran `just hooks-install`, and by any web
// or agent commit — with nothing to catch the violation before merge. This gate
// asserts every `scripts/checks/*.mjs` gate is reachable from a GitHub Actions
// workflow, directly or through a `just` recipe (its own body or a
// prerequisite's).
//
// It validates repository wiring rather than file content, so there is no
// staged-scope variant and no hook entry: the workflows and the justfile are
// the whole input.
import { $ } from "bun";

import { die } from "../lib/ci.mjs";

/** Repository-relative directory holding the gate scripts. */
export const CHECKS_DIR = "scripts/checks/";
/** Repository-relative directory holding the GitHub Actions workflows. */
export const WORKFLOWS_DIR = ".github/workflows/";
/** Any reference to a gate script, in a justfile recipe or a workflow step. */
const SCRIPT_REFERENCE = /scripts\/checks\/[A-Za-z0-9_.-]+\.mjs/g;
// A justfile recipe header: `<name> <params...>: <prerequisites...>`. `:=` is an
// assignment (`set shell := [...]`), never a recipe, hence the lookahead.
const RECIPE_HEADER = /^([A-Za-z0-9_-]+)([^:=]*):(?!=)(.*)$/;

/** Keep the gate scripts: `scripts/checks/*.mjs`, excluding their unit tests. */
export function gateScripts(paths) {
  return paths.filter(
    (path) =>
      path.startsWith(CHECKS_DIR) &&
      path.endsWith(".mjs") &&
      !path.endsWith(".test.mjs"),
  );
}

/**
 * Parse a justfile into `name -> { prerequisites, body }`.
 * @param {string} text
 * @returns {Map<string, {prerequisites: string[], body: string}>}
 */
export function parseRecipes(text) {
  const recipes = new Map();
  let current = null;
  for (const line of text.split("\n")) {
    if (/^\s/.test(line)) {
      if (current) recipes.get(current).body += `${line}\n`;
      continue;
    }
    const header = line.startsWith("#") ? null : line.match(RECIPE_HEADER);
    if (!header) {
      current = null;
      continue;
    }
    current = header[1];
    recipes.set(current, {
      prerequisites: header[3].trim().split(/\s+/).filter(Boolean),
      body: "",
    });
  }
  return recipes;
}

/**
 * Every gate script a `just <name>` invocation ends up running, following
 * prerequisites.
 * @returns {Set<string>}
 */
export function scriptsRunBy(recipes, name, seen = new Set()) {
  const found = new Set();
  if (seen.has(name)) return found;
  seen.add(name);
  const recipe = recipes.get(name);
  if (!recipe) return found;
  for (const reference of recipe.body.matchAll(SCRIPT_REFERENCE)) {
    found.add(reference[0]);
  }
  for (const prerequisite of recipe.prerequisites) {
    for (const script of scriptsRunBy(recipes, prerequisite, seen)) {
      found.add(script);
    }
  }
  return found;
}

/**
 * Every gate script the workflows run: referenced by path, or reached through a
 * `just <recipe>` step.
 * @returns {Set<string>}
 */
export function scriptsRunInCi(justfileText, workflowText) {
  const reachable = new Set(
    [...workflowText.matchAll(SCRIPT_REFERENCE)].map((match) => match[0]),
  );
  const recipes = parseRecipes(justfileText);
  for (const name of recipes.keys()) {
    const invocation = new RegExp(`(?:^|\\s)just\\s+${name}(?:\\s|$)`, "m");
    if (!invocation.test(workflowText)) continue;
    for (const script of scriptsRunBy(recipes, name)) reachable.add(script);
  }
  return reachable;
}

/** The gate scripts no workflow can reach. */
export function hookOnlyGates(scripts, reachable) {
  return scripts.filter((script) => !reachable.has(script));
}

async function trackedFiles(directory) {
  const out = await $`git ls-files -- ${directory}`.text();
  return out.split("\n").filter(Boolean);
}

async function concatenated(paths) {
  const texts = await Promise.all(paths.map((path) => Bun.file(path).text()));
  return texts.join("\n");
}

if (import.meta.main) {
  const scripts = gateScripts(await trackedFiles(CHECKS_DIR));
  const workflows = (await trackedFiles(WORKFLOWS_DIR)).filter((path) =>
    /\.ya?ml$/.test(path),
  );
  const reachable = scriptsRunInCi(
    await Bun.file("justfile").text(),
    await concatenated(workflows),
  );
  const unwired = hookOnlyGates(scripts, reachable);
  if (unwired.length) {
    die(
      `gate scripts no GitHub Actions workflow runs — a local hook is not a gate ` +
        `(constitution §2: gates run pre-merge and reproduce in CI). Add a step ` +
        `running the script, or a \`just\` recipe that does:\n${unwired
          .map((script) => `    ${script}`)
          .join("\n")}`,
    );
  }
}
