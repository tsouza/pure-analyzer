#!/usr/bin/env bun
// Refresh the analyzer-owned, frozen Legend grammar corpus. Ordinary parser
// verification never executes this script or needs a Legend engine.

import { mkdir } from "node:fs/promises";

import { die, notice } from "./lib/ci.mjs";

export const CORPUS_DIRECTORY = "crates/pure-analyzer-parser/corpus/legend-4.113.0";
export const METADATA_PATH = `${CORPUS_DIRECTORY}/metadata.json`;
export const ACCEPT_PATH = `${CORPUS_DIRECTORY}/accept.jsonl`;
export const REJECT_PATH = `${CORPUS_DIRECTORY}/reject.jsonl`;
export const CACHE_DIRECTORY = "target/parser-differential";
export const CACHE_SCHEMA_VERSION = 1;
export const PARSE_OK = "parse_ok";
export const PARSE_FAIL = "parse_fail";
export const INFO_ENDPOINT = "/api/server/v1/info";
export const HTTP_OK = 200;
export const HTTP_PARSER_ERROR = 400;
export const REQUEST_TIMEOUT_MS = 8_000;
// Required grammar-boundary classes. This list is deliberately code-owned
// rather than corpus-owned so a corpus-only edit cannot silently remove a
// legal-neighbor coverage guarantee.
export const CANONICAL_FAMILIES = Object.freeze([
  "bare-relation-column",
  "zero-argument-navigation",
  "date-navigation",
  "generated-navigation",
]);

/** A connection or timeout prevented an engine request from completing. */
export class EngineUnavailableError extends Error {}

function nonEmptyString(value) {
  return typeof value === "string" && value.trim().length > 0;
}

/** Validate and return version-pinned corpus metadata. */
export function assertMetadata(value) {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    throw new Error("metadata must be a JSON object");
  }
  if (value.schema_version !== CACHE_SCHEMA_VERSION) {
    throw new Error(`unsupported metadata schema version ${value.schema_version}`);
  }
  if (!/^\d+\.\d+\.\d+$/.test(value.engine_version ?? "")) {
    throw new Error("metadata engine_version must be an exact x.y.z pin");
  }
  if (!/^\/api\/.+/.test(value.grammar_endpoint ?? "")) {
    throw new Error("metadata grammar_endpoint must be an absolute API path");
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
    throw new Error("metadata required_families must exactly list the canonical grammar classes");
  }
  for (const field of ["provenance", "update_policy"]) {
    if (!nonEmptyString(value[field])) {
      throw new Error(`metadata ${field} must be non-empty`);
    }
  }
  return value;
}

/** Validate one corpus record and return its normalized representation. */
export function assertFixture(value, expectedLegend, path, line) {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    throw new Error(`${path}:${line}: fixture must be a JSON object`);
  }
  for (const field of ["id", "query", "endpoint", "family", "provenance"]) {
    if (!nonEmptyString(value[field])) {
      throw new Error(`${path}:${line}: fixture ${field} must be non-empty`);
    }
  }
  if (value.legend !== expectedLegend) {
    throw new Error(
      `${path}:${line}: expected ${expectedLegend} record, got ${JSON.stringify(value.legend)}`,
    );
  }
  return value;
}

/** Parse non-empty JSONL records from one verdict-specific corpus file. */
export function parseFixtures(text, expectedLegend, path) {
  return text
    .split("\n")
    .map((line, index) => ({ line, number: index + 1 }))
    .filter(({ line }) => line.trim().length > 0)
    .map(({ line, number }) => {
      try {
        return assertFixture(JSON.parse(line), expectedLegend, path, number);
      } catch (error) {
        if (error instanceof SyntaxError) {
          throw new Error(`${path}:${number}: invalid JSON: ${error.message}`);
        }
        throw error;
      }
    });
}

/** Require a legal parse neighbor for every documented grammar-boundary class. */
export function assertAcceptedFamilyCoverage(fixtures) {
  const acceptedFamilies = new Set(fixtures.map((fixture) => fixture.family));
  for (const family of CANONICAL_FAMILIES) {
    if (!acceptedFamilies.has(family)) {
      throw new Error(
        `canonical grammar family ${JSON.stringify(family)} must have a parse_ok legal-neighbor fixture`,
      );
    }
  }
}

/** Convert the Legend grammar endpoint's parser status into a frozen verdict. */
export function verdictFromStatus(status) {
  if (status === HTTP_OK) return PARSE_OK;
  if (status === HTTP_PARSER_ERROR) return PARSE_FAIL;
  throw new Error(`unexpected Legend grammar status ${status}`);
}

/** Return the cache path dedicated to one immutable engine version. */
export function cachePath(engineVersion) {
  return `${CACHE_DIRECTORY}/legend-${engineVersion}.json`;
}

/** Return fixtures whose live or cached verdict disagrees with the frozen corpus. */
export function differences(fixtures, observations) {
  const observedById = new Map(observations.map((observation) => [observation.id, observation]));
  return fixtures.flatMap((fixture) => {
    const observed = observedById.get(fixture.id);
    if (observed?.verdict === fixture.legend) return [];
    return [{ fixture, observed: observed?.verdict ?? "missing" }];
  });
}

/** Return whether a cache is complete and bound to the exact frozen input. */
export function cacheMatches(cache, metadata, fixtures) {
  if (
    !cache ||
    cache.schema_version !== CACHE_SCHEMA_VERSION ||
    cache.engine_version !== metadata.engine_version ||
    cache.grammar_endpoint !== metadata.grammar_endpoint ||
    !Array.isArray(cache.observations) ||
    cache.observations.length !== fixtures.length
  ) {
    return false;
  }
  const fixtureById = new Map(fixtures.map((fixture) => [fixture.id, fixture]));
  const seen = new Set();
  return cache.observations.every((observation) => {
    const fixture = fixtureById.get(observation?.id);
    if (!fixture || seen.has(fixture.id)) return false;
    seen.add(fixture.id);
    return (
      observation.query === fixture.query &&
      observation.endpoint === fixture.endpoint &&
      observation.family === fixture.family &&
      observation.verdict === fixture.legend
    );
  });
}

/** Return whether an unavailable engine may be replaced by an exact cache. */
export function canUseCacheFallback(error, cache, metadata, fixtures) {
  return error instanceof EngineUnavailableError && cacheMatches(cache, metadata, fixtures);
}

function baseUrl() {
  const raw = process.env.LEGEND_ENGINE_URL ?? "http://localhost:6300";
  let url;
  try {
    url = new URL(raw);
  } catch (error) {
    throw new Error(`invalid LEGEND_ENGINE_URL ${JSON.stringify(raw)}: ${error.message}`);
  }
  if (url.protocol !== "http:" && url.protocol !== "https:") {
    throw new Error("LEGEND_ENGINE_URL must use http or https");
  }
  return raw.replace(/\/+$/, "");
}

function endpointUrl(base, endpoint) {
  return `${base}${endpoint}`;
}

async function readJson(path) {
  try {
    return JSON.parse(await Bun.file(path).text());
  } catch (error) {
    throw new Error(`could not read ${path}: ${error.message}`);
  }
}

async function optionalJson(path) {
  const file = Bun.file(path);
  if (!(await file.exists())) return null;
  return readJson(path);
}

async function loadCorpus() {
  const metadata = assertMetadata(await readJson(METADATA_PATH));
  const accept = parseFixtures(await Bun.file(ACCEPT_PATH).text(), PARSE_OK, ACCEPT_PATH);
  const reject = parseFixtures(await Bun.file(REJECT_PATH).text(), PARSE_FAIL, REJECT_PATH);
  if (accept.length === 0 || reject.length === 0) {
    throw new Error("both accept.jsonl and reject.jsonl must contain at least one fixture");
  }
  assertAcceptedFamilyCoverage(accept);

  const fixtures = [...accept, ...reject];
  const ids = new Set();
  for (const fixture of fixtures) {
    if (!ids.add(fixture.id)) {
      throw new Error(`duplicate fixture id ${JSON.stringify(fixture.id)}`);
    }
    if (fixture.endpoint !== metadata.grammar_endpoint) {
      throw new Error(
        `fixture ${fixture.id} uses ${fixture.endpoint}, not pinned ${metadata.grammar_endpoint}`,
      );
    }
    if (!metadata.required_families.includes(fixture.family)) {
      throw new Error(`fixture ${fixture.id} uses undocumented grammar family ${fixture.family}`);
    }
  }
  return { metadata, fixtures };
}

class PinnedEngineError extends Error {}

/** Reject an engine that is absent from, or differs from, the immutable pin. */
export function assertEngineVersion(version, expectedVersion) {
  if (!nonEmptyString(version)) {
    throw new Error("Legend engine info did not contain git.build.version");
  }
  if (version !== expectedVersion) {
    throw new PinnedEngineError(
      `Legend engine version ${version} does not match pinned ${expectedVersion}. ` +
        "The frozen corpus is immutable; create a new versioned corpus for a deliberate re-pin.",
    );
  }
  return version;
}

async function checkedEngineVersion(base, expectedVersion) {
  let response;
  try {
    response = await fetch(endpointUrl(base, INFO_ENDPOINT), {
      signal: AbortSignal.timeout(REQUEST_TIMEOUT_MS),
    });
  } catch (error) {
    throw new EngineUnavailableError(`could not read Legend engine info: ${error.message}`);
  }
  if (!response.ok) {
    throw new Error(`Legend engine info returned HTTP ${response.status}`);
  }
  let info;
  try {
    info = await response.json();
  } catch (error) {
    throw new Error(`Legend engine info was not JSON: ${error.message}`);
  }
  const version = info?.info?.legendSDLC?.["git.build.version"];
  assertEngineVersion(version, expectedVersion);
}

async function liveObservations(base, fixtures) {
  const observations = [];
  for (const fixture of fixtures) {
    let response;
    try {
      response = await fetch(endpointUrl(base, fixture.endpoint), {
        method: "POST",
        headers: { "content-type": "text/plain" },
        body: fixture.query,
        signal: AbortSignal.timeout(REQUEST_TIMEOUT_MS),
      });
    } catch (error) {
      throw new EngineUnavailableError(
        `fixture ${fixture.id}: grammar request failed: ${error.message}`,
      );
    }
    let verdict;
    try {
      verdict = verdictFromStatus(response.status);
    } catch (error) {
      throw new Error(`fixture ${fixture.id}: ${error.message}`);
    }
    observations.push({
      id: fixture.id,
      query: fixture.query,
      endpoint: fixture.endpoint,
      family: fixture.family,
      verdict,
    });
  }
  return observations;
}

async function writeCache(metadata, observations) {
  await mkdir(CACHE_DIRECTORY, { recursive: true });
  const cache = {
    schema_version: CACHE_SCHEMA_VERSION,
    engine_version: metadata.engine_version,
    grammar_endpoint: metadata.grammar_endpoint,
    observations,
  };
  await Bun.write(cachePath(metadata.engine_version), `${JSON.stringify(cache, null, 2)}\n`);
}

function formatDifferences(rows) {
  return rows
    .map(
      ({ fixture, observed }) =>
        `  - ${fixture.id}: frozen ${fixture.legend}, observed ${observed}\n    ${fixture.query}`,
    )
    .join("\n");
}

async function main() {
  if (!process.argv.slice(2).includes("--refresh")) {
    die("usage: just parser-differential-refresh (the hermetic verifier is just parser-differential-verify)");
  }

  let corpus;
  try {
    corpus = await loadCorpus();
  } catch (error) {
    die(`invalid parser differential corpus: ${error.message}`);
  }
  const { metadata, fixtures } = corpus;
  let cache = null;
  try {
    cache = await optionalJson(cachePath(metadata.engine_version));
  } catch (error) {
    notice(`ignoring unreadable parser differential cache: ${error.message}`);
  }
  let observations;
  let cacheFallback = false;
  try {
    const base = baseUrl();
    await checkedEngineVersion(base, metadata.engine_version);
    observations = await liveObservations(base, fixtures);
  } catch (error) {
    if (error instanceof PinnedEngineError) die(error.message);
    if (!canUseCacheFallback(error, cache, metadata, fixtures)) {
      if (!(error instanceof EngineUnavailableError)) die(error.message);
      die(
        `${error.message}\nNo complete cache for Legend ${metadata.engine_version}; ` +
          "start the exact pinned engine before refreshing.",
      );
    }
    cacheFallback = true;
    observations = cache.observations;
    notice(`Legend engine unavailable; replaying complete cached ${metadata.engine_version} observations`);
  }

  const changed = differences(fixtures, observations);
  if (changed.length > 0) {
    die(
      `Legend grammar verdicts diverged from the immutable ${metadata.engine_version} corpus:\n` +
        `${formatDifferences(changed)}\n` +
        "Do not relabel this directory. Triage the minimal cases and create a new versioned corpus " +
        "for a deliberate engine re-pin.",
    );
  }
  if (!cacheFallback) await writeCache(metadata, observations);
  notice(
    `verified ${fixtures.length} parser differential cases against Legend ${metadata.engine_version}` +
      (cacheFallback ? " (cached)" : ""),
  );
}

if (import.meta.main) await main();
