#!/usr/bin/env bun
// CI log-sweep: fail if any job in this run logged an un-allowlisted warning.
//
// This is the net under everything else. clippy and rustdoc already fail on
// their own warnings via `-D warnings`, but tools without a native -Werror
// (docker build, apt, useradd, cargo-deny's `warn` checks, …) can print a
// warning while still exiting 0 — so a warning could otherwise slip through a
// green run. This gate reads the run's logs and fails on any tool warning,
// wherever it came from, enforcing constitution §2 ("warnings are errors").
//
// Scope: the sweep covers every job in *one workflow run* (the run it is invoked
// from, GITHUB_RUN_ID). To cover the whole pipeline it is invoked as a job from
// every workflow that runs tools — ci, lint, security, release-plz (via the
// reusable `.github/workflows/no-warnings.yml`, which each calls after its jobs
// finish). A workflow with no sweep job would leave its tool warnings unswept,
// so a new workflow must wire it in too.
//
// Logs are fetched per-job, not via the run's aggregated archive: `gh run view
// --log` serves that archive, which GitHub only publishes once the whole run
// finishes — but this sweep runs *inside* the run (needs: all jobs, if:
// always()), so the archive doesn't exist yet. Each finished job's own log is
// available the moment that job completes; jobs still in flight (this one and
// the gate) have a null conclusion and are skipped.
//
//   CI     : sweeps this run (GITHUB_RUN_ID)
//   local  : `bun scripts/checks/no-ci-warnings.mjs --run <run-id>`
import { $ } from "bun";
import { die, notice } from "../lib/ci.mjs";

// A real warning is a tool's `warning:` (rustc, useradd, most CLIs; `WARNING:`
// too), a *coded* `warning[<code>]:` (rustc's `warning[E0602]:`, cargo-deny's
// `warning[license-not-encountered]:` / `warning[duplicate]:`), or a GitHub
// Actions annotation — `##[warning]…` as rendered in the log, `::warning…` in the
// raw workflow-command form (e.g. an action forced off a deprecated Node
// runtime). Benign near-misses — counts like "0 warnings", the `-D warnings`
// flag, and Node's `DeprecationWarning:` — have no bare-word `warning:`/
// `warning[…]:` boundary and no `##[`/`::` prefix, so they don't match.
const WARNING = /\bwarning(\[[^\]]*\])?:|##\[warning\]|::warning/i;

// Lines that match WARNING but are NOT an actionable warning to fix. Every entry
// is a deliberate, documented exception — never a way to silence a real warning
// (constitution §2: fix at the source, don't grep it away). Keep this short; a
// growing list is a smell.
export const ALLOWLIST = [
  {
    // `pure-analyzer-analysis`'s `Severity` enum (crates/pure-analyzer-analysis/
    // src/pass.rs) has a `Warning` variant; a match arm on it (`Severity::Warning
    // => 1,`) satisfies WARNING's `::warning` branch case-insensitively, even
    // though it is a Rust path separator plus an identifier, not a GitHub
    // Actions raw workflow-command annotation. Matched narrowly on the exact
    // enum path so a real `::warning` annotation (lowercase, GitHub's own
    // syntax) still fails the sweep.
    re: /\bSeverity::Warning\b/,
    why: "pure-analyzer-analysis Severity::Warning enum variant, not a ::warning annotation",
  },
  {
    // `@actions/cache`/`@actions/toolkit` emit this annotation from their HTTP
    // retry logic when the cache backend returns 429 while several jobs race to
    // reserve the *same* shared cache key (Swatinem/rust-cache's by-design shared
    // prefix). The save is best-effort and non-fatal — the winning job stores the
    // cache — so the annotation is a transient of concurrency, not a code or tool
    // defect we can fix at a source. It is not suppressible without disabling the
    // cache. Matched narrowly by the toolkit's exact retry wording so no real
    // warning is caught.
    re: /you've hit a rate limit, your rate limit will reset in/i,
    why: "actions/cache 429 retry annotation under concurrent same-key save (transient, non-fatal)",
  },
];

/**
 * Pure offender detection: every line of `logText` that reads as a warning
 * (per WARNING) and is not covered by a documented ALLOWLIST entry. Kept side
 * effect free so it is unit-testable without touching the network.
 * @param {string} logText concatenated CI job logs
 * @returns {string[]} offending lines (order preserved)
 */
export function findWarnings(logText) {
  return logText
    .split("\n")
    .filter((line) => WARNING.test(line))
    .filter((line) => !ALLOWLIST.some(({ re }) => re.test(line)));
}

// A completed job is worth sweeping only if it actually ran steps on a runner.
// `success`/`failure` are the conclusions of a job that executed (`skipped` has
// no log — its endpoint 404s — and `cancelled`/`timed_out` already fail ci-gate
// on their own).
const SCANNABLE = new Set(["success", "failure"]);

// Jobs whose logs LEGITIMATELY contain warning-shaped text that is not a tool
// warning, so sweeping them is a guaranteed false positive:
//   - `test-scripts` runs this sweep's own unit tests, which print `warning[…]:`
//     fixture strings to prove the matcher fires;
//   - the `no-warnings` sweep job itself echoes each offender it finds.
// Excluded by name so the net can never flag its own scaffolding.
const NON_SWEPT_JOB = /\b(test-scripts|no-warnings)\b/i;

/**
 * Pure selection of the jobs whose logs should be swept. A real runner job has
 * one or more `steps`; a synthetic check-run surfaced in the jobs list by a
 * `github-check` reporter (e.g. reviewdog's actionlint) has ZERO steps, no
 * runner, and no downloadable log — its `…/logs` endpoint 404s. Sweeping it is
 * both impossible and pointless (a check-run emits no tool log), so it is
 * excluded here rather than mistaken for an unreadable real log. Jobs matching
 * `NON_SWEPT_JOB` (this sweep's own test/report output) are excluded too. Kept
 * pure so the filter is unit-testable without the network.
 * @param {Array<{name?: string, conclusion: string, steps?: unknown[]}>} jobs run's jobs
 * @returns {typeof jobs} the jobs to fetch and sweep
 */
export function scannableJobs(jobs) {
  return jobs.filter(
    (job) =>
      SCANNABLE.has(job.conclusion) &&
      (job.steps?.length ?? 0) > 0 &&
      !NON_SWEPT_JOB.test(job.name ?? ""),
  );
}

/**
 * Whether a failed `gh api .../logs` request is the known archive-readiness
 * race. GitHub returns HTTP 404 briefly after marking a real runner job
 * complete; authentication, authorization, malformed-request, and network
 * failures are permanent for this invocation and must fail immediately.
 * @param {{exitCode: number, stderr: Uint8Array|string}} result command result
 * @returns {boolean} true only for the retryable HTTP 404 response
 */
export function isTransientLogArchiveFailure(result) {
  return result.exitCode !== 0 && /\bHTTP\s+404\b/i.test(result.stderr.toString());
}

// Fetch the run's job logs and sweep them. Guarded by `import.meta.main` so the
// pure exports above can be imported by tests without hitting the network.
if (import.meta.main) {
  const runFlag = process.argv.indexOf("--run");
  const runId = runFlag !== -1 ? process.argv[runFlag + 1] : process.env.GITHUB_RUN_ID;
  if (!runId) die("no run id — set GITHUB_RUN_ID or pass --run <id>");

  const repo =
    process.env.GITHUB_REPOSITORY ||
    (await $`gh repo view --json nameWithOwner -q .nameWithOwner`.nothrow().text()).trim();
  if (!repo) die("could not determine repo (set GITHUB_REPOSITORY or run inside a gh-authed repo)");

  const jobsRaw = await $`gh run view ${runId} --repo ${repo} --json jobs`.nothrow().text();
  if (!jobsRaw.trim()) die(`could not list jobs for run ${runId} (need actions:read scope / GH_TOKEN)`);

  const scanned = scannableJobs(JSON.parse(jobsRaw).jobs);
  if (scanned.length === 0) die(`run ${runId} has no completed jobs to sweep`);

  // A dropped log means its warnings go unscanned, so a fetch failure must fail
  // the gate — never vanish silently. Retry first so a transient gh/API blip
  // doesn't flake the gate (constitution §3), then die if a log stays unreadable.
  // GitHub can acknowledge a completed job several seconds before its per-job
  // log archive is readable. Allow up to roughly one minute of linear backoff;
  // three near-immediate attempts proved too short on clean hosted runs.
  const FETCH_ATTEMPTS = 8;
  const RETRY_BACKOFF_MS = 2000;

  async function fetchJobLog(job) {
    for (let attempt = 1; attempt <= FETCH_ATTEMPTS; attempt++) {
      // Recent gh releases reject otherwise-valid log responses containing ANSI
      // control sequences unless the caller opts in. Runner logs routinely
      // contain colour output, so accept those bytes and scan the decoded text.
      const out = await $`gh api --allow-escape-sequences /repos/${repo}/actions/jobs/${job.databaseId}/logs`
        .nothrow()
        .quiet();
      if (out.exitCode === 0) return out.stdout.toString();
      if (!isTransientLogArchiveFailure(out)) {
        const detail = out.stderr.toString().trim() || `gh api exited ${out.exitCode}`;
        die(`could not read logs for job "${job.name}" (${job.databaseId}): ${detail}`);
      }
      if (attempt < FETCH_ATTEMPTS) await Bun.sleep(RETRY_BACKOFF_MS * attempt);
    }
    die(
      `could not read logs for job "${job.name}" (${job.databaseId}) after ` +
        `${FETCH_ATTEMPTS} attempts — need actions:read scope / GH_TOKEN`,
    );
  }

  const logs = (await Promise.all(scanned.map(fetchJobLog))).join("\n");
  const offenders = findWarnings(logs);

  if (offenders.length > 0) {
    notice(`CI logs contain ${offenders.length} warning(s) — warnings are errors (constitution §2):`);
    for (const line of offenders.slice(0, 60)) notice(`  ${line.trim()}`);
    die(
      "Fix each at its source. If a match is genuinely benign, add a justified " +
        "entry to ALLOWLIST in scripts/checks/no-ci-warnings.mjs.",
    );
  }
  notice("CI logs are clean — no un-allowlisted tool warnings.");
}
