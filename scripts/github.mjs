#!/usr/bin/env bun
// Identity-checked GitHub CLI entry point. Use this instead of direct `gh` writes.
import { userInfo } from "node:os";
import { join } from "node:path";
import { die } from "./lib/ci.mjs";
import {
  githubAuthPolicy,
  githubInvocationProblem,
  githubLoginProblem,
  PROJECT_GITHUB_LOGIN,
} from "./lib/project-identity.mjs";

const PROJECT_GITHUB_HOST = "github.com";
const AUTH_ENVIRONMENT = [
  "GH_TOKEN",
  "GITHUB_TOKEN",
  "GH_ENTERPRISE_TOKEN",
  "GITHUB_ENTERPRISE_TOKEN",
  "GH_HOST",
  "GH_CONFIG_DIR",
  "GH_DEBUG",
  "HOME",
  "USERPROFILE",
  "HOMEDRIVE",
  "HOMEPATH",
  "XDG_CONFIG_HOME",
  "XDG_CONFIG_DIRS",
  "APPDATA",
  "LOCALAPPDATA",
];
const HOME_ENVIRONMENT = ["HOME", "USERPROFILE", "HOMEDRIVE", "HOMEPATH"];

function operatingSystemHomeDirectory() {
  const saved = HOME_ENVIRONMENT.map((name) => [
    name,
    Object.hasOwn(process.env, name),
    process.env[name],
  ]);
  try {
    for (const name of HOME_ENVIRONMENT) delete process.env[name];
    const home = userInfo().homedir;
    if (!home) throw new Error("operating-system home directory is unavailable");
    return home;
  } finally {
    for (const [name, existed, value] of saved) {
      if (existed) process.env[name] = value;
      else delete process.env[name];
    }
  }
}

const PROJECT_USER_HOME = operatingSystemHomeDirectory();

/** Run one gh invocation with injectable effects so tests never touch the network. */
export async function runGuardedGithub(args, { acquireCredential, verifyCredential, execute }) {
  if (args.length === 0) throw new Error("a GitHub CLI command is required");

  const invocationProblem = githubInvocationProblem(args);
  if (invocationProblem) throw new Error(invocationProblem);

  const policy = githubAuthPolicy(args);
  if (policy.action === "block") throw new Error(policy.reason);
  if (policy.action === "repair-switch") return execute(args, null);

  const credential = await acquireCredential();
  if (
    !credential ||
    typeof credential.token !== "string" ||
    credential.token.length === 0 ||
    credential.host !== PROJECT_GITHUB_HOST
  ) {
    throw new Error("could not establish the pinned tsouza GitHub credential boundary");
  }
  const login = (await verifyCredential(credential)).trim();
  const problem = githubLoginProblem(login);
  if (problem) throw new Error(problem);
  return execute(args, credential);
}

export function githubEnvironment(credential = null, sourceEnvironment = process.env) {
  const environment = { ...sourceEnvironment };
  for (const name of AUTH_ENVIRONMENT) delete environment[name];
  environment.HOME = PROJECT_USER_HOME;
  if (process.platform === "win32") {
    environment.USERPROFILE = PROJECT_USER_HOME;
    environment.APPDATA = join(PROJECT_USER_HOME, "AppData", "Roaming");
    environment.LOCALAPPDATA = join(PROJECT_USER_HOME, "AppData", "Local");
  }
  environment.GH_HOST = PROJECT_GITHUB_HOST;
  if (credential) environment.GH_TOKEN = credential.token;
  return environment;
}

async function acquireProjectCredential() {
  const process = Bun.spawn(["gh", "auth", "token", "--user", PROJECT_GITHUB_LOGIN], {
    stdin: "ignore",
    stdout: "pipe",
    stderr: "pipe",
    env: githubEnvironment(),
  });
  const [exitCode, stdout] = await Promise.all([
    process.exited,
    new Response(process.stdout).text(),
    new Response(process.stderr).text(),
  ]);
  if (exitCode !== 0) {
    throw new Error(`could not obtain the stored ${PROJECT_GITHUB_LOGIN} credential`);
  }
  const token = stdout.trim();
  if (!token) throw new Error(`stored ${PROJECT_GITHUB_LOGIN} credential is empty`);
  return Object.freeze({ token, host: PROJECT_GITHUB_HOST });
}

async function verifyProjectCredential(credential) {
  const process = Bun.spawn(["gh", "api", "user", "--jq", ".login"], {
    stdin: "ignore",
    stdout: "pipe",
    stderr: "pipe",
    env: githubEnvironment(credential),
  });
  const [exitCode, stdout] = await Promise.all([
    process.exited,
    new Response(process.stdout).text(),
    new Response(process.stderr).text(),
  ]);
  if (exitCode !== 0) throw new Error("could not verify the pinned GitHub credential");
  return stdout;
}

async function executeGh(args, credential) {
  const process = Bun.spawn(["gh", ...args], {
    stdin: "inherit",
    stdout: "inherit",
    stderr: "inherit",
    env: githubEnvironment(credential),
  });
  return process.exited;
}

if (import.meta.main) {
  try {
    const exitCode = await runGuardedGithub(process.argv.slice(2), {
      acquireCredential: acquireProjectCredential,
      verifyCredential: verifyProjectCredential,
      execute: executeGh,
    });
    process.exit(exitCode);
  } catch (error) {
    die(`GitHub identity guard: ${error.message}`);
  }
}
