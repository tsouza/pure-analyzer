#!/usr/bin/env bun
// Keep change planning and progress in GitHub Issues/PRs, not in a checked-in
// duplicate work ledger. Durable product and architecture references
// remain valid source material; this gate rejects retired per-change ledgers.
import { $ } from "bun";
import { die } from "../lib/ci.mjs";

/** Paths formerly used for checked-in work state rather than product truth. */
export const WORK_LEDGER_PATHS = [
  "specs",
  "docs/design/pure-analyzer-design.md",
  "docs/lessons.md",
  "crates/pure-analyzer-purecard/docs/lessons.md",
  ".claude/commands/spec.md",
  ".claude/skills/spec",
  "docs/methodology/spec-driven.md",
  "crates/pure-analyzer-purecard/docs/methodology/spec-driven.md",
];

/** Return tracked paths that would reintroduce the retired work ledger. */
export function workLedgerPaths(paths) {
  return paths.filter((path) =>
    WORK_LEDGER_PATHS.some((ledgerPath) =>
      path === ledgerPath || path.startsWith(`${ledgerPath}/`),
    ),
  );
}

async function indexedPaths() {
  const out = await $`git ls-files -z`.text();
  return out.split("\0").filter(Boolean);
}

if (import.meta.main) {
  const paths = workLedgerPaths(await indexedPaths());
  if (paths.length > 0) {
    die(
      `checked-in work ledger paths are forbidden; keep change state in GitHub Issues/PRs:\n${paths
        .map((path) => `    ${path}`)
        .join("\n")}`,
    );
  }
}
