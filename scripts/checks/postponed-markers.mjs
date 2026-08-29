#!/usr/bin/env bun
// Reject markers that must never reach a tracked Rust source: postponed work
// (TODO/FIXME/XXX/#[ignore]) and a leaked cargo-mutants edit.
// The ast-grep rule already bans todo!()/unimplemented!()/unreachable!() macros;
// this covers the comment markers those can't see.
//
// The mutation marker is here because `cargo mutants --in-place` rewrites the
// real source tree: run against a worktree that is still being edited it
// interleaves with those edits, and a mutant — a silently *weakened* gate — can
// ride into a commit. Observed doing exactly that (issue #55, Phase 5), where
// only an unrelated fixture that happened to cover the mutated function caught
// it; a mutant on a less-covered line would have merged. cargo-mutants stamps
// every edit it makes, so this is an exact signal, not a heuristic.
//   default : scan added lines of STAGED *.rs (git pre-commit, via lefthook)
//   --all   : scan all tracked Rust sources (CI structural gate)
import { $ } from "bun";
import { stagedFiles, stagedAddedLines } from "../lib/git.mjs";
import { die } from "../lib/ci.mjs";

// One source of truth for both scan modes: the staged-line filter and the
// `git grep` both read this, so they can never drift over which markers are
// banned (they already had.)
const MARKERS = String.raw`\b(TODO|FIXME|XXX)\b|#\[ignore\]|changed by cargo-mutants`;
const PATTERN = new RegExp(MARKERS);
const RUST_PATHSPECS = ["crates/**/*.rs", "xtask/**/*.rs", "fuzz/**/*.rs"];

async function hits() {
  if (process.argv.includes("--all")) {
    const out = await $`git grep -nE ${MARKERS} -- ${RUST_PATHSPECS}`
      .nothrow()
      .text();
    return out.split("\n").filter(Boolean);
  }
  const files = await stagedFiles({ suffix: ".rs" });
  if (files.length === 0) return [];
  return (await stagedAddedLines(files)).filter((l) => PATTERN.test(l));
}

const found = await hits();
if (found.length) {
  die(
    `postponed-work or cargo-mutants markers found — resolve, file an issue, or ` +
      `restore the mutated source:\n${found.map((h) => `    ${h}`).join("\n")}`,
  );
}
