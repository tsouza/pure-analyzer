import { describe, expect, spyOn, test } from "bun:test";

import { die, error, notice } from "./ci.mjs";

// `inCI` inside ci.mjs is captured ONCE at module load (`Boolean(process.env.
// GITHUB_ACTIONS)`), not re-read per call — so these tests exercise whichever
// mode the test runner's own environment loaded the module under, rather
// than toggling it mid-run (setting the env var after import would not
// change the module's already-captured constant).
const inCI = Boolean(process.env.GITHUB_ACTIONS);
const prefix = (msg) => (inCI ? `::error::${msg}` : `✖ ${msg}`);
const noticePrefix = (msg) => (inCI ? `::notice::${msg}` : msg);

describe("error", () => {
  test("annotates a message and never exits", () => {
    const errSpy = spyOn(console, "error").mockImplementation(() => {});
    const exitSpy = spyOn(process, "exit").mockImplementation(() => {
      throw new Error("error() must not exit");
    });
    expect(() => error("boom")).not.toThrow();
    expect(errSpy).toHaveBeenCalledWith(prefix("boom"));
    exitSpy.mockRestore();
    errSpy.mockRestore();
  });
});

describe("notice", () => {
  test("annotates an informational message", () => {
    const spy = spyOn(console, "error").mockImplementation(() => {});
    notice("fyi");
    expect(spy).toHaveBeenCalledWith(noticePrefix("fyi"));
    spy.mockRestore();
  });
});

describe("die", () => {
  test("emits the same annotation error() does, then exits with the given code", () => {
    const errSpy = spyOn(console, "error").mockImplementation(() => {});
    const exitSpy = spyOn(process, "exit").mockImplementation(() => {});
    die("fatal", { code: 3 });
    expect(errSpy).toHaveBeenCalledWith(prefix("fatal"));
    expect(exitSpy).toHaveBeenCalledWith(3);
    exitSpy.mockRestore();
    errSpy.mockRestore();
  });

  test("defaults to exit code 1", () => {
    const errSpy = spyOn(console, "error").mockImplementation(() => {});
    const exitSpy = spyOn(process, "exit").mockImplementation(() => {});
    die("fatal");
    expect(exitSpy).toHaveBeenCalledWith(1);
    exitSpy.mockRestore();
    errSpy.mockRestore();
  });
});
