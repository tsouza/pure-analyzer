import { describe, expect, test } from "bun:test";

import {
  CANONICAL_FAMILIES,
  DIFFERENT,
  EQUAL,
  INDECISIVE,
  PinnedEngineError,
  SCHEMA_VERSION,
  assertCorpus,
  assertEngineVersion,
  assertFixture,
  assertMetadata,
  canonicalJson,
  jsonEqual,
  parseFixtures,
} from "./analysis-semantic-corpus.mjs";

const metadata = {
  schema_version: SCHEMA_VERSION,
  engine_version: "4.113.0",
  model_endpoint: "/api/pure/v1/grammar/grammarToJson/model",
  lambda_endpoint: "/api/pure/v1/grammar/grammarToJson/lambda",
  execution_endpoint: "/api/pure/v1/execution/execute",
  required_families: [...CANONICAL_FAMILIES],
};

function decisiveFixture(id, family, outcome, leftResult, rightResult) {
  return {
    id,
    family,
    candidate: "test-candidate",
    model: "Class test::Row {}",
    left: { lambda: "|1", result: leftResult },
    right: { lambda: "|2", result: rightResult },
    outcome,
  };
}

function indecisiveFixture() {
  return {
    id: "indecisive",
    family: CANONICAL_FAMILIES[2],
    candidate: "test-candidate",
    model: "Class test::Row {}",
    probe: { left: { lambda: "|1" }, right: { lambda: "|true" } },
    outcome: INDECISIVE,
    reason: "No result is available.",
  };
}

describe("analysis semantic corpus validation", () => {
  test("requires exact metadata pins, endpoints, and canonical semantic classes", () => {
    expect(assertMetadata(metadata)).toEqual(metadata);
    expect(() => assertMetadata({ ...metadata, engine_version: "latest" })).toThrow(
      "exact x.y.z pin",
    );
    expect(() => assertMetadata({ ...metadata, model_endpoint: "https://other.example" })).toThrow(
      "absolute API path",
    );
    expect(() => assertMetadata({ ...metadata, required_families: CANONICAL_FAMILIES.slice(1) })).toThrow(
      "canonical semantic classes",
    );
  });

  test("accepts only results consistent with their declared semantic relationship", () => {
    const equal = decisiveFixture("equal", CANONICAL_FAMILIES[0], EQUAL, [1, 2], [1, 2]);
    const different = decisiveFixture("different", CANONICAL_FAMILIES[1], DIFFERENT, [1, 2], [2]);
    expect(assertFixture(equal, "cases.jsonl", 1)).toEqual(equal);
    expect(assertFixture(different, "cases.jsonl", 2)).toEqual(different);
    expect(() =>
      assertFixture({ ...equal, right: { ...equal.right, result: [2] } }, "cases.jsonl", 3),
    ).toThrow("declared equal conflicts");
    expect(() =>
      assertFixture({ ...different, right: { ...different.right, result: [1, 2] } }, "cases.jsonl", 4),
    ).toThrow("declared different conflicts");
  });

  test("keeps absent evidence explicitly indecisive and result-free", () => {
    const fixture = indecisiveFixture();
    expect(assertFixture(fixture, "cases.jsonl", 1)).toEqual(fixture);
    expect(() =>
      assertFixture(
        { ...fixture, probe: { ...fixture.probe, left: { lambda: "|1", result: true } } },
        "cases.jsonl",
        2,
      ),
    ).toThrow("unexpected corpus fields");
  });

  test("requires every semantic class and every outcome, including indecisive", () => {
    const fixtures = [
      decisiveFixture("order", CANONICAL_FAMILIES[0], DIFFERENT, [1], [2]),
      decisiveFixture("bag", CANONICAL_FAMILIES[1], EQUAL, [1], [1]),
      indecisiveFixture(),
    ];
    expect(assertCorpus(metadata, fixtures)).toEqual(fixtures);
    expect(() => assertCorpus(metadata, fixtures.slice(0, 2))).toThrow("three-valued-logic");
  });

  test("parses JSONL records with line-specific errors", () => {
    const fixture = decisiveFixture("one", CANONICAL_FAMILIES[0], DIFFERENT, 1, 2);
    expect(parseFixtures(`${JSON.stringify(fixture)}\n`, "cases.jsonl")).toEqual([fixture]);
    expect(() => parseFixtures("{not-json}\n", "cases.jsonl")).toThrow("cases.jsonl:1: invalid JSON");
  });

  test("compares JSON structurally and refuses version drift", () => {
    expect(jsonEqual({ second: [2], first: 1 }, { first: 1, second: [2] })).toBe(true);
    expect(canonicalJson({ second: 2, first: 1 })).toBe('{"first":1,"second":2}');
    expect(assertEngineVersion("4.113.0", metadata.engine_version)).toBe("4.113.0");
    expect(() => assertEngineVersion("4.114.0", metadata.engine_version)).toThrow(PinnedEngineError);
  });
});
