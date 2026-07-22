// Shared logging/exit helpers for the repo's .mjs automation (Bun).
// Emits GitHub Actions workflow commands under CI, plain text locally.

const inCI = Boolean(process.env.GITHUB_ACTIONS);

/** Print an error and exit non-zero (default code 1). */
export function die(message, { code = 1 } = {}) {
  console.error(inCI ? `::error::${message}` : `✖ ${message}`);
  process.exit(code);
}

/** Print a non-fatal notice. */
export function notice(message) {
  console.error(inCI ? `::notice::${message}` : message);
}
