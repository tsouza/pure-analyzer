import { describe, expect, test } from "bun:test";

import {
  CACHE_SCHEMA_VERSION,
  CANONICAL_FAMILIES,
  EngineUnavailableError,
  PARSE_FAIL,
  PARSE_OK,
  assertEngineVersion,
  assertAcceptedFamilyCoverage,
  assertMetadata,
  canUseCacheFallback,
  cacheMatches,
  differences,
  parseFixtures,
  verdictFromStatus,
} from "./parser-differential.mjs";

const metadata = {
  schema_version: CACHE_SCHEMA_VERSION,
  engine_version: "4.113.0",
  grammar_endpoint: "/api/pure/v1/grammar/grammarToJson/lambda",
  required_families: [...CANONICAL_FAMILIES],
  provenance: "test",
  update_policy: "test",
};

const fixtures = [
  {
    id: "accept",
    query: "model::Person.all()",
    legend: PARSE_OK,
    endpoint: metadata.grammar_endpoint,
    family: CANONICAL_FAMILIES[0],
    provenance: "test",
  },
  {
    id: "reject",
    query: "model::Person.all(",
    legend: PARSE_FAIL,
    endpoint: metadata.grammar_endpoint,
    family: CANONICAL_FAMILIES[0],
    provenance: "test",
  },
];

describe("parser differential corpus validation", () => {
  test("requires an exact engine pin and endpoint", () => {
    expect(assertMetadata(metadata)).toEqual(metadata);
    expect(() => assertMetadata({ ...metadata, engine_version: "latest" })).toThrow(
      "exact x.y.z pin",
    );
    expect(() => assertMetadata({ ...metadata, grammar_endpoint: "https://other.example" })).toThrow(
      "absolute API path",
    );
    expect(() => assertMetadata({ ...metadata, required_families: CANONICAL_FAMILIES.slice(1) })).toThrow(
      "canonical grammar classes",
    );
  });

  test("requires a legal parse neighbor for every canonical grammar class", () => {
    const legalNeighbors = CANONICAL_FAMILIES.map((family) => ({ family }));
    expect(() => assertAcceptedFamilyCoverage(legalNeighbors)).not.toThrow();
    expect(() => assertAcceptedFamilyCoverage(legalNeighbors.slice(1))).toThrow(
      "parse_ok legal-neighbor fixture",
    );
  });

  test("refuses an unpinned engine instead of relabeling the frozen corpus", () => {
    expect(assertEngineVersion("4.113.0", metadata.engine_version)).toBe("4.113.0");
    expect(() => assertEngineVersion("4.114.0", metadata.engine_version)).toThrow(
      "does not match pinned",
    );
  });

  test("binds each JSONL file to its declared verdict", () => {
    const text = `${JSON.stringify(fixtures[0])}\n`;
    expect(parseFixtures(text, PARSE_OK, "accept.jsonl")).toEqual([fixtures[0]]);
    expect(() => parseFixtures(text, PARSE_FAIL, "reject.jsonl")).toThrow("expected parse_fail");
  });

  test("maps only the grammar endpoint's parser responses", () => {
    expect(verdictFromStatus(200)).toBe(PARSE_OK);
    expect(verdictFromStatus(400)).toBe(PARSE_FAIL);
    expect(() => verdictFromStatus(500)).toThrow("unexpected Legend grammar status 500");
  });

  test("accepts only a complete cache bound to the frozen rows", () => {
    const cache = {
      schema_version: CACHE_SCHEMA_VERSION,
      engine_version: metadata.engine_version,
      grammar_endpoint: metadata.grammar_endpoint,
      observations: fixtures.map((fixture) => ({
        id: fixture.id,
        query: fixture.query,
        endpoint: fixture.endpoint,
        family: fixture.family,
        verdict: fixture.legend,
      })),
    };
    expect(cacheMatches(cache, metadata, fixtures)).toBe(true);
    expect(
      cacheMatches(
        { ...cache, observations: cache.observations.slice(0, 1) },
        metadata,
        fixtures,
      ),
    ).toBe(false);
    expect(
      cacheMatches(
        {
          ...cache,
          observations: [{ ...cache.observations[0], verdict: PARSE_FAIL }, cache.observations[1]],
        },
        metadata,
        fixtures,
      ),
    ).toBe(false);
  });

  test("falls back only when an exact engine is unavailable", () => {
    const cache = {
      schema_version: CACHE_SCHEMA_VERSION,
      engine_version: metadata.engine_version,
      grammar_endpoint: metadata.grammar_endpoint,
      observations: fixtures.map((fixture) => ({
        id: fixture.id,
        query: fixture.query,
        endpoint: fixture.endpoint,
        family: fixture.family,
        verdict: fixture.legend,
      })),
    };

    expect(
      canUseCacheFallback(new EngineUnavailableError("connection refused"), cache, metadata, fixtures),
    ).toBe(true);
    expect(
      canUseCacheFallback(
        new Error("Legend engine info returned HTTP 503"),
        cache,
        metadata,
        fixtures,
      ),
    ).toBe(false);
    expect(
      canUseCacheFallback(
        new Error("fixture accept: unexpected Legend grammar status 500"),
        cache,
        metadata,
        fixtures,
      ),
    ).toBe(false);
  });

  test("reports the smallest diverging fixture", () => {
    expect(
      differences(fixtures, [
        { id: "accept", verdict: PARSE_OK },
        { id: "reject", verdict: PARSE_OK },
      ]),
    ).toEqual([{ fixture: fixtures[1], observed: PARSE_OK }]);
  });
});
