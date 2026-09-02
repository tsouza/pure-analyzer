#!/usr/bin/env bun
// Refresh frozen Legend evidence for M4a comparison inputs. Rust replays the
// lowerable source pairs hermetically; this tool only contacts the pinned oracle.

import { die, notice } from "./lib/ci.mjs";
import {
  assertMetadata,
  CORPUS_DIRECTORY,
  METADATA_PATH,
} from "./analysis-semantic-corpus.mjs";
import {
  assertEngineVersion,
  canonicalJson,
  checkedEngineVersion,
  executeEvidence,
  jsonEqual,
  legendEngineBaseUrl,
} from "./lib/legend-engine.mjs";

export {
  EngineUnavailableError,
  PinnedEngineError,
  assertEngineVersion,
  canonicalJson,
  jsonEqual,
} from "./lib/legend-engine.mjs";

export const CASES_PATH = `${CORPUS_DIRECTORY}/comparison.jsonl`;
export const EQUIVALENT = "equivalent";
export const NOT_EQUIVALENT = "not_equivalent";
export const INDECISIVE = "indecisive";
export const INDECISIVE_REASON = "IND_MISSING_REWRITE";
export const OUTPUT_COLUMN_FIELDS = Object.freeze([
  "name",
  "type",
  "multiplicity",
  "nullability",
]);

function isObject(value) {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function nonEmptyString(value) {
  return typeof value === "string" && value.trim().length > 0;
}

function assertExactFields(value, fields, path) {
  const actual = Object.keys(value).sort();
  const expected = [...fields].sort();
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(`${path}: unexpected corpus fields`);
  }
}

function assertIntegerValues(value, path) {
  if (!Array.isArray(value) || value.length === 0 || value.some((item) => !Number.isSafeInteger(item))) {
    throw new Error(`${path}: values must be a non-empty integer list`);
  }
  return value;
}

function assertStringValues(value, path) {
  if (!Array.isArray(value) || value.length === 0 || value.some((item) => !nonEmptyString(item))) {
    throw new Error(`${path}: values must be a non-empty string list`);
  }
  return value;
}

function pureString(value) {
  return `'${value.replaceAll("'", "''")}'`;
}

/** Render the bounded engine oracle that is mechanically tied to one M3 source shape. */
export function oracleLambda(value, path = "oracle") {
  if (!isObject(value)) throw new Error(`${path}: oracle must be a JSON object`);
  switch (value.kind) {
    case "scan": {
      assertExactFields(value, ["kind", "values"], path);
      return `|[${assertIntegerValues(value.values, `${path}:values`).join(", ")}]`;
    }
    case "filter_true": {
      assertExactFields(value, ["kind", "values"], path);
      return `|[${assertIntegerValues(value.values, `${path}:values`).join(", ")}]->filter(x: Integer[1]|true)`;
    }
    case "ordered_columns": {
      assertExactFields(value, ["kind", "columns"], path);
      const columns = assertStringValues(value.columns, `${path}:columns`);
      if (new Set(columns).size !== columns.length) {
        throw new Error(`${path}: ordered column names must be unique`);
      }
      return `|[${columns.map(pureString).join(", ")}]`;
    }
    case "literal_filter": {
      assertExactFields(value, ["kind", "values", "value"], path);
      const values = assertStringValues(value.values, `${path}:values`);
      if (!nonEmptyString(value.value) || !values.includes(value.value)) {
        throw new Error(`${path}: literal filter value must be one of its input values`);
      }
      return `|[${values.map(pureString).join(", ")}]->filter(x: String[1]|$x == ${pureString(value.value)})`;
    }
    default:
      throw new Error(`${path}: unsupported bounded oracle ${JSON.stringify(value.kind)}`);
  }
}

function assertEvidence(value, path, decisive) {
  if (!isObject(value)) throw new Error(`${path}: evidence must be a JSON object`);
  assertExactFields(
    value,
    decisive ? ["source", "oracle", "lambda", "result"] : ["source", "oracle", "lambda"],
    path,
  );
  for (const field of ["source", "lambda"]) {
    if (!nonEmptyString(value[field])) {
      throw new Error(`${path}: evidence ${field} must be a non-empty string`);
    }
  }
  const expectedLambda = oracleLambda(value.oracle, `${path}:oracle`);
  if (value.lambda !== expectedLambda) {
    throw new Error(`${path}: lambda must exactly render its bounded oracle`);
  }
  return value;
}

function assertDifference(value, path) {
  if (!isObject(value)) throw new Error(`${path}: difference must be a JSON object`);
  assertExactFields(value, ["kind", "index", "field"], path);
  if (value.kind !== "output_column") {
    throw new Error(`${path}: only output_column is a committed M4a refutation`);
  }
  if (!Number.isSafeInteger(value.index) || value.index < 0) {
    throw new Error(`${path}: difference index must be a non-negative integer`);
  }
  if (!OUTPUT_COLUMN_FIELDS.includes(value.field)) {
    throw new Error(`${path}: unsupported output-column field`);
  }
  return value;
}

/** Validate one M4a comparison witness and return it unchanged. */
export function assertFixture(value, path, line) {
  const fixturePath = `${path}:${line}`;
  if (!isObject(value)) throw new Error(`${fixturePath}: fixture must be a JSON object`);
  if (!nonEmptyString(value.outcome)) {
    throw new Error(`${fixturePath}: fixture outcome must be a non-empty string`);
  }
  const decisive = value.outcome === EQUIVALENT || value.outcome === NOT_EQUIVALENT;
  if (value.outcome === EQUIVALENT) {
    assertExactFields(value, ["id", "model", "left", "right", "outcome"], fixturePath);
  } else if (value.outcome === NOT_EQUIVALENT) {
    assertExactFields(
      value,
      ["id", "model", "left", "right", "outcome", "difference"],
      fixturePath,
    );
    assertDifference(value.difference, `${fixturePath}:difference`);
  } else if (value.outcome === INDECISIVE) {
    assertExactFields(value, ["id", "model", "left", "right", "outcome", "reason"], fixturePath);
    if (value.reason !== INDECISIVE_REASON) {
      throw new Error(`${fixturePath}: indecisive reason must be ${INDECISIVE_REASON}`);
    }
  } else {
    throw new Error(`${fixturePath}: unsupported outcome ${JSON.stringify(value.outcome)}`);
  }
  for (const field of ["id", "model"]) {
    if (!nonEmptyString(value[field])) {
      throw new Error(`${fixturePath}: fixture ${field} must be a non-empty string`);
    }
  }
  const left = assertEvidence(value.left, `${fixturePath}:left`, decisive);
  const right = assertEvidence(value.right, `${fixturePath}:right`, decisive);
  if (value.outcome === EQUIVALENT && !jsonEqual(left.result, right.result)) {
    throw new Error(`${fixturePath}: declared equivalent conflicts with frozen results`);
  }
  if (value.outcome === NOT_EQUIVALENT && jsonEqual(left.result, right.result)) {
    throw new Error(`${fixturePath}: declared not_equivalent conflicts with frozen results`);
  }
  return value;
}

/** Parse non-empty JSONL M4a comparison witnesses. */
export function parseFixtures(text, path) {
  return text
    .split("\n")
    .map((line, index) => ({ line, number: index + 1 }))
    .filter(({ line }) => line.trim().length > 0)
    .map(({ line, number }) => {
      try {
        return assertFixture(JSON.parse(line), path, number);
      } catch (error) {
        if (error instanceof SyntaxError) {
          throw new Error(`${path}:${number}: invalid JSON: ${error.message}`);
        }
        throw error;
      }
    });
}

/** Reject corpus drift that removes an M4a outcome class or duplicates a witness. */
export function assertCorpus(fixtures) {
  if (fixtures.length === 0) throw new Error("comparison.jsonl must contain M4a witnesses");
  const ids = new Set();
  const outcomes = new Set();
  for (const fixture of fixtures) {
    if (!ids.add(fixture.id)) throw new Error(`duplicate fixture id ${JSON.stringify(fixture.id)}`);
    outcomes.add(fixture.outcome);
  }
  for (const outcome of [EQUIVALENT, NOT_EQUIVALENT, INDECISIVE]) {
    if (!outcomes.has(outcome)) {
      throw new Error(`comparison corpus must retain an explicit ${outcome} outcome`);
    }
  }
  return fixtures;
}

async function readJson(path) {
  try {
    return JSON.parse(await Bun.file(path).text());
  } catch (error) {
    throw new Error(`could not read ${path}: ${error.message}`);
  }
}

async function loadCorpus() {
  const metadata = assertMetadata(await readJson(METADATA_PATH));
  let text;
  try {
    text = await Bun.file(CASES_PATH).text();
  } catch (error) {
    throw new Error(`could not read ${CASES_PATH}: ${error.message}`);
  }
  return { metadata, fixtures: assertCorpus(parseFixtures(text, CASES_PATH)) };
}

async function verifyFixture(base, metadata, fixture) {
  if (fixture.outcome === INDECISIVE) return;
  const left = await executeEvidence(base, metadata, fixture, "left");
  const right = await executeEvidence(base, metadata, fixture, "right");
  for (const [side, observed] of [["left", left], ["right", right]]) {
    if (!jsonEqual(observed, fixture[side].result)) {
      throw new Error(
        `${fixture.id}: ${side} result diverged from frozen evidence\n` +
          `frozen: ${canonicalJson(fixture[side].result)}\n` +
          `observed: ${canonicalJson(observed)}`,
      );
    }
  }
  const equal = jsonEqual(left, right);
  if ((fixture.outcome === EQUIVALENT) !== equal) {
    throw new Error(
      `${fixture.id}: live results conflict with declared ${fixture.outcome} outcome`,
    );
  }
}

async function main() {
  if (process.argv.slice(2).join(" ") !== "--refresh") {
    die(
      "usage: just analysis-comparison-corpus-refresh " +
        "(the hermetic verifier is just analysis-comparison-corpus-verify)",
    );
  }
  let corpus;
  try {
    corpus = await loadCorpus();
  } catch (error) {
    die(`invalid analysis comparison corpus: ${error.message}`);
  }
  try {
    const base = legendEngineBaseUrl();
    await checkedEngineVersion(base, corpus.metadata.engine_version);
    for (const fixture of corpus.fixtures) {
      await verifyFixture(base, corpus.metadata, fixture);
    }
  } catch (error) {
    die(error.message);
  }
  const decisiveCount = corpus.fixtures.filter((fixture) => fixture.outcome !== INDECISIVE).length;
  notice(
    `verified ${decisiveCount} decisive M4a witnesses against Legend ` +
      `${corpus.metadata.engine_version}; indecisive witnesses remain explicitly unproven`,
  );
}

if (import.meta.main) await main();
