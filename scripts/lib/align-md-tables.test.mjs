import { describe, expect, test } from "bun:test";

import { alignTable } from "./align-md-tables.mjs";

describe("alignTable", () => {
  test("aligns emoji by display width", () => {
    expect(alignTable([
      "| Status | Next |\n",
      "| --- | --- |\n",
      "| ✅ | go |\n",
      "| plain | stop |\n",
    ])).toEqual([
      "| Status | Next |\n",
      "| ------ | ---- |\n",
      "| ✅     | go   |\n",
      "| plain  | stop |\n",
    ]);
  });

  test("aligns East Asian wide characters", () => {
    expect(alignTable([
      "| Name | Value |\n",
      "| --- | --- |\n",
      "| 表 | ok |\n",
      "| plain | longer |\n",
    ])).toEqual([
      "| Name  | Value  |\n",
      "| ----- | ------ |\n",
      "| 表    | ok     |\n",
      "| plain | longer |\n",
    ]);
  });

  test("preserves separator alignment markers", () => {
    expect(alignTable([
      "| Left | Right |\n",
      "| :--- | ---: |\n",
      "| a | b |\n",
    ])).toEqual([
      "| Left | Right |\n",
      "| :--- | ----: |\n",
      "| a    | b     |\n",
    ]);
  });

  test("leaves malformed tables unchanged", () => {
    const lines = ["| one | two |\n", "| only-one |\n"];
    expect(alignTable(lines)).toEqual(lines);
  });
});
