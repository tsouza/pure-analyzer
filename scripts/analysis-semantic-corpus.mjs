#!/usr/bin/env bun
// Verify the analyzer-owned semantic witness corpus against the exact Legend
// engine that produced it. Ordinary corpus replay is hermetic and runs in Rust.

import { die, notice } from "./lib/ci.mjs";
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

export const CORPUS_DIRECTORY = "crates/pure-analyzer-analysis/corpus/legend-4.113.0";
export const METADATA_PATH = `${CORPUS_DIRECTORY}/metadata.json`;
export const CASES_PATH = `${CORPUS_DIRECTORY}/cases.jsonl`;
export const SCHEMA_VERSION = 1;
export const EQUAL = "equal";
export const DIFFERENT = "different";
export const INDECISIVE = "indecisive";
// Required semantic classes live in code so a corpus-only change cannot
// silently remove the evidence needed to guard a future rewrite.
export const CANONICAL_FAMILIES = Object.freeze([
  "row-order",
  "bag-semantics",
  "three-valued-logic",
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

function assertApiEndpoint(value, field) {
  if (!nonEmptyString(value[field]) || !/^\/api\/.+/.test(value[field])) {
    throw new Error(`metadata ${field} must be an absolute API path`);
  }
}

function assertEvidence(value, path, decisive) {
  if (!isObject(value)) throw new Error(`${path}: evidence must be a JSON object`);
  assertExactFields(value, decisive ? ["lambda", "result"] : ["lambda"], path);
  if (!nonEmptyString(value.lambda)) {
    throw new Error(`${path}: evidence lambda must be a non-empty string`);
  }
  if (decisive && !Object.hasOwn(value, "result")) {
    throw new Error(`${path}: decisive evidence must include a result`);
  }
  return value;
}

/** Validate and return the immutable engine-pinned corpus metadata. */
export function assertMetadata(value) {
  if (!isObject(value)) throw new Error("metadata must be a JSON object");
  assertExactFields(
    value,
    [
      "schema_version",
      "engine_version",
      "model_endpoint",
      "lambda_endpoint",
      "execution_endpoint",
      "required_families",
    ],
    "metadata",
  );
  if (value.schema_version !== SCHEMA_VERSION) {
    throw new Error(`unsupported metadata schema version ${value.schema_version}`);
  }
  if (!/^\d+\.\d+\.\d+$/.test(value.engine_version ?? "")) {
    throw new Error("metadata engine_version must be an exact x.y.z pin");
  }
  for (const field of ["model_endpoint", "lambda_endpoint", "execution_endpoint"]) {
    assertApiEndpoint(value, field);
  }
  if (
    !Array.isArray(value.required_families) ||
    value.required_families.length === 0 ||
    value.required_families.some((family) => !nonEmptyString(family)) ||
    new Set(value.required_families).size !== value.required_families.length
  ) {
    throw new Error("metadata required_families must be a non-empty unique string list");
  }
  const requiredFamilies = new Set(value.required_families);
  if (
    requiredFamilies.size !== CANONICAL_FAMILIES.length ||
    CANONICAL_FAMILIES.some((family) => !requiredFamilies.has(family))
  ) {
    throw new Error("metadata required_families must exactly list the canonical semantic classes");
  }
  return value;
}

/** Validate one semantic witness and return it unchanged. */
export function assertFixture(value, path, line) {
  const fixturePath = `${path}:${line}`;
  if (!isObject(value)) throw new Error(`${fixturePath}: fixture must be a JSON object`);
  if (!nonEmptyString(value.outcome)) {
    throw new Error(`${fixturePath}: fixture outcome must be a non-empty string`);
  }
  const decisive = value.outcome === EQUAL || value.outcome === DIFFERENT;
  if (decisive) {
    assertExactFields(
      value,
      ["id", "family", "candidate", "model", "left", "right", "outcome"],
      fixturePath,
    );
  } else if (value.outcome === INDECISIVE) {
    assertExactFields(
      value,
      ["id", "family", "candidate", "model", "probe", "outcome", "reason"],
      fixturePath,
    );
  } else {
    throw new Error(`${fixturePath}: unsupported outcome ${JSON.stringify(value.outcome)}`);
  }
  for (const field of ["id", "family", "candidate", "model"]) {
    if (!nonEmptyString(value[field])) {
      throw new Error(`${fixturePath}: fixture ${field} must be a non-empty string`);
    }
  }
  if (decisive) {
    const left = assertEvidence(value.left, `${fixturePath}:left`, true);
    const right = assertEvidence(value.right, `${fixturePath}:right`, true);
    const equal = jsonEqual(left.result, right.result);
    if ((value.outcome === EQUAL) !== equal) {
      throw new Error(`${fixturePath}: declared ${value.outcome} conflicts with frozen results`);
    }
  } else {
    if (!nonEmptyString(value.reason)) {
      throw new Error(`${fixturePath}: indecisive fixture reason must be a non-empty string`);
    }
    if (!isObject(value.probe)) throw new Error(`${fixturePath}: probe must be a JSON object`);
    assertExactFields(value.probe, ["left", "right"], `${fixturePath}:probe`);
    assertEvidence(value.probe.left, `${fixturePath}:probe:left`, false);
    assertEvidence(value.probe.right, `${fixturePath}:probe:right`, false);
  }
  return value;
}

/** Parse non-empty JSONL semantic witness records. */
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

/** Reject corpus drift that removes coverage or turns absence into equality. */
export function assertCorpus(metadata, fixtures) {
  if (fixtures.length === 0) throw new Error("cases.jsonl must contain semantic witnesses");
  const ids = new Set();
  const families = new Set();
  const outcomes = new Set();
  for (const fixture of fixtures) {
    if (!ids.add(fixture.id)) throw new Error(`duplicate fixture id ${JSON.stringify(fixture.id)}`);
    if (!metadata.required_families.includes(fixture.family)) {
      throw new Error(`fixture ${fixture.id} uses undocumented semantic family ${fixture.family}`);
    }
    families.add(fixture.family);
    outcomes.add(fixture.outcome);
  }
  for (const family of CANONICAL_FAMILIES) {
    if (!families.has(family)) {
      throw new Error(`canonical semantic family ${JSON.stringify(family)} needs a witness`);
    }
  }
  for (const outcome of [EQUAL, DIFFERENT, INDECISIVE]) {
    if (!outcomes.has(outcome)) {
      throw new Error(`semantic corpus must retain an explicit ${outcome} outcome`);
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
  return { metadata, fixtures: assertCorpus(metadata, parseFixtures(text, CASES_PATH)) };
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
  if ((fixture.outcome === EQUAL) !== equal) {
    throw new Error(`${fixture.id}: live results conflict with declared ${fixture.outcome} outcome`);
  }
}

async function main() {
  if (process.argv.slice(2).join(" ") !== "--refresh") {
    die(
      "usage: just analysis-semantic-corpus-refresh " +
        "(the hermetic verifier is just analysis-semantic-corpus-verify)",
    );
  }
  let corpus;
  try {
    corpus = await loadCorpus();
  } catch (error) {
    die(`invalid analysis semantic corpus: ${error.message}`);
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
    `verified ${decisiveCount} decisive semantic witnesses against Legend ` +
      `${corpus.metadata.engine_version}; indecisive witnesses remain explicitly unproven`,
  );
}

if (import.meta.main) await main();
