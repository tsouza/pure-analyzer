import { afterAll, beforeAll, describe, expect, test } from "bun:test";
import { mkdtempSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { checkLocalGit } from "./project-identity.mjs";
import { PROJECT_GIT_REMOTE } from "../lib/project-identity.mjs";

const checker = fileURLToPath(new URL("./project-identity.mjs", import.meta.url));
const claudeHook = fileURLToPath(
  new URL("../hooks/claude-project-identity.mjs", import.meta.url),
);

function run(command, { cwd, env = process.env, stdin } = {}) {
  return Bun.spawnSync(command, { cwd, env, stdin, stdout: "pipe", stderr: "pipe" });
}

function git(cwd, args) {
  const result = run(["git", ...args], { cwd });
  if (result.exitCode !== 0) throw new Error(result.stderr.toString());
  return result.stdout.toString().trim();
}

describe("project identity command boundaries", () => {
  let repository;
  let linkedWorktree;
  let base;
  let head;

  beforeAll(() => {
    repository = mkdtempSync(join(tmpdir(), "pure-analyzer-identity-test-"));
    linkedWorktree = `${repository}-linked`;
    const hooks = join(repository, "empty-hooks");
    mkdirSync(hooks);
    git(repository, ["init", "-b", "main"]);
    git(repository, ["config", "core.hooksPath", hooks]);
    git(repository, ["config", "user.name", "Thiago Souza"]);
    git(repository, ["config", "user.email", "122435+tsouza@users.noreply.github.com"]);
    git(repository, ["config", "commit.gpgsign", "false"]);
    git(repository, ["remote", "add", "origin", PROJECT_GIT_REMOTE]);

    const tracked = join(repository, "tracked.txt");
    writeFileSync(tracked, "base\n");
    git(repository, ["add", "tracked.txt"]);
    git(repository, ["commit", "-m", "chore: base"]);
    base = git(repository, ["rev-parse", "HEAD"]);
    writeFileSync(tracked, "head\n");
    git(repository, ["commit", "-am", "chore: head"]);
    head = git(repository, ["rev-parse", "HEAD"]);
    git(repository, ["worktree", "add", "-b", "identity-test", linkedWorktree, head]);
  });

  afterAll(() => {
    rmSync(linkedWorktree, { recursive: true, force: true });
    rmSync(repository, { recursive: true, force: true });
  });

  test("reads effective identity and shared origin from a linked worktree", () => {
    expect(checkLocalGit(linkedWorktree)).toEqual([]);

    const wrongEnvironment = run(["bun", checker, "git"], {
      cwd: linkedWorktree,
      env: { ...process.env, GIT_AUTHOR_EMAIL: "wrong@example.com" },
    });
    expect(wrongEnvironment.exitCode).not.toBe(0);
    expect(wrongEnvironment.stderr.toString()).toContain("effective Git author email");

    const wrongTransport = run(["bun", checker, "git"], {
      cwd: linkedWorktree,
      env: { ...process.env, GIT_SSH_COMMAND: "ssh -i /tmp/squid-key" },
    });
    expect(wrongTransport.exitCode).not.toBe(0);
    expect(wrongTransport.stderr.toString()).toContain("GIT_SSH_COMMAND is forbidden");

    const wrongPush = run(
      [
        "bun",
        checker,
        "git",
        "--remote-name",
        "fork",
        "--remote-url",
        "git@github.com:other/pure-analyzer.git",
      ],
      { cwd: linkedWorktree },
    );
    expect(wrongPush.exitCode).not.toBe(0);
    expect(wrongPush.stderr.toString()).toContain("pushes must use the origin remote");

    git(repository, ["config", "--add", "remote.origin.pushurl", PROJECT_GIT_REMOTE]);
    git(repository, [
      "config",
      "--add",
      "remote.origin.pushurl",
      "git@github.com-tsouza:tsouza-squid/pure-analyzer.git",
    ]);
    const multiplePushUrls = run(["bun", checker, "git"], { cwd: linkedWorktree });
    expect(multiplePushUrls.exitCode).not.toBe(0);
    expect(multiplePushUrls.stderr.toString()).toContain("exactly one push URL");
    git(repository, ["config", "--unset-all", "remote.origin.pushurl"]);
  });

  test("validates a temporary event file and PR commit range without network", () => {
    const eventPath = join(repository, "event.json");
    const event = {
      sender: { login: "tsouza", type: "User" },
      pull_request: {
        user: { login: "tsouza", type: "User" },
        base: { sha: base },
        head: { sha: head },
      },
    };
    writeFileSync(eventPath, JSON.stringify(event));
    const env = {
      ...process.env,
      GITHUB_ACTIONS: "true",
      GITHUB_EVENT_NAME: "pull_request",
      GITHUB_EVENT_PATH: eventPath,
      GITHUB_ACTOR: "tsouza",
      GITHUB_TRIGGERING_ACTOR: "tsouza",
    };
    expect(run(["bun", checker, "ci"], { cwd: repository, env }).exitCode).toBe(0);

    const rejected = run(["bun", checker, "ci"], {
      cwd: repository,
      env: { ...env, GITHUB_ACTOR: "tsouza-in-automode" },
    });
    expect(rejected.exitCode).not.toBe(0);
    expect(rejected.stderr.toString()).toContain("workflow actor must be tsouza");
  });

  test("the Claude hook blocks malformed input", () => {
    for (const input of ["{", "{}", '{"tool_name":"Bash","tool_input":{}}']) {
      const result = run(["bun", claudeHook], {
        cwd: linkedWorktree,
        stdin: Buffer.from(input),
      });
      expect(result.exitCode).toBe(2);
      expect(result.stderr.toString()).toContain("project-identity:");
    }
  });
});
