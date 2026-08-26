#!/usr/bin/env bun
// Reject time-frozen self-description in PureCARD's shipped src/** doc-comments.
// A shipped crate must not call itself a scaffold/stub/future work: code is the
// source of truth for what exists, and a module doc frozen at an earlier
// milestone silently lies to every reader. Status lives in one tracked place,
// not scattered across module headers (constitution §5; docs/lessons.md).
//   default : scan staged crates/pure-analyzer-purecard/src/**/*.rs
//   --all   : scan all tracked crates/pure-analyzer-purecard/src/**/*.rs
//
// A genuine, still-accurate deferral is allowed through an inline or immediately
// preceding `// stale-ok: <reason >= 12 chars>`; a bare `// stale-ok:` is itself
// an error. Every honoured suppression is echoed to stderr so the CI no-warnings
// log sweep keeps deliberate deferrals visible and auditable.
import { $ } from "bun";
import { stagedFiles } from "../lib/git.mjs";
import { die, notice } from "../lib/ci.mjs";

/** Repository-relative root of the shipped PureCARD Rust sources. */
export const PURECARD_SRC = "crates/pure-analyzer-purecard/src/";

// PROTECTED ratchet (constitution §7): these arrays only grow. Removing a
// pattern needs a human. Phrase-anchored (not bare words) so ordinary impl prose
// — "narrow does not consume", "lands in [InSourceIdent]" — never trips them.
export const BANNED = [
  /\b(scaffold(ing)?|skeleton)\b/i,
  /\bstub\b|\bStubDecoder\b/i,
  /\bstill absent\b/i,
  /\bnot \(?yet\)? (built|implemented|wired|acted on|narrowed|enumerat\w+)\b/i,
  /\b(later|future|next) (milestone|task|release)\b/i,
  /\blands? in a (later|future)\b|\barrives? at M[0-5]\b/i,
  /\bM0[\s-]*(only|scaffold|skeleton)\b/i,
  /\bwill be (built|added|supplied|wired|implemented)\b|\bto be built\b/i,
  /\b(TODO|FIXME|XXX)\b/, // doc-comment TODOs the macro/comment gate cannot see
];

// Only banned inside a `//!` module header, where they describe the module
// itself as throwaway/provisional. A `///` item doc may legitimately say a
// buffer is a "throwaway stack" (accurate current behaviour), so those are only
// self-description smells at the module-header altitude.
export const HEADER_BANNED = [/\bfor now\b/i, /\bthrowaway\b/i];

// A justified suppression: `// stale-ok: <reason>`. The reason must be real
// (>= this many chars) or the marker is itself an error.
const SUPPRESS = /\/\/\s*stale-ok:\s*(.*)$/;
const STANDALONE_SUPPRESS = /^\s*\/\/\s*stale-ok:\s*(.*)$/;
export const MIN_REASON_LEN = 12;

/**
 * The trimmed stale-ok reason on `line`, or null if it carries no marker.
 * Preceding-line suppressions must be standalone Rust `//` comments; inline
 * suppressions are considered only on the doc-comment currently being scanned.
 */
function markerReason(line, { standalone = false } = {}) {
  if (line === undefined) return null;
  const match = line.match(standalone ? STANDALONE_SUPPRESS : SUPPRESS);
  return match ? match[1].trim() : null;
}

/**
 * Scan one file's text. Pure and unit-tested — no I/O, no process exit.
 * @param {string} text
 * @returns {{hits: Array<{line:number,text:string,pattern:string}>,
 *            suppressions: Array<{line:number,reason:string}>}}
 */
export function scan(text) {
  const lines = text.split("\n");
  const hits = [];
  const suppressions = [];

  lines.forEach((raw, index) => {
    const trimmed = raw.trimStart();
    const isDoc = trimmed.startsWith("///") || trimmed.startsWith("//!");
    if (!isDoc) return;
    const patterns = trimmed.startsWith("//!") ? [...BANNED, ...HEADER_BANNED] : BANNED;
    const pattern = patterns.find((candidate) => candidate.test(raw));
    if (!pattern) return;

    const here = markerReason(raw);
    const above = markerReason(lines[index - 1], { standalone: true });
    const reason = [here, above].find(
      (candidate) => candidate !== null && candidate.length >= MIN_REASON_LEN,
    );
    if (reason !== undefined) {
      suppressions.push({ line: index + 1, reason });
      return;
    }
    hits.push({ line: index + 1, text: trimmed, pattern: String(pattern) });
  });

  // A bare or too-short `// stale-ok:` is itself an error: the escape hatch must
  // always carry a real justification. Ignore marker-shaped source text: only a
  // doc-comment's inline marker or a standalone Rust comment is an escape hatch.
  lines.forEach((raw, index) => {
    const trimmed = raw.trimStart();
    const isDoc = trimmed.startsWith("///") || trimmed.startsWith("//!");
    const reason = markerReason(raw, { standalone: !isDoc });
    if (reason !== null && reason.length < MIN_REASON_LEN) {
      hits.push({
        line: index + 1,
        text: raw.trim(),
        pattern: `bare stale-ok (reason must be >= ${MIN_REASON_LEN} chars)`,
      });
    }
  });

  return { hits, suppressions };
}

/** Keep only repository-relative Rust paths inside PureCARD's shipped source tree. */
export function purecardSourceFiles(paths) {
  return paths.filter((path) => path.startsWith(PURECARD_SRC) && path.endsWith(".rs"));
}

/** Git object expression for the staged version of a repository-relative path. */
export function stagedObject(path) {
  return `:${path}`;
}

async function trackedPurecardSourceFiles() {
  const out = await $`git ls-files -- ${PURECARD_SRC}`.text();
  return purecardSourceFiles(out.split("\n").filter(Boolean));
}

async function filesToScan() {
  if (process.argv.includes("--all")) return trackedPurecardSourceFiles();
  return purecardSourceFiles(await stagedFiles({ suffix: ".rs" }));
}

async function sourceText(path, scanAll) {
  if (scanAll) return Bun.file(path).text();
  return $`git show ${stagedObject(path)}`.text();
}

async function main() {
  const scanAll = process.argv.includes("--all");
  const files = await filesToScan();
  const allHits = [];
  for (const path of files) {
    const { hits, suppressions } = scan(await sourceText(path, scanAll));
    for (const suppression of suppressions) {
      notice(`stale-ok honoured ${path}:${suppression.line}: ${suppression.reason}`);
    }
    for (const hit of hits) {
      allHits.push(`${path}:${hit.line}: ${hit.text}  [${hit.pattern}]`);
    }
  }

  if (allHits.length) {
    die(
      `stale self-description in shipped PureCARD doc-comments — reword to present-tense fact, ` +
        `or justify a genuine deferral with an inline \`// stale-ok: <reason>\`:\n${allHits
          .map((hit) => `    ${hit}`)
          .join("\n")}`,
    );
  }
}

if (import.meta.main) await main();
