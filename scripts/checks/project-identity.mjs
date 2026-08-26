#!/usr/bin/env bun
// Fail-closed identity gate for local Git hooks and GitHub Actions.
import { die, notice } from "../lib/ci.mjs";
import {
  GIT_TRANSPORT_OVERRIDE_NAMES,
  gitIdentityProblems,
  githubEventProblems,
  parseCommitIdentities,
  PROJECT_GIT_AUTHOR_EMAIL,
  PROJECT_GIT_AUTHOR_NAME,
  PROJECT_GIT_REMOTE,
  pullRequestCommitProblems,
} from "../lib/project-identity.mjs";

function gitOutput(args, cwd = process.cwd()) {
  const result = Bun.spawnSync(["git", ...args], { cwd, stdout: "pipe", stderr: "pipe" });
  if (result.exitCode !== 0) {
    const detail = result.stderr.toString().trim() || `git ${args.join(" ")} exited ${result.exitCode}`;
    throw new Error(detail);
  }
  return result.stdout.toString().trim();
}

export function localGitSnapshot(cwd = process.cwd()) {
  return {
    authorIdent: gitOutput(["var", "GIT_AUTHOR_IDENT"], cwd),
    committerIdent: gitOutput(["var", "GIT_COMMITTER_IDENT"], cwd),
    fetchRemotes: gitOutput(["remote", "get-url", "--all", "origin"], cwd).split(/\r?\n/),
    pushRemotes: gitOutput(["remote", "get-url", "--push", "--all", "origin"], cwd).split(
      /\r?\n/,
    ),
    transportEnvironment: Object.fromEntries(
      GIT_TRANSPORT_OVERRIDE_NAMES.filter((name) => Object.hasOwn(process.env, name)).map(
        (name) => [name, process.env[name]],
      ),
    ),
  };
}

export function checkLocalGit(cwd, hookRemoteName, hookRemoteUrl) {
  return gitIdentityProblems({
    ...localGitSnapshot(cwd),
    hookRemoteName,
    hookRemoteUrl,
  });
}

async function checkCiEvent() {
  const eventPath = process.env.GITHUB_EVENT_PATH;
  if (!eventPath) die("GITHUB_EVENT_PATH is required for the CI identity gate");

  let payload;
  try {
    payload = await Bun.file(eventPath).json();
  } catch (error) {
    die(`could not read GitHub event payload: ${error.message}`);
  }
  const problems = githubEventProblems({
    eventName: process.env.GITHUB_EVENT_NAME,
    actor: process.env.GITHUB_ACTOR,
    triggeringActor: process.env.GITHUB_TRIGGERING_ACTOR,
    payload,
  });
  if (process.env.GITHUB_EVENT_NAME === "pull_request") {
    const base = payload?.pull_request?.base?.sha;
    const head = payload?.pull_request?.head?.sha;
    if (!/^[0-9a-f]{40}$/.test(base ?? "") || !/^[0-9a-f]{40}$/.test(head ?? "")) {
      problems.push("pull-request event is missing valid base/head commit IDs");
    } else {
      try {
        const records = gitOutput([
          "log",
          "-z",
          "--format=%H%x00%an%x00%ae%x00%cn%x00%ce",
          `${base}..${head}`,
        ]);
        problems.push(
          ...pullRequestCommitProblems(parseCommitIdentities(records), payload.pull_request.user),
        );
      } catch (error) {
        problems.push(`could not inspect pull-request commit identities: ${error.message}`);
      }
    }
  }
  if (problems.length > 0) die(problems.join("; "));
  notice("GitHub event identity satisfies the project policy.");
}

if (import.meta.main) {
  const mode = process.argv[2];
  if (mode === "ci") {
    await checkCiEvent();
  } else if (mode === "git") {
    const remoteNameAt = process.argv.indexOf("--remote-name");
    const remoteUrlAt = process.argv.indexOf("--remote-url");
    try {
      const problems = checkLocalGit(
        process.cwd(),
        remoteNameAt === -1 ? undefined : process.argv[remoteNameAt + 1],
        remoteUrlAt === -1 ? undefined : process.argv[remoteUrlAt + 1],
      );
      if (problems.length > 0) {
        die(
          `${problems.join("; ")}. Required: ${PROJECT_GIT_AUTHOR_NAME} ` +
            `<${PROJECT_GIT_AUTHOR_EMAIL}> and ${PROJECT_GIT_REMOTE}`,
        );
      }
      notice("Local Git author, committer, and origin satisfy the project identity policy.");
    } catch (error) {
      die(`could not verify local Git identity: ${error.message}`);
    }
  } else {
    die("usage: bun scripts/checks/project-identity.mjs <git|ci>");
  }
}
