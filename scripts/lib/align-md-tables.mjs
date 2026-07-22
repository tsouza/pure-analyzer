// Align markdown tables in place so markdownlint's MD060 (table-column-style:
// aligned) passes — markdownlint-cli2 --fix has no auto-fixer for MD060.
// Ported from cerberus's scripts/align-md-tables.py; adds alignment-colon
// preservation. Column width = codepoint count (assumes display-width-1 chars;
// swap for a wcwidth measure if emoji/CJK ever appear inside a table).
//
// Usage (CLI): bun scripts/lib/align-md-tables.mjs FILE [FILE ...]
import { die } from "./ci.mjs";

const SEP_CELL = /^:?-+:?$/;

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
      widths[j] = Math.max(widths[j], [...stripped].length);
    }
  }

  return rows.map((row, i) => {
    const cells = row.map((cell, j) => {
      const stripped = cell.trim();
      if (i === sepIdx) return fmtSep(stripped, widths[j]);
      const pad = widths[j] - [...stripped].length;
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
