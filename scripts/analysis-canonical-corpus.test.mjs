import { describe, expect, test } from "bun:test";

import {
  assertCorpus,
  assertFixture,
  oracleLambda,
  parseFixtures,
} from "./analysis-canonical-corpus.mjs";

function fixture(id, source, oracle, result) {
  return {
    id,
    model: "Class test::Row {}",
    source,
    oracle,
    lambda: oracleLambda(oracle),
    result,
  };
}

describe("analysis canonical corpus validation", () => {
  test("requires the exact fixture schema and a lambda matching its oracle", () => {
    const scan = fixture("bare-scan", "test::Row.all()", { kind: "scan", values: [1, 2] }, [1, 2]);
    expect(assertFixture(scan, "canonical.jsonl", 1)).toEqual(scan);
    expect(() => assertFixture({ ...scan, extra: true }, "canonical.jsonl", 2)).toThrow(
      "unexpected corpus fields",
    );
    expect(() =>
      assertFixture({ ...scan, lambda: "|[9]" }, "canonical.jsonl", 3),
    ).toThrow("exactly render its bounded oracle");
    expect(() => assertFixture({ ...scan, source: "" }, "canonical.jsonl", 4)).toThrow(
      "must be a non-empty string",
    );
  });

  test("accepts a scalar bounded-oracle result alongside a list result", () => {
    const singleColumn = fixture(
      "map-shaped-single-value",
      "test::Row.all()->map(v0| $v0.name)",
      { kind: "ordered_columns", columns: ["value"] },
      "value",
    );
    expect(assertFixture(singleColumn, "canonical.jsonl", 1)).toEqual(singleColumn);
  });

  test("rejects an empty corpus or a duplicate fixture id", () => {
    const first = fixture("a", "test::Row.all()", { kind: "scan", values: [1] }, [1]);
    const duplicate = fixture("a", "test::Row.all()", { kind: "scan", values: [2] }, [2]);
    expect(assertCorpus([first])).toEqual([first]);
    expect(() => assertCorpus([])).toThrow("must contain canonical-emission fixtures");
    expect(() => assertCorpus([first, duplicate])).toThrow("duplicate fixture id");
  });

  test("parses non-empty JSONL lines and reports line-anchored JSON errors", () => {
    const scan = fixture("bare-scan", "test::Row.all()", { kind: "scan", values: [1] }, [1]);
    expect(parseFixtures(`${JSON.stringify(scan)}\n`, "canonical.jsonl")).toEqual([scan]);
    expect(() => parseFixtures("{not-json}\n", "canonical.jsonl")).toThrow(
      "canonical.jsonl:1: invalid JSON",
    );
  });
});
