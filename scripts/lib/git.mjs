// Shared git helpers for the repo's .mjs automation (Bun). See constitution §2.
import { $ } from "bun";

/** Absolute path to the repository root. */
export async function repoRoot() {
  return (await $`git rev-parse --show-toplevel`.text()).trim();
}

/**
 * Staged files (Added/Copied/Modified/Renamed), NUL-safe.
 * @param {{suffix?: string}} [opts] optionally keep only paths ending in `suffix`.
 * @returns {Promise<string[]>}
 */
export async function stagedFiles({ suffix } = {}) {
  const out = await $`git diff --cached --name-only --diff-filter=ACMR -z`.text();
  let files = out.split("\0").filter(Boolean);
  if (suffix) files = files.filter((f) => f.endsWith(suffix));
  return files;
}

/**
 * Added (`+`) lines across a staged diff, with the leading `+` stripped and the
 * `+++` file headers removed.
 * @param {string[]} paths pathspecs to diff (empty = all staged).
 * @returns {Promise<string[]>}
 */
export async function stagedAddedLines(paths = []) {
  const out = await $`git diff --cached -U0 --diff-filter=ACMR -- ${paths}`.text();
  return out
    .split("\n")
    .filter((l) => l.startsWith("+") && !l.startsWith("+++"))
    .map((l) => l.slice(1));
}
