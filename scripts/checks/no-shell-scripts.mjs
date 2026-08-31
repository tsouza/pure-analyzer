#!/usr/bin/env bun

// The constitution forbids checked-in shell scripts. Inspect Git's tracked
// entries rather than path-filter output so an extensionless script cannot
// make the structural gate disappear.
import { $ } from "bun";
import { lstat, open } from "node:fs/promises";

import { die } from "../lib/ci.mjs";

export const SHELL_SCRIPT_SUFFIXES = [
  ".sh",
  ".bash",
  ".zsh",
  ".ksh",
  ".dash",
  ".fish",
];
export const SHELL_INTERPRETERS = ["sh", "bash", "zsh", "ksh", "dash", "fish"];
export const SHEBANG_READ_BYTES = 4 * 1024;

function basename(path) {
  return path.split("/").at(-1)?.toLowerCase() ?? "";
}

function shellInterpreter(firstLine) {
  if (!firstLine.startsWith("#!")) return "";
  const words = firstLine.slice(2).trim().split(/\s+/).filter(Boolean);
  let index = 0;
  let interpreter = basename(words[index]);
  if (interpreter !== "env") return interpreter;

  index += 1;
  while (words[index]?.startsWith("-")) index += 1;
  return basename(words[index]);
}

/** Return tracked paths whose extension makes them prohibited shell scripts. */
export function shellScriptPaths(paths) {
  return paths.filter((path) => {
    const lowerCasePath = path.toLowerCase();
    return SHELL_SCRIPT_SUFFIXES.some((suffix) =>
      lowerCasePath.endsWith(suffix),
    );
  });
}

/** Whether a first line selects a supported shell interpreter. */
export function hasShellShebang(firstLine) {
  return SHELL_INTERPRETERS.includes(shellInterpreter(firstLine));
}

/** Parse NUL-delimited `git ls-files --stage` output into tracked entries. */
export function parseTrackedEntries(output) {
  return output
    .split("\0")
    .filter(Boolean)
    .flatMap((entry) => {
      const tab = entry.indexOf("\t");
      if (tab === -1) return [];
      const [mode] = entry.slice(0, tab).split(" ", 1);
      const path = entry.slice(tab + 1);
      return mode && path ? [{ mode, path }] : [];
    });
}

function executable(mode) {
  return (Number.parseInt(mode, 8) & 0o111) !== 0;
}

/** Return prohibited tracked entries, including extensionless shell shebangs. */
export function shellScriptEntries(entries) {
  return entries.flatMap((entry) => {
    if (shellScriptPaths([entry.path]).length > 0) {
      return [{ ...entry, reason: "shell extension" }];
    }
    if (!hasShellShebang(entry.firstLine ?? "")) return [];
    return [
      {
        ...entry,
        reason: executable(entry.mode)
          ? "executable shell shebang"
          : "shell shebang",
      },
    ];
  });
}

async function firstLine(path) {
  let file;
  try {
    file = await open(path, "r");
    const buffer = Buffer.alloc(SHEBANG_READ_BYTES);
    const { bytesRead } = await file.read(buffer, 0, buffer.length, 0);
    return (
      buffer.subarray(0, bytesRead).toString("utf8").split(/\r?\n/, 1)[0] ?? ""
    );
  } catch {
    return "";
  } finally {
    await file?.close().catch(() => {});
  }
}

async function trackedEntries() {
  const output = await $`git ls-files --stage -z`.text();
  const entries = parseTrackedEntries(output);
  const present = await Promise.all(
    entries.map(async (entry) => {
      try {
        const details = await lstat(entry.path);
        if (shellScriptPaths([entry.path]).length > 0) return entry;
        if (!details.isFile()) return undefined;
        return { ...entry, firstLine: await firstLine(entry.path) };
      } catch {
        return undefined;
      }
    }),
  );
  return present.filter(Boolean);
}

if (import.meta.main) {
  const entries = shellScriptEntries(await trackedEntries());
  if (entries.length) {
    die(
      `tracked shell scripts violate constitution §2 (use a just target, cargo xtask, or Bun .mjs):\n${entries
        .map(({ path, reason }) => `    ${path} (${reason})`)
        .join("\n")}`,
    );
  }
}
