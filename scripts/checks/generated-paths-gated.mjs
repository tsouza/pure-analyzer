#!/usr/bin/env bun
// A gate that regenerates a tracked directory only protects it while the
// workflow filter that runs the gate also matches that directory. `xtask ci`
// regenerates the explain reference pages and fails on drift, but it runs
// behind ci.yml's `code` paths filter; when the filter omitted `docs/**`, a
// docs-only edit skipped the gate entirely and could desync a generated page
// from its catalog. This derives the generated roots from their Rust
// declarations rather than restating them, so adding a generated directory
// without gating it fails here.
import { readFileSync } from "node:fs";

import { die } from "../lib/ci.mjs";

/** Rust sources declaring a directory that `xtask ci` regenerates and diffs. */
export const GENERATED_ROOT_DECLARATIONS = [
  { source: "xtask/src/explain_docs.rs", constant: "EXPLAIN_DIRECTORY" },
];

/** The workflow whose `code` filter decides whether `xtask ci` runs. */
export const CI_WORKFLOW = ".github/workflows/ci.yml";

/** Extract a `const NAME: &str = "value";` path from Rust source text. */
export function declaredPath(text, constant) {
  const match = text.match(
    new RegExp(`const\\s+${constant}\\s*:\\s*&str\\s*=\\s*"([^"]+)"`),
  );
  return match?.[1];
}

/** Extract the `code:` filter globs from the paths-filter step in ci.yml. */
export function codeFilterGlobs(text) {
  const lines = text.split("\n");
  const start = lines.findIndex((line) => line === " ".repeat(12) + "code:");
  if (start < 0) return [];
  const globs = [];
  for (const line of lines.slice(start + 1)) {
    if (line.trim() === "" || line.startsWith(" ".repeat(14) + "#")) continue;
    const entry = line.match(/^ {14}- '(.*)'$/);
    if (!entry) break;
    globs.push(entry[1]);
  }
  return globs;
}

/** Whether any glob matches every file under `directory`. */
export function isGated(globs, directory) {
  const segments = directory.split("/");
  return globs.some((glob) => {
    if (glob === "**" || glob === "**/*") return true;
    const prefix = glob.replace(/\/\*\*(\/\*)?$/, "");
    if (prefix === glob) return false;
    const parts = prefix.split("/");
    return (
      parts.length <= segments.length &&
      parts.every((part, index) => part === segments[index])
    );
  });
}

/** Generated roots that the `code` filter fails to cover. */
export function ungatedRoots(globs, roots) {
  return roots.filter((root) => !isGated(globs, root));
}

if (import.meta.main) {
  const roots = GENERATED_ROOT_DECLARATIONS.map(({ source, constant }) => {
    const path = declaredPath(readFileSync(source, "utf8"), constant);
    if (!path) {
      die(`could not read ${constant} from ${source}`);
    }
    return path;
  });
  const globs = codeFilterGlobs(readFileSync(CI_WORKFLOW, "utf8"));
  if (globs.length === 0) {
    die(`could not read the \`code\` paths filter from ${CI_WORKFLOW}`);
  }
  const ungated = ungatedRoots(globs, roots);
  if (ungated.length > 0) {
    die(
      `${CI_WORKFLOW} \`code\` filter does not match directories that \`xtask ci\` regenerates, so a change to them would skip the gate that owns them:\n${ungated
        .map((root) => `    ${root}`)
        .join("\n")}`,
    );
  }
}
