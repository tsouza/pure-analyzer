import { expect, test } from "bun:test";

import { MARKERS, ereSafe, hasMarker } from "./postponed-markers.mjs";

test("every banned marker is matched", () => {
  for (const line of [
    "    // TODO: come back to this",
    "// FIXME later",
    "// XXX",
    "    #[ignore]",
    "    true /* ~ changed by cargo-mutants ~ */",
  ]) {
    expect(hasMarker(line)).toBe(true);
  }
});

test("ordinary source lines are left alone", () => {
  for (const line of [
    "    let todo = compute();",
    "/// Documents the XXXL variant.",
    "    fn ignore_case(s: &str) -> String {",
    "// cargo-mutants is run from the mutation just target",
  ]) {
    expect(hasMarker(line)).toBe(false);
  }
});

// The pattern feeds two regex engines — JavaScript's, for the staged-line
// filter, and `git grep -E`'s POSIX ERE, for `--all`. A JS-only construct would
// keep the pre-commit hook working while silently turning `--all` into a
// literal-text search: the CI gate would go green for the wrong reason, which
// is the exact failure mode this file's mutation marker exists to prevent.
test("the shared pattern stays in the dialect git grep -E also understands", () => {
  expect(ereSafe(MARKERS)).toBe(true);
  expect(ereSafe(String.raw`(?:TODO)`)).toBe(false);
  expect(ereSafe(String.raw`\d+`)).toBe(false);
});
