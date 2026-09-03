#!/usr/bin/env bun
// pr-label.mjs — applies exactly one Conventional-Commit type label to every
// PR, derived from the PR TITLE's commitlint prefix (the title IS the
// squash-merge commit subject, so it always carries the type). Ported from
// the sibling `cerberus` repo's `.github/scripts` + `pr-label.yml` pattern
// (see those files), reworked for this repo's constitution: cerberus's
// workflow runs its two jobs' logic as inline `actions/github-script`
// blocks; constitution §2 reserves that shape for a single-tool pass-
// through and puts anything with branching/loops in a Bun `.mjs` invoked by
// a plain `run:` line instead — so this script (not inline YAML) is the
// whole implementation, and `pr-label.yml` just sets env vars and runs it.
//
// The mapping itself (feat -> enhancement, fix -> bug, …) lives in
// `type-label.mjs`, the single source of truth `issue-label.mjs` also
// delegates to — so the two labelers can never disagree about what a CC
// prefix means.
//
// Two modes, driven by PR_LABEL_MODE:
//   event    — label the single PR named by PR_LABEL_PR_NUMBER (the
//              workflow reads it from the pull_request_target payload).
//   backfill — self-healing: walk every OPEN PR and apply any MISSING
//              expected label. Catches a PR whose event-driven run was
//              queued, failed, or never fired. Idempotent: skips a PR that
//              already carries its expected label.
//
// DEPENDABOT: bot-authored PRs (login ending `[bot]`) are SKIPPED by BOTH
// modes. Dependabot already self-labels via `.github/dependabot.yml`
// (`labels: [dependencies]`) and `.github/labeler.yml`'s path globs; a
// label edit from a non-Dependabot actor makes Dependabot refuse to
// auto-rebase the PR (recovery is `@dependabot recreate`). Skipping bots
// keeps both modes consistent and leaves Dependabot's rebase intact.
//
// SECURITY: this script never checks out or executes PR head code. The
// event path reads the PR title from the `pull_request_target` payload
// (which runs with a write token on the BASE ref); the backfill path reads
// titles from the REST API. The only code either path runs is this
// trusted, in-repo module from the base ref.
//
// ENV CONTRACT
//   GITHUB_TOKEN       (required, unless PR_LABEL_DRY_RUN=1 with a fixture)
//   GITHUB_REPOSITORY  (required) — `owner/repo`.
//   PR_LABEL_MODE      (required) — `event` | `backfill`.
//   GITHUB_EVENT_PATH  (required in `event` mode) — webhook payload JSON.
//   PR_LABEL_DRY_RUN   (optional) — `1`/`true` computes and reports without applying.
//   PR_LABEL_FIXTURE   (optional, dry-run only) — path to a JSON array of
//                        `{number, title, user: {login}, labels}` to read
//                        INSTEAD of the API.
//   GITHUB_API_URL     (optional) — API base; defaults to https://api.github.com.
//
// Exits 0 on success, 1 on any failure (vacuous run, API error).
//
// argv `--check-tables` runs the mapping-table assertion and exits.

import { readFileSync } from "node:fs";
import process from "node:process";

import { die, error, notice } from "../lib/ci.mjs";
import { addLabels, DEFAULT_API_URL, listOpenPullRequests } from "./gh-rest.mjs";
import { labelsForTitle, TYPE_TO_LABEL } from "./type-label.mjs";

/** isBotAuthored reports whether a PR's author login is a bot account (ends `[bot]`). */
export function isBotAuthored(pr) {
  return (pr?.user?.login ?? "").endsWith("[bot]");
}

/**
 * labelsForPullRequest computes the FULL decision for one PR:
 *   { want, missing, botSkipped }
 * `want` is the type-label mapping's full answer for the title; `missing`
 * is the additive delta to POST (empty for a bot-authored PR, or one that
 * already carries everything it should).
 */
export function labelsForPullRequest(pr) {
  if (isBotAuthored(pr)) return { want: [], missing: [], botSkipped: true };
  const want = labelsForTitle(pr?.title ?? "");
  const have = new Set((pr?.labels ?? []).map((l) => (typeof l === "string" ? l : l.name)));
  return { want, missing: want.filter((l) => !have.has(l)), botSkipped: false };
}

/** assertTablesUsable fails the run if the shared mapping table has been emptied. */
export function assertTablesUsable() {
  const problems = [];
  if (Object.keys(TYPE_TO_LABEL).length === 0) problems.push("TYPE_TO_LABEL (from type-label.mjs) is empty");
  return problems;
}

function truthy(v) {
  return v === "1" || String(v).toLowerCase() === "true";
}

async function main() {
  const tableProblems = assertTablesUsable();
  if (tableProblems.length > 0) die(`pr-label mapping table is unusable:\n  - ${tableProblems.join("\n  - ")}`);

  const mode = process.env.PR_LABEL_MODE ?? "";
  if (mode !== "event" && mode !== "backfill") {
    die(`PR_LABEL_MODE must be 'event' or 'backfill' (got ${JSON.stringify(mode)})`);
  }
  const dryRun = truthy(process.env.PR_LABEL_DRY_RUN);
  const api = process.env.GITHUB_API_URL || DEFAULT_API_URL;
  const repo = process.env.GITHUB_REPOSITORY ?? "";
  const token = process.env.GITHUB_TOKEN ?? "";
  const fixture = process.env.PR_LABEL_FIXTURE ?? "";

  if (!fixture && !repo) die("GITHUB_REPOSITORY is required (owner/repo)");
  if (!fixture && !token) die("GITHUB_TOKEN is required");

  let prs;
  if (fixture) {
    if (!dryRun) die("PR_LABEL_FIXTURE is a dry-run-only input; set PR_LABEL_DRY_RUN=1");
    prs = JSON.parse(readFileSync(fixture, "utf8"));
  } else if (mode === "event") {
    const eventPath = process.env.GITHUB_EVENT_PATH ?? "";
    if (!eventPath) die("GITHUB_EVENT_PATH is required in event mode");
    const payload = JSON.parse(readFileSync(eventPath, "utf8"));
    if (!payload.pull_request) die("event payload carries no `pull_request` object");
    prs = [payload.pull_request];
  } else {
    prs = await listOpenPullRequests(api, repo, token);
  }

  if (!Array.isArray(prs)) die("the PR set is not an array — nothing to process");

  // ANTI-VACUITY: a run that saw no PRs at all is a broken run, not a clean
  // one. In event mode the payload always carries exactly one.
  if (prs.length === 0) {
    die(`pr-label (${mode}) processed ZERO pull requests — the fetch returned nothing`);
  }

  let applied = 0;
  let touched = 0;
  let alreadyComplete = 0;
  let botSkipped = 0;
  let noType = 0;

  for (const pr of prs) {
    const number = pr.number;
    const title = pr.title ?? "";
    const decision = labelsForPullRequest(pr);

    if (decision.botSkipped) {
      botSkipped++;
      notice(`PR #${number} is bot-authored (${pr.user?.login}); skipping (it self-labels).`);
      continue;
    }
    if (decision.want.length === 0) {
      noType++;
      continue; // no CC prefix, or `style:` — not an error, just nothing to add.
    }
    if (decision.missing.length === 0) {
      alreadyComplete++;
      notice(`PR #${number} already carries: ${decision.want.join(", ")}.`);
      continue;
    }

    if (dryRun) {
      notice(`PR #${number} DRY-RUN would add [${decision.missing.join(", ")}] — ${title}`);
    } else {
      await addLabels(api, repo, token, number, decision.missing);
      notice(`PR #${number} labeled [${decision.missing.join(", ")}] — ${title}`);
    }
    applied += decision.missing.length;
    touched++;
  }

  // ANTI-VACUITY: in backfill mode, applying nothing while every PR was
  // eligible (none bot-skipped, none type-less) but stayed unlabeled would
  // be the hollow-green shape this design exists to prevent.
  const eligible = prs.length - botSkipped - noType;
  if (mode === "backfill" && !dryRun && applied === 0 && eligible > 0 && alreadyComplete < eligible) {
    die(`backfill applied ZERO labels while ${eligible - alreadyComplete} eligible PR(s) remain unlabeled — the labeler is inert`);
  }

  notice(
    `pr-label (${mode}${dryRun ? ", dry-run" : ""}): ${prs.length} PR(s) scanned, ${touched} PR(s) ` +
      `${dryRun ? "would be" : ""} labeled, ${applied} label(s) ${dryRun ? "pending" : "applied"}, ` +
      `${alreadyComplete} already complete, ${botSkipped} bot-skipped, ${noType} carry no CC type.`,
  );
}

if (process.argv.includes("--check-tables")) {
  const problems = assertTablesUsable();
  if (problems.length > 0) die(`pr-label --check-tables: ${problems.join("; ")}`);
  notice("pr-label --check-tables: mapping table usable");
  process.exit(0);
}

if (import.meta.main) {
  main().catch((e) => {
    error(String(e?.stack ?? e));
    process.exit(1);
  });
}
