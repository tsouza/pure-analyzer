// Shared logging/exit helpers for the repo's .mjs automation (Bun).
// Emits GitHub Actions workflow commands under CI, plain text locally.

const inCI = Boolean(process.env.GITHUB_ACTIONS);

/**
 * Print an error annotation without exiting. For a run that must collect
 * several failures across a full scan and report them together — e.g. every
 * unclassifiable issue in a labeler backfill — before dying once at the end.
 */
export function error(message) {
  console.error(inCI ? `::error::${message}` : `✖ ${message}`);
}

/** Print an error and exit non-zero (default code 1). */
export function die(message, { code = 1 } = {}) {
  error(message);
  process.exit(code);
}

/** Print a non-fatal notice. */
export function notice(message) {
  console.error(inCI ? `::notice::${message}` : message);
}
