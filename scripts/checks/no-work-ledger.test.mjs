import { expect, test } from "bun:test";

import { WORK_LEDGER_PATHS, workLedgerPaths } from "./no-work-ledger.mjs";

test("rejects retired checked-in work ledgers", () => {
    expect(WORK_LEDGER_PATHS).toEqual([
      "specs",
      "docs/design/pure-analyzer-design.md",
      "docs/lessons.md",
      "crates/pure-analyzer-purecard/docs/lessons.md",
      ".claude/commands/spec.md",
      ".claude/skills/spec",
      "docs/methodology/spec-driven.md",
      "crates/pure-analyzer-purecard/docs/methodology/spec-driven.md",
  ]);
  expect(
    workLedgerPaths([
      "specs/old-feature.md",
      "specs",
      "docs/design/pure-analyzer-design.md",
      "docs/lessons.md",
      "crates/pure-analyzer-purecard/docs/lessons.md",
      ".claude/commands/spec.md",
      ".claude/skills/spec/SKILL.md",
      "docs/methodology/spec-driven.md",
      "crates/pure-analyzer-purecard/docs/methodology/spec-driven.md",
      "crates/pure-analyzer-purecard/docs/spec/grammar.md",
    ]),
  ).toEqual([
      "specs/old-feature.md",
      "specs",
      "docs/design/pure-analyzer-design.md",
      "docs/lessons.md",
      "crates/pure-analyzer-purecard/docs/lessons.md",
      ".claude/commands/spec.md",
      ".claude/skills/spec/SKILL.md",
      "docs/methodology/spec-driven.md",
      "crates/pure-analyzer-purecard/docs/methodology/spec-driven.md",
  ]);
});
