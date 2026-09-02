import { describe, expect, test } from "bun:test";

import {
  EQUIVALENT,
  INDECISIVE,
  INDECISIVE_REASON,
  NOT_EQUIVALENT,
  OUTPUT_COLUMN_FIELDS,
  assertCorpus,
  assertFixture,
  oracleLambda,
  parseFixtures,
} from "./analysis-comparison-corpus.mjs";

function side(source, oracle, result) {
  const evidence = { source, oracle, lambda: oracleLambda(oracle) };
  if (result !== undefined) evidence.result = result;
  return evidence;
}

function fixture(id, outcome, leftResult, rightResult) {
  const left = outcome === EQUIVALENT
    ? side("test::Row.all()->filter(row| true)", { kind: "filter_true", values: [1] }, leftResult)
    : outcome === NOT_EQUIVALENT
      ? side(
        "test::Row.all()->project(~[name: row | $row.name])",
        { kind: "ordered_columns", columns: ["name"] },
        leftResult,
      )
      : side(
        "test::Row.all()->filter(row| $row.name == 'Ada')",
        { kind: "literal_filter", values: ["Ada"], value: "Ada" },
        leftResult,
      );
  const right = outcome === EQUIVALENT
    ? side("test::Row.all()", { kind: "scan", values: [1] }, rightResult)
    : outcome === NOT_EQUIVALENT
      ? side(
        "test::Row.all()->project(~[email: row | $row.email])",
        { kind: "ordered_columns", columns: ["email"] },
        rightResult,
      )
      : side(
        "test::Row.all()->filter(row| $row.name == 'Grace')",
        { kind: "literal_filter", values: ["Grace"], value: "Grace" },
        rightResult,
      );
  const value = {
    id,
    model: "Class test::Row {}",
    left,
    right,
    outcome,
  };
  if (outcome === NOT_EQUIVALENT) {
    value.difference = { kind: "output_column", index: 0, field: OUTPUT_COLUMN_FIELDS[0] };
  }
  if (outcome === INDECISIVE) value.reason = INDECISIVE_REASON;
  return value;
}

describe("analysis comparison corpus validation", () => {
  test("requires exact conditional schemas and source/lambda evidence", () => {
    const equivalent = fixture("equivalent", EQUIVALENT, [1], [1]);
    const refuted = fixture("refuted", NOT_EQUIVALENT, [1], [2]);
    const indecisive = fixture("indecisive", INDECISIVE);
    expect(assertFixture(equivalent, "comparison.jsonl", 1)).toEqual(equivalent);
    expect(assertFixture(refuted, "comparison.jsonl", 2)).toEqual(refuted);
    expect(assertFixture(indecisive, "comparison.jsonl", 3)).toEqual(indecisive);
    expect(() =>
      assertFixture({ ...equivalent, extra: true }, "comparison.jsonl", 4),
    ).toThrow("unexpected corpus fields");
    expect(() =>
      assertFixture(
        { ...refuted, difference: { ...refuted.difference, index: -1 } },
        "comparison.jsonl",
        5,
      ),
    ).toThrow("non-negative integer");
    expect(() =>
      assertFixture(
        { ...equivalent, left: { ...equivalent.left, lambda: "|[1]" } },
        "comparison.jsonl",
        6,
      ),
    ).toThrow("exactly render its bounded oracle");
  });

  test("rejects frozen evidence that contradicts a committed M4a outcome", () => {
    const equivalent = fixture("equivalent", EQUIVALENT, [1], [1]);
    const refuted = fixture("refuted", NOT_EQUIVALENT, [1], [2]);
    expect(() =>
      assertFixture(
        {
          ...equivalent,
          right: side("test::Row.all()", { kind: "scan", values: [2] }, [2]),
        },
        "comparison.jsonl",
        1,
      ),
    ).toThrow("declared equivalent conflicts");
    expect(() =>
      assertFixture(
        {
          ...refuted,
          right: side("test::Row.all()", { kind: "scan", values: [1] }, [1]),
        },
        "comparison.jsonl",
        2,
      ),
    ).toThrow("declared not_equivalent conflicts");
  });

  test("keeps indecisive evidence result-free and pins its typed boundary", () => {
    const undecided = fixture("indecisive", INDECISIVE);
    expect(() =>
      assertFixture(
        {
          ...undecided,
          left: side("test::Row.all()", { kind: "scan", values: [1] }, [1]),
        },
        "comparison.jsonl",
        1,
      ),
    ).toThrow("unexpected corpus fields");
    expect(() =>
      assertFixture({ ...undecided, reason: "IND_UNMODELED_OP" }, "comparison.jsonl", 2),
    ).toThrow(INDECISIVE_REASON);
  });

  test("requires all three M4a outcome classes and reports JSONL line failures", () => {
    const fixtures = [
      fixture("equivalent", EQUIVALENT, [1], [1]),
      fixture("refuted", NOT_EQUIVALENT, [1], [2]),
      fixture("indecisive", INDECISIVE),
    ];
    expect(assertCorpus(fixtures)).toEqual(fixtures);
    expect(() => assertCorpus(fixtures.slice(0, 2))).toThrow(INDECISIVE);
    expect(parseFixtures(`${JSON.stringify(fixtures[0])}\n`, "comparison.jsonl")).toEqual([
      fixtures[0],
    ]);
    expect(() => parseFixtures("{not-json}\n", "comparison.jsonl")).toThrow(
      "comparison.jsonl:1: invalid JSON",
    );
  });
});
