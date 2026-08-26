#!/usr/bin/env bun
// Label PureCARD's differential corpus with the Legend engine's grammar verdict.
//
// The decoder core is pure and never calls the engine (constitution §1); this is
// offline tooling, run on demand by a maintainer who has a Legend engine. It
// POSTs each query in the nested PureCARD corpus to the engine's grammar parser
// and freezes the verdict (`parse_ok` / `parse_fail`) back into the committed
// file. CI then replays the frozen corpus against L1 without an engine.
//
//   just label-differential                 # localhost:6300
//   LEGEND_ENGINE_URL=… just label-differential
//
// Engine contract (verified): POST the raw lambda string as text/plain to
// `…/grammar/grammarToJson/lambda` → HTTP 200 == parses, HTTP 400 == PARSER error.
import { die, notice } from "./lib/ci.mjs";

const BASE = process.env.LEGEND_ENGINE_URL ?? "http://localhost:6300";
const GRAMMAR = `${BASE}/api/pure/v1/grammar/grammarToJson/lambda`;
const INFO = `${BASE}/api/server/v1/info`;
const CORPUS = "crates/pure-analyzer-purecard/corpus/differential_l1.jsonl";
const HTTP_OK = 200;
const HTTP_PARSER_ERROR = 400;
const REQUEST_TIMEOUT_MS = 8000;

// The corpus verdicts (and the whole gold corpus) are validated against exactly
// this Legend version. Labeling against a different engine would silently
// produce verdicts for a different grammar, so the version is asserted. Set
// LEGEND_SKIP_VERSION_CHECK=1 only for a deliberate, corpus-revalidating re-pin.
const PINNED_ENGINE_VERSION = "4.113.0";

async function assertEngineVersion() {
  if (process.env.LEGEND_SKIP_VERSION_CHECK === "1") {
    notice("LEGEND_SKIP_VERSION_CHECK=1 — engine version not verified");
    return;
  }
  let info;
  try {
    const res = await fetch(INFO, { signal: AbortSignal.timeout(REQUEST_TIMEOUT_MS) });
    info = await res.json();
  } catch (err) {
    die(
      `could not read engine version at ${INFO}: ${err.message}\n` +
        `start a Legend ${PINNED_ENGINE_VERSION} engine, or set LEGEND_SKIP_VERSION_CHECK=1`,
    );
  }
  const version = info?.info?.legendSDLC?.["git.build.version"] ?? "unknown";
  if (version !== PINNED_ENGINE_VERSION) {
    die(
      `engine version mismatch: running ${version}, corpus is validated against ` +
        `${PINNED_ENGINE_VERSION}. Labeling against a different grammar would corrupt the ` +
        `corpus. Run a ${PINNED_ENGINE_VERSION} engine, or (for a deliberate re-pin) set ` +
        `LEGEND_SKIP_VERSION_CHECK=1 and update PINNED_ENGINE_VERSION + ` +
        `crates/pure-analyzer-purecard/docs/spec/grammar.md.`,
    );
  }
  notice(`engine version ${version} matches the pinned ${PINNED_ENGINE_VERSION}`);
}

async function verdict(query) {
  let res;
  try {
    res = await fetch(GRAMMAR, {
      method: "POST",
      headers: { "Content-Type": "text/plain" },
      body: query,
      signal: AbortSignal.timeout(REQUEST_TIMEOUT_MS),
    });
  } catch (err) {
    die(`engine unreachable at ${GRAMMAR}: ${err.message}`);
  }
  if (res.status === HTTP_OK) return "parse_ok";
  if (res.status === HTTP_PARSER_ERROR) return "parse_fail";
  die(`unexpected engine status ${res.status} for query: ${query}`);
}

await assertEngineVersion();

const text = await Bun.file(CORPUS).text();
const rows = text
  .split("\n")
  .filter((line) => line.trim().length > 0)
  .map((line) => JSON.parse(line));
let flips = 0;
const out = [];
for (const row of rows) {
  const fresh = await verdict(row.q);
  if (row.legend && row.legend !== fresh) {
    notice(`verdict changed (${row.legend} → ${fresh}): ${row.q}`);
    flips += 1;
  }
  out.push(JSON.stringify({ ...row, legend: fresh }));
}
// The committed verdicts are frozen engine ground truth the CI gate replays. A
// run that flips any of them must be an explicit, deliberate revalidation, not
// a silent overwrite. Refuse to write flips unless the maintainer opts in.
if (flips > 0 && process.env.LEGEND_ALLOW_VERDICT_FLIPS !== "1") {
  die(
    `${flips} verdict change(s) would overwrite frozen engine ground truth; refusing. ` +
      `Confirm the engine is a faithful ${PINNED_ENGINE_VERSION} and the changes are intended, ` +
      `then re-run with LEGEND_ALLOW_VERDICT_FLIPS=1.`,
  );
}
await Bun.write(CORPUS, `${out.join("\n")}\n`);
notice(`labelled ${out.length} queries` + (flips ? ` (${flips} verdict change(s))` : ""));
