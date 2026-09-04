import { expect, test } from "bun:test";
import { join } from "node:path";

import {
  commentBody,
  hasTrackingLabel,
  issueBody,
  TRACKING_LABEL,
  TRACKING_TITLE,
  upsertTrackingIssue,
} from "./mutation-nightly-failure.mjs";

const CLI = join(import.meta.dir, "mutation-nightly-failure.mjs");
const RUN_URL = "https://github.com/tsouza/pure-analyzer/actions/runs/123";
const TIMESTAMP = "2026-09-02T03:17:00.000Z";

function runCLI(env) {
  return Bun.spawnSync([process.execPath, CLI], { env: { ...process.env, ...env } });
}

test("hasTrackingLabel recognises both string and object label shapes", () => {
  expect(hasTrackingLabel({ labels: [TRACKING_LABEL] })).toBeTrue();
  expect(hasTrackingLabel({ labels: [{ name: TRACKING_LABEL }] })).toBeTrue();
  expect(hasTrackingLabel({ labels: ["bug", { name: "enhancement" }] })).toBeFalse();
  expect(hasTrackingLabel({ labels: [] })).toBeFalse();
  expect(hasTrackingLabel({})).toBeFalse();
});

test("commentBody and issueBody carry the run URL and detection timestamp", () => {
  expect(commentBody(RUN_URL, TIMESTAMP)).toContain(RUN_URL);
  expect(commentBody(RUN_URL, TIMESTAMP)).toContain(TIMESTAMP);
  expect(issueBody(RUN_URL, TIMESTAMP)).toContain(RUN_URL);
  expect(issueBody(RUN_URL, TIMESTAMP)).toContain(TIMESTAMP);
  expect(issueBody(RUN_URL, TIMESTAMP)).toContain(TRACKING_LABEL);
});

test("comments on an already-open tracking issue instead of opening a duplicate", async () => {
  let commented;
  let created = false;
  const result = await upsertTrackingIssue(
    { api: "https://api.github.com", repo: "o/r", token: "t", runUrl: RUN_URL, timestamp: TIMESTAMP },
    {
      listOpenIssues: async () => [
        { number: 1, labels: ["bug"] },
        { number: 42, labels: [{ name: TRACKING_LABEL }] },
      ],
      createIssue: async () => {
        created = true;
        throw new Error("must not create a duplicate issue");
      },
      addIssueComment: async (_api, _repo, _token, number, body) => {
        commented = { number, body };
        return { id: 1, body };
      },
    },
  );

  expect(created).toBeFalse();
  expect(commented).toEqual({ number: 42, body: commentBody(RUN_URL, TIMESTAMP) });
  expect(result).toEqual({ action: "commented", number: 42 });
});

test("opens a new labeled tracking issue when none is currently open", async () => {
  let createdWith;
  const result = await upsertTrackingIssue(
    { api: "https://api.github.com", repo: "o/r", token: "t", runUrl: RUN_URL, timestamp: TIMESTAMP },
    {
      listOpenIssues: async () => [{ number: 1, labels: ["bug"] }],
      createIssue: async (_api, _repo, _token, payload) => {
        createdWith = payload;
        return { number: 99 };
      },
      addIssueComment: async () => {
        throw new Error("must not comment when no tracking issue is open");
      },
    },
  );

  expect(createdWith).toEqual({
    title: TRACKING_TITLE,
    body: issueBody(RUN_URL, TIMESTAMP),
    labels: [TRACKING_LABEL],
  });
  expect(result).toEqual({ action: "created", number: 99 });
});

test("CLI fails closed on missing required environment", () => {
  const missingRepo = runCLI({
    GITHUB_REPOSITORY: "",
    GITHUB_TOKEN: "t",
    MUTATION_FAILURE_RUN_URL: RUN_URL,
  });
  expect(missingRepo.exitCode).toBe(1);
  expect(missingRepo.stderr.toString()).toContain("GITHUB_REPOSITORY");

  const missingToken = runCLI({
    GITHUB_REPOSITORY: "o/r",
    GITHUB_TOKEN: "",
    MUTATION_FAILURE_RUN_URL: RUN_URL,
  });
  expect(missingToken.exitCode).toBe(1);
  expect(missingToken.stderr.toString()).toContain("GITHUB_TOKEN");

  const missingRunUrl = runCLI({
    GITHUB_REPOSITORY: "o/r",
    GITHUB_TOKEN: "t",
    MUTATION_FAILURE_RUN_URL: "",
  });
  expect(missingRunUrl.exitCode).toBe(1);
  expect(missingRunUrl.stderr.toString()).toContain("MUTATION_FAILURE_RUN_URL");
});
