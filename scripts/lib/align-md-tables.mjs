// Align markdown tables in place so markdownlint's MD060 (table-column-style:
// aligned) passes — markdownlint-cli2 --fix has no auto-fixer for MD060.
// Ported from cerberus's scripts/align-md-tables.py; adds alignment-colon
// preservation. Column width follows terminal display width so emoji and CJK
// cells line up with markdownlint's `string-width`-based MD060 implementation.
//
// Usage (CLI): bun scripts/lib/align-md-tables.mjs FILE [FILE ...]
import { die } from "./ci.mjs";

const SEP_CELL = /^:?-+:?$/;
const SEGMENTER = new Intl.Segmenter(undefined, { granularity: "grapheme" });
const ZERO_WIDTH_CLUSTER = /^(?:\p{Default_Ignorable_Code_Point}|\p{Control}|\p{Mark}|\p{Surrogate})+$/u;
const NON_PRINTING = /[\p{Default_Ignorable_Code_Point}\p{Control}\p{Format}\p{Mark}\p{Surrogate}]/u;
const EMOJI = /\p{Emoji_Presentation}|\uFE0F/u;

/** Return whether a Unicode scalar has East Asian full/wide display width. */
function isFullwidth(codePoint) {
  return codePoint >= 0x1100 && (
    codePoint <= 0x115f ||
    codePoint === 0x2329 ||
    codePoint === 0x232a ||
    (codePoint >= 0x2e80 && codePoint <= 0x303e) ||
    (codePoint >= 0x3040 && codePoint <= 0xa4cf) ||
    (codePoint >= 0xac00 && codePoint <= 0xd7a3) ||
    (codePoint >= 0xf900 && codePoint <= 0xfaff) ||
    (codePoint >= 0xfe10 && codePoint <= 0xfe19) ||
    (codePoint >= 0xfe30 && codePoint <= 0xfe6f) ||
    (codePoint >= 0xff00 && codePoint <= 0xff60) ||
    (codePoint >= 0xffe0 && codePoint <= 0xffe6) ||
    (codePoint >= 0x1b000 && codePoint <= 0x1b2ff) ||
    (codePoint >= 0x1f200 && codePoint <= 0x1f251) ||
    (codePoint >= 0x20000 && codePoint <= 0x3fffd)
  );
}

/** Return the number of terminal columns occupied by a string. */
function displayWidth(value) {
  let width = 0;
  for (const { segment } of SEGMENTER.segment(value)) {
    if (ZERO_WIDTH_CLUSTER.test(segment)) continue;
    if (EMOJI.test(segment)) {
      width += 2;
      continue;
    }

    const visible = [...segment].find((character) => !NON_PRINTING.test(character));
    if (visible) width += isFullwidth(visible.codePointAt(0)) ? 2 : 1;
  }
  return width;
}

/** Render a separator cell as `[:]---…[:]` padded to `width` (colons preserved). */
function fmtSep(stripped, width) {
  const left = stripped.startsWith(":");
  const right = stripped.endsWith(":");
  const dashes = Math.max(1, width - (left ? 1 : 0) - (right ? 1 : 0));
  return ` ${left ? ":" : ""}${"-".repeat(dashes)}${right ? ":" : ""} `;
}

/** Align one contiguous block of table lines. Returns input unchanged if it isn't a well-formed table. */
export function alignTable(tableLines) {
  const rows = [];
  for (const line of tableLines) {
    const s = line.replace(/\n$/, "").trim();
    if (!s.startsWith("|") || !s.endsWith("|")) return tableLines;
    rows.push(s.slice(1, -1).split("|"));
  }
  if (rows.length === 0) return tableLines;

  const nCols = rows[0].length;
  const widths = new Array(nCols).fill(0);
  let sepIdx = -1;
  for (let i = 0; i < rows.length; i++) {
    if (rows[i].length !== nCols) return tableLines;
    for (let j = 0; j < nCols; j++) {
      const stripped = rows[i][j].trim();
      if (SEP_CELL.test(stripped)) sepIdx = i;
      widths[j] = Math.max(widths[j], displayWidth(stripped));
    }
  }

  return rows.map((row, i) => {
    const cells = row.map((cell, j) => {
      const stripped = cell.trim();
      if (i === sepIdx) return fmtSep(stripped, widths[j]);
      const pad = widths[j] - displayWidth(stripped);
      return ` ${stripped}${" ".repeat(pad)} `;
    });
    return `|${cells.join("|")}|\n`;
  });
}

/** Rewrite `path` with all its tables aligned. Returns true if the file changed. */
export async function alignFile(path) {
  const original = await Bun.file(path).text();
  const lines = original.split(/(?<=\n)/); // keep newlines
  const out = [];
  let block = [];
  const flush = () => {
    if (block.length) out.push(...alignTable(block));
    block = [];
  };
  for (const line of lines) {
    const t = line.trim();
    if (t.startsWith("|") && t.endsWith("|")) block.push(line);
    else {
      flush();
      out.push(line);
    }
  }
  flush();
  const next = out.join("");
  if (next !== original) {
    await Bun.write(path, next);
    return true;
  }
  return false;
}

if (import.meta.main) {
  const files = process.argv.slice(2);
  let changed = 0;
  for (const f of files) {
    try {
      if (await alignFile(f)) changed++;
    } catch (e) {
      die(`align-md-tables: ${f}: ${e.message}`);
    }
  }
  if (changed) console.error(`align-md-tables: aligned tables in ${changed} file(s)`);
}
