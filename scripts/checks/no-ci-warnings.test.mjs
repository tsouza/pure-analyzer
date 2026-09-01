// Regression + behavior tests for the CI log-sweep's warning detection.
// Run with Bun: `bun test scripts/`.
//
// The load-bearing case guards a fixed flake: `@actions/cache` emits a
// `##[warning]You've hit a rate limit…` annotation from its HTTP retry logic
// when concurrent jobs race to reserve the same shared cache key. That
// transient, non-fatal annotation once failed the sweep (constitution §3:
// flakes are bugs). It is now allowlisted — but ONLY that exact wording, so a
// genuine tool warning is still caught (constitution §2: never weaken a gate).
import { expect, test, describe } from "bun:test";
import {
  emptySweepState,
  findWarnings,
  isTransientLogArchiveFailure,
  scannableJobs,
  ALLOWLIST,
} from "./no-ci-warnings.mjs";

// A real CI log line carries a leading RFC3339 timestamp, matching what
// `gh api …/logs` returns and what the sweep feeds `findWarnings`.
const ts = "2026-07-09T07:30:53.2034204Z";
const line = (body) => `${ts} ${body}`;

describe("findWarnings — benign allowlisted matches", () => {
  test("the actions/cache rate-limit retry annotation is NOT an offender (regression)", () => {
    const log = [
      line("##[group]Run actions/cache@v6"),
      line("##[warning]You've hit a rate limit, your rate limit will reset in 7 seconds"),
      line("Failed to save: Unable to reserve cache with key v0-rust-structural-Linux-x64-abc, another job may be creating this cache."),
    ].join("\n");

    expect(findWarnings(log)).toEqual([]);
  });

  test("the pure-analyzer-analysis Severity::Warning enum variant is NOT an offender (regression)", () => {
    // `Severity::Warning => 1,` (crates/pure-analyzer-analysis/src/pass.rs) is a
    // Rust enum path, not a GitHub Actions `::warning` annotation — but WARNING's
    // `::warning` branch matches it case-insensitively regardless.
    const log = line("        Severity::Warning => 1,");
    expect(findWarnings(log)).toEqual([]);
  });

  test("every ALLOWLIST entry is a documented, non-empty exception", () => {
    for (const entry of ALLOWLIST) {
      expect(entry.re).toBeInstanceOf(RegExp);
      expect(typeof entry.why).toBe("string");
      expect(entry.why.length).toBeGreaterThan(0);
    }
  });
});

describe("findWarnings — real warnings still fail (no weakening)", () => {
  test("a rustc-style `warning:` line is an offender", () => {
    const log = line("warning: unused variable: `x`");
    expect(findWarnings(log)).toEqual([log]);
  });

  test.each([
    "warning[E0602]: unknown lint",
    "warning[license-not-encountered]: license was not encountered",
    "warning[duplicate]: found 2 duplicate entries for crate 'hashbrown'",
  ])("a coded `warning[<code>]:` line is an offender: %p", (body) => {
    // rustc and cargo-deny print the diagnostic code in brackets *before* the
    // colon (`warning[E0602]:`, `warning[license-not-encountered]:`). A regex
    // requiring the colon immediately after "warning" let these slip the net;
    // the code form must be caught too (constitution §2).
    const log = line(body);
    expect(findWarnings(log)).toEqual([log]);
  });

  test("a GitHub Actions `##[warning]` annotation (non-allowlisted) is an offender", () => {
    const log = line("##[warning]Node.js 16 actions are deprecated");
    expect(findWarnings(log)).toEqual([log]);
  });

  test("a raw `::warning` workflow command is an offender", () => {
    const log = line("::warning::deprecated input used");
    expect(findWarnings(log)).toEqual([log]);
  });

  test("a different rate-limit warning (not the toolkit wording) is NOT allowlisted", () => {
    // Guards the narrowness of the allowlist: only the exact retry annotation is
    // benign; an API rate-limit surfaced as a bare warning must still fail.
    const log = line("warning: GitHub API rate limit exceeded for user");
    expect(findWarnings(log)).toEqual([log]);
  });

  test("real offenders survive alongside an allowlisted line", () => {
    const log = [
      line("##[warning]You've hit a rate limit, your rate limit will reset in 3 seconds"),
      line("warning: field is never read: `y`"),
    ].join("\n");
    expect(findWarnings(log)).toEqual([line("warning: field is never read: `y`")]);
  });
});

describe("scannableJobs — only real runner jobs are swept", () => {
  const real = { name: "docs", conclusion: "success", steps: [{}, {}] };
  const failed = { name: "check", conclusion: "failure", steps: [{}] };

  test("a completed job that ran steps is scanned", () => {
    expect(scannableJobs([real, failed])).toEqual([real, failed]);
  });

  test("a github-check reporter job (zero steps, no log) is excluded (regression)", () => {
    // reviewdog's `reporter: github-check` surfaces a synthetic check-run in the
    // jobs list with conclusion success but no runner, no steps, and no
    // downloadable log (its `…/logs` endpoint 404s). Sweeping it would fail the
    // gate on a nonexistent log, so it must be filtered out.
    const checkRun = { name: "actionlint", conclusion: "success", steps: [] };
    expect(scannableJobs([real, checkRun])).toEqual([real]);
  });

  test("skipped and in-flight jobs are excluded", () => {
    const skipped = { name: "bench", conclusion: "skipped", steps: [{}] };
    const inflight = { name: "ci-gate", conclusion: null, steps: [] };
    expect(scannableJobs([real, skipped, inflight])).toEqual([real]);
  });

  test("a missing steps field is treated as no steps", () => {
    const noSteps = { name: "phantom", conclusion: "success" };
    expect(scannableJobs([noSteps])).toEqual([]);
  });

  test("the sweep's own test/report jobs are excluded (no self-flagging)", () => {
    // These jobs legitimately print `warning[…]:` fixtures / offenders, so
    // sweeping them would be a guaranteed false positive.
    const testScripts = { name: "test-scripts", conclusion: "success", steps: [{}] };
    const noWarnings = { name: "no-warnings / no-warnings (log sweep)", conclusion: "success", steps: [{}] };
    expect(scannableJobs([real, testScripts, noWarnings])).toEqual([real]);
  });
});

describe("emptySweepState — cancellation is distinct from a miswired sweep", () => {
  test("a terminal cancellation-only dependency set needs no log sweep", () => {
    const cancelled = { name: "check", conclusion: "cancelled", steps: [] };
    const skipped = { name: "coverage", conclusion: "skipped", steps: [] };
    const sweep = { name: "no-warnings (log sweep)", conclusion: null, steps: [] };

    expect(emptySweepState([cancelled, skipped, sweep])).toBe("cancelled");
  });

  test("an incomplete job list is retried instead of being accepted", () => {
    const cancelled = { name: "changes", conclusion: "cancelled", steps: [] };
    const inflight = { name: "check", conclusion: null, steps: [] };
    const sweep = { name: "no-warnings (log sweep)", conclusion: null, steps: [] };

    expect(emptySweepState([])).toBe("retry");
    expect(emptySweepState([inflight, sweep])).toBe("retry");
    expect(emptySweepState([cancelled, inflight, sweep])).toBe("retry");
    expect(emptySweepState([sweep])).toBe("retry");
  });

  test("a persistent non-cancellation empty selection remains fail-closed", () => {
    const skipped = { name: "check", conclusion: "skipped", steps: [] };
    const synthetic = { name: "actionlint", conclusion: "success", steps: [] };
    const timedOut = { name: "coverage", conclusion: "timed_out", steps: [] };
    const cancelled = { name: "changes", conclusion: "cancelled", steps: [] };

    expect(emptySweepState([skipped])).toBe("miswired");
    expect(emptySweepState([synthetic])).toBe("miswired");
    expect(emptySweepState([cancelled, timedOut])).toBe("miswired");
  });
});

describe("isTransientLogArchiveFailure — retry only archive-readiness races", () => {
  test("HTTP 404 from a completed job log is retryable", () => {
    expect(
      isTransientLogArchiveFailure({
        exitCode: 1,
        stderr: Buffer.from("gh: Not Found (HTTP 404)"),
      }),
    ).toBe(true);
  });

  test.each([
    [4, "gh: To get started with GitHub CLI, please run: gh auth login"],
    [1, "gh: Resource not accessible by integration (HTTP 403)"],
    [1, "gh: Validation Failed (HTTP 422)"],
    [1, "error connecting to api.github.com"],
  ])("exit %d with %p fails without retry", (exitCode, stderr) => {
    expect(isTransientLogArchiveFailure({ exitCode, stderr })).toBe(false);
  });

  test("a successful request is never classified as retryable", () => {
    expect(isTransientLogArchiveFailure({ exitCode: 0, stderr: "HTTP 404" })).toBe(false);
  });
});

describe("findWarnings — benign near-misses never match", () => {
  test.each([
    "Compiling app v0.1.0 (0 warnings emitted)",
    "RUSTFLAGS=-D warnings",
    "(node:2670) [DEP0040] DeprecationWarning: The `punycode` module is deprecated.",
    "warnings: 0",
  ])("%p is not an offender", (body) => {
    expect(findWarnings(line(body))).toEqual([]);
  });
});
