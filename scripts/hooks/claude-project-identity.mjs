#!/usr/bin/env bun
// Claude Code PreToolUse guard for Git/GitHub mutations in this repository.
import { existsSync, readFileSync } from "node:fs";
import { isAbsolute, join } from "node:path";
import { checkLocalGit } from "../checks/project-identity.mjs";
import { githubAuthPolicy, PROJECT_GITHUB_LOGIN } from "../lib/project-identity.mjs";

const COMMAND_WRAPPERS = new Set(["command", "env", "exec", "nice", "sudo", "time"]);
const GIT_OPTIONS_WITH_VALUE = new Set([
  "-C",
  "-c",
  "--config-env",
  "--git-dir",
  "--work-tree",
  "--namespace",
]);
const GUARDED_GIT_COMMANDS = new Set([
  "am",
  "cherry-pick",
  "commit",
  "merge",
  "push",
  "rebase",
  "revert",
  "tag",
]);
const IDENTITY_ENVIRONMENT = /^(APPDATA|HOME|HOMEDRIVE|HOMEPATH|LOCALAPPDATA|USERPROFILE|XDG_CONFIG_HOME|XDG_CONFIG_DIRS|GH_TOKEN|GITHUB_TOKEN|GH_ENTERPRISE_TOKEN|GITHUB_ENTERPRISE_TOKEN|GH_CONFIG_DIR|GH_HOST|GIT_(AUTHOR|COMMITTER)_(NAME|EMAIL)|GIT_CONFIG_|GIT_DIR|GIT_WORK_TREE|GIT_SSH|GIT_SSH_COMMAND|GIT_SSH_VARIANT|GIT_PROXY_COMMAND|GIT_ASKPASS|SSH_AUTH_SOCK|SSH_ASKPASS|SSH_ASKPASS_REQUIRE|SSH_SK_PROVIDER)=/;
const GIT_IDENTITY_OPTION = /^(--author(?:=|$)|--no-verify$|-n$|--git-dir(?:=|$)|--work-tree(?:=|$)|--config-env(?:=|$)|-c)/;
const EXECUTABLE_PREFIX = String.raw`(?:(?:[A-Za-z]:)?[\\/])?(?:[^\s;&|()'"\x60]+[\\/])*`;
const RAW_GH = new RegExp(
  String.raw`(?:^|[\s;&|()'"\x60])${EXECUTABLE_PREFIX}gh(?:\.exe)?(?=$|[\s;&|()'"\x60])`,
  "i",
);
const GIT_TEXT = new RegExp(
  String.raw`(?:^|[\s;&|()'"\x60])${EXECUTABLE_PREFIX}git(?:\.exe)?(?=$|[\s;&|()'"\x60])`,
  "i",
);
const CHECKED_GITHUB_TEXT = new RegExp(
  String.raw`(?:^|[\s;&|])(?:just\s+github|(?:${EXECUTABLE_PREFIX})?bun(?:\.exe)?\s+(?:[^\s;&|]+[\\/])?scripts[\\/]github\.mjs)(?=$|[\s;&|])`,
  "i",
);
const EXACT_GH_REPAIR = new RegExp(
  String.raw`^\s*gh(?:\.exe)?\s+auth\s+switch\s+(?:--user(?:=|\s+)tsouza|-u\s+tsouza)\s*$`,
  "i",
);
const INDIRECT_EXECUTION = new RegExp(
  String.raw`(?:^|[\s;&|()'"\x60])${EXECUTABLE_PREFIX}(?:ba|z|da)?sh(?:\.exe)?\s+-[^\s]*c(?:\s|$)|` +
    String.raw`(?:^|[\s;&|])(?:env|sudo|nice|time)\s+-|(?:^|[\s;&|])(?:eval|xargs)(?:\s|$)|` +
    String.raw`(?:^|[;&|]\s*)\(|\$\(`,
  "i",
);

export function splitShellSegments(command) {
  return command.split(/&&|\|\||[;\n|]/g);
}

function bareExecutable(word) {
  return word
    ?.replace(/^[('"`]+|[)'"`]+$/g, "")
    .split(/[\\/]/)
    .at(-1)
    ?.replace(/\.exe$/i, "")
    .toLowerCase();
}

function commandStart(words) {
  let index = 0;
  while (index < words.length) {
    if (/^[A-Za-z_][A-Za-z0-9_]*=/.test(words[index])) {
      index += 1;
      continue;
    }

    const wrapper = bareExecutable(words[index]);
    if (!COMMAND_WRAPPERS.has(wrapper)) break;
    index += 1;
    if (wrapper === "command") {
      while (["-p", "-v", "-V"].includes(words[index])) index += 1;
      if (words[index] === "--") index += 1;
    } else if (wrapper === "exec") {
      while (index < words.length) {
        const option = words[index];
        if (option === "-a" || option === "--argv0") {
          index += 2;
        } else if (option.startsWith("--argv0=")) {
          index += 1;
        } else if (option === "-c" || option === "-l") {
          index += 1;
        } else if (option === "--") {
          index += 1;
          break;
        } else {
          break;
        }
      }
    }
  }
  return index;
}

/** Find a direct git/gh invocation in one simple shell segment. */
export function projectInvocation(segment) {
  const words = segment.trim().replace(/^[()]+/, "").split(/\s+/).filter(Boolean);
  let index = commandStart(words);
  const executable = bareExecutable(words[index]);
  if (executable !== "git" && executable !== "gh") return null;
  index += 1;

  if (executable === "gh") return { executable, args: words.slice(index), words };
  const prefixStart = index;
  while (index < words.length && words[index].startsWith("-")) {
    const option = words[index];
    index += GIT_OPTIONS_WITH_VALUE.has(option) ? 2 : 1;
  }
  return {
    executable,
    args: words.slice(index),
    prefixArgs: words.slice(prefixStart, index),
    words,
  };
}

function identityOverrideProblem(invocation) {
  const environment = invocation.words.find((word) => IDENTITY_ENVIRONMENT.test(word));
  if (environment) return `command-local identity override is forbidden: ${environment.split("=")[0]}`;

  if (invocation.executable === "gh") return null;
  for (let index = 0; index < invocation.words.length; index += 1) {
    const word = invocation.words[index];
    if (GIT_IDENTITY_OPTION.test(word)) return `Git identity/hook override is forbidden: ${word}`;
  }
  return null;
}

function unquote(value) {
  if (!value) return value;
  const first = value[0];
  return (first === "\"" || first === "'") && value.at(-1) === first ? value.slice(1, -1) : value;
}

function resolveDirectory(value, base) {
  const path = unquote(value);
  if (!path) return base;
  return isAbsolute(path) ? path : join(base, path);
}

function gitDirectory(invocation, fallback) {
  let directory = fallback;
  for (let index = 0; index < invocation.prefixArgs.length; index += 1) {
    const option = invocation.prefixArgs[index];
    if (option === "-C") {
      directory = resolveDirectory(invocation.prefixArgs[index + 1], directory);
      index += 1;
    } else if (option.startsWith("-C") && option.length > 2) {
      directory = resolveDirectory(option.slice(2), directory);
    }
  }
  return directory;
}

/**
 * Evaluate a Claude Bash payload with injected effects. An unavailable identity
 * source is a block, never permission to guess.
 */
export async function guardClaudePayload(
  payload,
  { checkGit = checkLocalGit } = {},
) {
  if (!payload || payload.tool_name !== "Bash") {
    return { allowed: false, reason: "malformed PreToolUse payload: expected Bash tool_name" };
  }
  const command = payload?.tool_input?.command;
  if (typeof command !== "string" || command.length === 0) {
    return { allowed: false, reason: "malformed PreToolUse payload: expected a non-empty command" };
  }

  const commandEnvironment = command
    .split(/\s+/)
    .find((word) => IDENTITY_ENVIRONMENT.test(word.replace(/^[('"`]+/, "")));
  if (
    commandEnvironment &&
    (GIT_TEXT.test(command) || RAW_GH.test(command) || CHECKED_GITHUB_TEXT.test(command))
  ) {
    const name = commandEnvironment.replace(/^[('"`]+/, "").split("=")[0];
    return { allowed: false, reason: `command-local identity override is forbidden: ${name}` };
  }

  if (RAW_GH.test(command) && !EXACT_GH_REPAIR.test(command)) {
    return {
      allowed: false,
      reason: "raw gh is forbidden; use `just github …` so the credential-pinned wrapper owns execution",
    };
  }
  if (GIT_TEXT.test(command) && INDIRECT_EXECUTION.test(command)) {
    return {
      allowed: false,
      reason: "indirect/subshell Git or GitHub mutation syntax is forbidden; invoke the checked command directly",
    };
  }

  let cwd = payload.cwd && existsSync(payload.cwd) ? payload.cwd : process.cwd();
  for (const segment of splitShellSegments(command)) {
    const words = segment.trim().split(/\s+/).filter(Boolean);
    if (words[0] === "cd" && words[1] && !words[1].startsWith("-")) {
      cwd = resolveDirectory(words[1], cwd);
      continue;
    }

    const invocation = projectInvocation(segment);
    if (!invocation) continue;

    const overrideProblem = identityOverrideProblem(invocation);
    if (overrideProblem) return { allowed: false, reason: overrideProblem };

    if (invocation.executable === "gh") {
      const policy = githubAuthPolicy(invocation.args);
      if (policy.action === "block") return { allowed: false, reason: policy.reason };
      if (policy.action === "repair-switch") continue;
      return {
        allowed: false,
        reason: "raw gh is forbidden; use `just github …` so the credential-pinned wrapper owns execution",
      };
    }

    const subcommand = invocation.args[0];
    if (!GUARDED_GIT_COMMANDS.has(subcommand)) continue;
    try {
      const problems = checkGit(gitDirectory(invocation, cwd));
      if (problems.length > 0) return { allowed: false, reason: problems.join("; ") };
    } catch (error) {
      return { allowed: false, reason: `could not verify local Git identity: ${error.message}` };
    }
  }
  return { allowed: true };
}

async function main() {
  let payload;
  try {
    payload = JSON.parse(readFileSync(0, "utf8"));
  } catch {
    process.stderr.write("project-identity: malformed or missing PreToolUse payload; blocking fail-closed.\n");
    return 2;
  }
  const result = await guardClaudePayload(payload);
  if (result.allowed) return 0;
  process.stderr.write(
    `project-identity: ${result.reason}\n` +
      `Use ${PROJECT_GITHUB_LOGIN}, the account-specific origin, and ` +
      "`bun scripts/github.mjs …` for GitHub CLI operations.\n",
  );
  return 2;
}

if (import.meta.main) process.exit(await main());
