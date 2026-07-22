#!/usr/bin/env bun
// Reject postponed-work markers (TODO/FIXME/XXX/#[ignore]) in Rust sources.
// The ast-grep rule already bans todo!()/unimplemented!()/unreachable!() macros;
// this covers the comment markers those can't see.
//   default : scan added lines of STAGED *.rs (git pre-commit, via lefthook)
//   --all   : scan all tracked Rust sources (CI structural gate)
import { $ } from "bun";
import { stagedFiles, stagedAddedLines } from "../lib/git.mjs";
import { die } from "../lib/ci.mjs";

const PATTERN = /\b(?:TODO|FIXME|XXX)\b|#\[ignore\]/;
const RUST_PATHSPECS = ["crates/**/*.rs", "xtask/**/*.rs", "fuzz/**/*.rs"];

async function hits() {
  if (process.argv.includes("--all")) {
    const out = await $`git grep -nE ${"\\b(TODO|FIXME|XXX)\\b|#\\[ignore\\]"} -- ${RUST_PATHSPECS}`
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
    `postponed-work markers found — resolve or file an issue:\n${found
      .map((h) => `    ${h}`)
      .join("\n")}`,
  );
}
