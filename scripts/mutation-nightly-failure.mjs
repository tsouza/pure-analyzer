#!/usr/bin/env bun
// mutation-nightly-failure.mjs — surfaces a failing nightly mutation run
// (`.github/workflows/mutation.yml`) as a tracked GitHub issue instead of
// letting it rot silently (issue #276's "nightly-failure surfacing"
// criterion: "a failing nightly mutation run opens/updates a tracking issue
// automatically").
//
// One label (`nightly-mutation-failure`) marks the tracking issue. On every
// failing nightly run this script:
//   - finds the OPEN issue carrying that label and adds a comment noting the
//     new failure, or
//   - opens a fresh one (GitHub auto-creates a label that doesn't exist yet)
//     if none is open.
// A closed tracking issue (the nightly went green and someone closed it) is
// deliberately NOT reopened by a later failure — a fresh episode gets a
// fresh issue rather than reanimating stale discussion on an old one.
//
// Per constitution §2, non-trivial CI step logic is a Bun `.mjs` module
// (not inline YAML): `.github/workflows/mutation.yml` just sets env vars and
// runs this script. Shares its GitHub REST client with the issue/PR labelers
// (`scripts/label/gh-rest.mjs`) rather than carrying its own fetch
// boilerplate.
//
// ENV CONTRACT
//   GITHUB_TOKEN            (required) — needs `issues: write`.
//   GITHUB_REPOSITORY       (required) — `owner/repo`.
//   MUTATION_FAILURE_RUN_URL (required) — link to the failing workflow run.
//   GITHUB_API_URL          (optional) — API base; defaults to https://api.github.com.
//
// Exits 0 on success, 1 on any failure (missing env, API error).

import { die, notice } from "./lib/ci.mjs";
import {
  addIssueComment,
  createIssue,
  DEFAULT_API_URL,
  listOpenIssues,
} from "./label/gh-rest.mjs";

export const TRACKING_LABEL = "nightly-mutation-failure";
export const TRACKING_TITLE = "Nightly mutation run is failing";

/** Comment posted on an already-open tracking issue for a new failing run. */
export function commentBody(runUrl, timestamp) {
  return `Nightly mutation run failed again: ${runUrl} (detected ${timestamp}).`;
}

/** Body for a freshly opened tracking issue. */
export function issueBody(runUrl, timestamp) {
  return [
    "The nightly full-workspace mutation run (`.github/workflows/mutation.yml`) failed.",
    "",
    `First detected failing run: ${runUrl} (${timestamp}).`,
    "",
    "Opened automatically by `scripts/mutation-nightly-failure.mjs` (issue #276) so a red " +
      "nightly cannot rot silently. Close this once the nightly run is green again — a later " +
      `failure opens a fresh issue rather than reopening this one (label: \`${TRACKING_LABEL}\`).`,
  ].join("\n");
}

/** Whether an issue carries the tracking label (label entries may be strings or objects). */
export function hasTrackingLabel(issue) {
  return (issue?.labels ?? []).some((label) =>
    (typeof label === "string" ? label : label?.name) === TRACKING_LABEL,
  );
}

/**
 * Find the open tracking issue and comment on it, or open a new one.
 * Returns `{ action: "commented" | "created", number }`.
 */
export async function upsertTrackingIssue(
  { api, repo, token, runUrl, timestamp },
  dependencies = {},
) {
  const {
    listOpenIssues: listIssues = listOpenIssues,
    createIssue: create = createIssue,
    addIssueComment: comment = addIssueComment,
  } = dependencies;

  const openIssues = await listIssues(api, repo, token);
  const existing = openIssues.find(hasTrackingLabel);

  if (existing) {
    await comment(api, repo, token, existing.number, commentBody(runUrl, timestamp));
    return { action: "commented", number: existing.number };
  }

  const created = await create(api, repo, token, {
    title: TRACKING_TITLE,
    body: issueBody(runUrl, timestamp),
    labels: [TRACKING_LABEL],
  });
  return { action: "created", number: created.number };
}

async function main() {
  const api = process.env.GITHUB_API_URL || DEFAULT_API_URL;
  const repo = process.env.GITHUB_REPOSITORY ?? "";
  const token = process.env.GITHUB_TOKEN ?? "";
  const runUrl = process.env.MUTATION_FAILURE_RUN_URL ?? "";

  if (!repo) die("GITHUB_REPOSITORY is required (owner/repo)");
  if (!token) die("GITHUB_TOKEN is required");
  if (!runUrl) die("MUTATION_FAILURE_RUN_URL is required");

  const result = await upsertTrackingIssue({
    api,
    repo,
    token,
    runUrl,
    timestamp: new Date().toISOString(),
  });
  notice(`mutation nightly-failure tracking: ${result.action} issue #${result.number}`);
}

if (import.meta.main) {
  try {
    await main();
  } catch (error) {
    die(error instanceof Error ? error.message : String(error));
  }
}
