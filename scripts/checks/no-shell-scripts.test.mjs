import { expect, test } from "bun:test";

import {
  SHELL_SCRIPT_SUFFIXES,
  hasShellShebang,
  parseTrackedEntries,
  shellScriptEntries,
  shellScriptPaths,
} from "./no-shell-scripts.mjs";

test("rejects shell-script suffixes independent of case", () => {
  expect(
    shellScriptPaths([
      "scripts/check.mjs",
      "ci/build.sh",
      "hooks/VERIFY.BASH",
      "tools/bootstrap.zsh",
      "tools/check.ksh",
      "tools/check.dash",
      "tools/check.fish",
      "README.md",
    ]),
  ).toEqual([
    "ci/build.sh",
    "hooks/VERIFY.BASH",
    "tools/bootstrap.zsh",
    "tools/check.ksh",
    "tools/check.dash",
    "tools/check.fish",
  ]);
});

test("the protected extension list stays explicit and reviewable", () => {
  expect(SHELL_SCRIPT_SUFFIXES).toEqual([
    ".sh",
    ".bash",
    ".zsh",
    ".ksh",
    ".dash",
    ".fish",
  ]);
});

test("rejects shell shebangs even without a shell extension", () => {
  expect(hasShellShebang("#!/bin/sh")).toBeTrue();
  expect(hasShellShebang("#!/usr/bin/env bash -eu")).toBeTrue();
  expect(hasShellShebang("#!/usr/bin/env -S zsh -eu")).toBeTrue();
  expect(hasShellShebang("#!/usr/bin/env bun")).toBeFalse();
});

test("reports tracked executable and non-executable shell shebangs", () => {
  expect(
    shellScriptEntries([
      {
        mode: "100755",
        path: "tools/verify",
        firstLine: "#!/usr/bin/env bash",
      },
      { mode: "100644", path: "tools/legacy", firstLine: "#!/bin/dash" },
      {
        mode: "100755",
        path: "scripts/check.mjs",
        firstLine: "#!/usr/bin/env bun",
      },
      { mode: "100644", path: "docs/notes.md", firstLine: "" },
    ]),
  ).toEqual([
    {
      mode: "100755",
      path: "tools/verify",
      firstLine: "#!/usr/bin/env bash",
      reason: "executable shell shebang",
    },
    {
      mode: "100644",
      path: "tools/legacy",
      firstLine: "#!/bin/dash",
      reason: "shell shebang",
    },
  ]);
});

test("parses Git index modes before reading tracked files", () => {
  const index = [
    "100755 deadbeef 0\ttools/verify",
    "100644 beadfeed 0\tdocs/readme.md",
    "",
  ].join("\0");
  expect(parseTrackedEntries(index)).toEqual([
    { mode: "100755", path: "tools/verify" },
    { mode: "100644", path: "docs/readme.md" },
  ]);
});
