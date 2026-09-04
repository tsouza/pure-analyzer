#!/usr/bin/env bun
// Refresh frozen Legend evidence for pinned canonical-emission fixtures. Rust
// replays each fixture's fixed point hermetically; this tool only contacts
// the pinned oracle (issue #245).

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
  jsonEqual,
  legendEngineBaseUrl,
  requestJson,
} from "./lib/legend-engine.mjs";
import { assertExactFields, isObject, nonEmptyString, oracleLambda } from "./lib/oracle.mjs";

export {
  EngineUnavailableError,
  PinnedEngineError,
  assertEngineVersion,
  canonicalJson,
  jsonEqual,
} from "./lib/legend-engine.mjs";
export { oracleLambda } from "./lib/oracle.mjs";

export const CASES_PATH = `${CORPUS_DIRECTORY}/canonical.jsonl`;

/** Validate one pinned canonical-emission fixture and return it unchanged. */
export function assertFixture(value, path, line) {
  const fixturePath = `${path}:${line}`;
  if (!isObject(value)) throw new Error(`${fixturePath}: fixture must be a JSON object`);
  assertExactFields(value, ["id", "model", "source", "oracle", "lambda", "result"], fixturePath);
  for (const field of ["id", "model", "source", "lambda"]) {
    if (!nonEmptyString(value[field])) {
      throw new Error(`${fixturePath}: fixture ${field} must be a non-empty string`);
    }
  }
  const expectedLambda = oracleLambda(value.oracle, `${fixturePath}:oracle`);
  if (value.lambda !== expectedLambda) {
    throw new Error(`${fixturePath}: lambda must exactly render its bounded oracle`);
  }
  if (!("result" in value)) {
    throw new Error(`${fixturePath}: fixture must carry a frozen result`);
  }
  return value;
}

/** Parse non-empty JSONL canonical-emission fixtures. */
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

/** Reject corpus drift that empties the corpus or duplicates a fixture id. */
export function assertCorpus(fixtures) {
  if (fixtures.length === 0) {
    throw new Error("canonical.jsonl must contain canonical-emission fixtures");
  }
  const ids = new Set();
  for (const fixture of fixtures) {
    if (ids.has(fixture.id)) throw new Error(`duplicate fixture id ${JSON.stringify(fixture.id)}`);
    ids.add(fixture.id);
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

/** Compile `fixture.model` once and execute its bounded `lambda` against it. */
async function executeFixtureLambda(base, metadata, fixture) {
  const model = await requestJson(
    base,
    metadata.model_endpoint,
    "POST",
    fixture.model,
    "text/plain",
    `${fixture.id}: model`,
  );
  const lambda = await requestJson(
    base,
    metadata.lambda_endpoint,
    "POST",
    fixture.lambda,
    "text/plain",
    `${fixture.id}: lambda`,
  );
  return requestJson(
    base,
    metadata.execution_endpoint,
    "POST",
    JSON.stringify({
      model,
      function: lambda,
      mapping: null,
      runtime: null,
      context: { _type: "BaseExecutionContext" },
      parameterValues: [],
    }),
    "application/json",
    `${fixture.id}: execution`,
  );
}

async function verifyFixture(base, metadata, fixture) {
  const observed = await executeFixtureLambda(base, metadata, fixture);
  if (!jsonEqual(observed, fixture.result)) {
    throw new Error(
      `${fixture.id}: bounded-oracle result diverged from frozen evidence\n` +
        `frozen: ${canonicalJson(fixture.result)}\n` +
        `observed: ${canonicalJson(observed)}`,
    );
  }
}

async function main() {
  if (process.argv.slice(2).join(" ") !== "--refresh") {
    die(
      "usage: just analysis-canonical-corpus-refresh " +
        "(the hermetic verifier is just analysis-canonical-corpus-verify)",
    );
  }
  let corpus;
  try {
    corpus = await loadCorpus();
  } catch (error) {
    die(`invalid analysis canonical corpus: ${error.message}`);
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
  notice(
    `verified ${corpus.fixtures.length} canonical-emission fixtures against Legend ` +
      `${corpus.metadata.engine_version}`,
  );
}

if (import.meta.main) await main();
