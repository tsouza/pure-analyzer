// Shared, side-effect-free identity policy for local hooks, Claude Code, and CI.

export const PROJECT_GITHUB_LOGIN = "tsouza";
export const FORBIDDEN_GITHUB_LOGIN = "tsouza-squid";
export const PROJECT_GIT_AUTHOR_NAME = "Thiago Souza";
export const PROJECT_GIT_AUTHOR_EMAIL = "122435+tsouza@users.noreply.github.com";
export const PROJECT_GIT_REMOTE = "git@github.com-tsouza:tsouza/pure-analyzer.git";
export const GIT_TRANSPORT_OVERRIDE_NAMES = Object.freeze([
  "GIT_SSH",
  "GIT_SSH_COMMAND",
  "GIT_SSH_VARIANT",
  "GIT_PROXY_COMMAND",
]);

const IDENT = /^(.*) <([^<>]+)> \d+ [+-]\d{4}$/;
const BOT_LOGIN = /\[bot\]$/;
const BOT_NOREPLY = /^(\d+)\+(.+\[bot\])@users\.noreply\.github\.com$/;

/** Parse `git var GIT_*_IDENT` without depending on the timestamp. */
export function parseGitIdent(value) {
  const match = IDENT.exec(value.trim());
  return match ? { name: match[1], email: match[2] } : null;
}

function checkIdent(label, value) {
  const ident = parseGitIdent(value);
  if (!ident) return [`${label} is not a valid effective Git identity`];

  const problems = [];
  if (ident.name !== PROJECT_GIT_AUTHOR_NAME) {
    problems.push(`${label} name must be exactly ${JSON.stringify(PROJECT_GIT_AUTHOR_NAME)}`);
  }
  if (ident.email !== PROJECT_GIT_AUTHOR_EMAIL) {
    problems.push(`${label} email must be exactly ${JSON.stringify(PROJECT_GIT_AUTHOR_EMAIL)}`);
  }
  return problems;
}

/** Return every local identity violation; an empty result is permission to proceed. */
export function gitIdentityProblems({
  authorIdent,
  committerIdent,
  fetchRemotes,
  pushRemotes,
  hookRemoteName,
  hookRemoteUrl,
  transportEnvironment = {},
}) {
  const problems = [
    ...checkIdent("effective Git author", authorIdent),
    ...checkIdent("effective Git committer", committerIdent),
  ];

  const fetchUrls = Array.isArray(fetchRemotes) ? fetchRemotes : [fetchRemotes];
  const pushUrls = Array.isArray(pushRemotes) ? pushRemotes : [pushRemotes];
  if (fetchUrls.length !== 1 || fetchUrls[0] !== PROJECT_GIT_REMOTE) {
    problems.push(`origin must have exactly one fetch URL: ${PROJECT_GIT_REMOTE}`);
  }
  if (pushUrls.length !== 1 || pushUrls[0] !== PROJECT_GIT_REMOTE) {
    problems.push(`origin must have exactly one push URL: ${PROJECT_GIT_REMOTE}`);
  }
  if (hookRemoteName !== undefined && hookRemoteName !== "origin") {
    problems.push("pushes must use the origin remote");
  }
  if (hookRemoteUrl !== undefined && hookRemoteUrl !== PROJECT_GIT_REMOTE) {
    problems.push(`the active push URL must be exactly ${PROJECT_GIT_REMOTE}`);
  }
  for (const name of GIT_TRANSPORT_OVERRIDE_NAMES) {
    if (Object.hasOwn(transportEnvironment, name)) {
      problems.push(`Git transport override ${name} is forbidden`);
    }
  }
  return problems;
}

/** Return a failure message unless a pinned GitHub token resolves exactly to `tsouza`. */
export function githubLoginProblem(login) {
  if (login === PROJECT_GITHUB_LOGIN) return null;
  if (!login) return "GitHub CLI did not return an authenticated login";
  return `GitHub CLI login must be exactly ${PROJECT_GITHUB_LOGIN}; got ${login}`;
}

/**
 * Classify an auth command before any credential check. Switching explicitly to
 * `tsouza` is the only repair operation allowed while another account is active.
 */
export function githubAuthPolicy(args) {
  if (args[0] !== "auth") return { action: "check-login" };
  if (args[1] === "status") return { action: "diagnostic" };
  if (args[1] !== "switch") {
    return {
      action: "block",
      reason: "GitHub authentication may only be changed with `gh auth switch --user tsouza`",
    };
  }

  const exactRepair =
    (args.length === 4 &&
      (args[2] === "--user" || args[2] === "-u") &&
      args[3] === PROJECT_GITHUB_LOGIN) ||
    (args.length === 3 && args[2] === `--user=${PROJECT_GITHUB_LOGIN}`);
  if (exactRepair) return { action: "repair-switch" };
  return {
    action: "block",
    reason: `GitHub auth switch must name --user ${PROJECT_GITHUB_LOGIN} exactly`,
  };
}

const CORE_GH_COMMANDS = new Set([
  "api",
  "attestation",
  "auth",
  "browse",
  "cache",
  "codespace",
  "completion",
  "config",
  "gist",
  "help",
  "issue",
  "label",
  "org",
  "pr",
  "project",
  "release",
  "repo",
  "ruleset",
  "run",
  "search",
  "secret",
  "ssh-key",
  "status",
  "variable",
  "workflow",
]);

/** Reject credentials/hosts that would escape the wrapper's pinned boundary. */
export function githubInvocationProblem(args) {
  if (!CORE_GH_COMMANDS.has(args[0])) {
    return `unsupported GitHub CLI command ${JSON.stringify(args[0])}; aliases and extensions are forbidden`;
  }
  for (let index = 0; index < args.length; index += 1) {
    const arg = args[index];
    const lower = arg.toLowerCase();
    const shortFlags = arg.startsWith("-") && !arg.startsWith("--")
      ? arg.slice(1).split("=", 1)[0]
      : "";
    if (lower === "--hostname" || lower.startsWith("--hostname=")) {
      return "explicit GitHub hostname overrides are forbidden";
    }
    if (args[0] === "auth" && shortFlags.includes("h")) {
      return "explicit GitHub hostname overrides are forbidden";
    }
    if (
      args[0] === "auth" &&
      (shortFlags.includes("t") ||
        lower === "--show-token" ||
        lower.startsWith("--show-token="))
    ) {
      return "printing GitHub authentication tokens is forbidden";
    }

    let header = null;
    const shortHeaderAt = shortFlags.indexOf("H");
    if (lower === "--header") {
      header = args[index + 1] ?? "";
      index += 1;
    } else if (lower.startsWith("--header=")) {
      header = arg.slice("--header=".length);
    } else if (shortHeaderAt !== -1) {
      header = arg.slice(shortHeaderAt + 2);
      if (header.startsWith("=")) header = header.slice(1);
      if (!header) {
        header = args[index + 1] ?? "";
        index += 1;
      }
    }
    if (/^\s*authorization\s*:/i.test(header ?? "")) {
      return "explicit Authorization headers are forbidden";
    }
  }
  return null;
}

function isGenuineBot(user) {
  return user?.type === "Bot" && BOT_LOGIN.test(user.login ?? "");
}

function actorProblem(label, login, userType, required) {
  if (!login) return required ? `${label} is missing from the GitHub event context` : null;
  if (login === PROJECT_GITHUB_LOGIN && (userType === undefined || userType === "User")) return null;
  if (BOT_LOGIN.test(login) && (userType === undefined || userType === "Bot")) return null;
  return `${label} must be ${PROJECT_GITHUB_LOGIN} or a genuine [bot] identity; got ${login}`;
}

/** Validate the identities exposed by a GitHub Actions event without an API call. */
export function githubEventProblems({ eventName, actor, triggeringActor, payload }) {
  const problems = [];
  const observed = [
    ["workflow actor", actor, undefined, true],
    ["triggering actor", triggeringActor, undefined, false],
    ["event sender", payload?.sender?.login, payload?.sender?.type, true],
  ];
  for (const [label, login, type, required] of observed) {
    const problem = actorProblem(label, login, type, required);
    if (problem) problems.push(problem);
  }

  if (eventName === "pull_request" || eventName === "pull_request_target") {
    const author = payload?.pull_request?.user;
    if (author?.login === PROJECT_GITHUB_LOGIN && author?.type === "User") return problems;
    if (isGenuineBot(author)) return problems;
    const rendered = author?.login ? `${author.login} (${author.type ?? "unknown type"})` : "missing";
    problems.push(
      `human pull-request author must be ${PROJECT_GITHUB_LOGIN}; genuine GitHub bots remain allowed (got ${rendered})`,
    );
  }
  return problems;
}

function isExpectedHumanIdentity(name, email) {
  return name === PROJECT_GIT_AUTHOR_NAME && email === PROJECT_GIT_AUTHOR_EMAIL;
}

function isMatchingBotIdentity(name, email, expectedLogin) {
  const match = BOT_NOREPLY.exec(email);
  return match !== null && name === expectedLogin && match[2] === expectedLogin;
}

/** Parse NUL-delimited `%H,%an,%ae,%cn,%ce` records emitted by `git log -z`. */
export function parseCommitIdentities(value) {
  const fields = value.split("\0");
  if (fields.at(-1) === "") fields.pop();
  if (fields.length % 5 !== 0) throw new Error("malformed Git commit identity records");

  const commits = [];
  for (let index = 0; index < fields.length; index += 5) {
    commits.push({
      oid: fields[index],
      authorName: fields[index + 1],
      authorEmail: fields[index + 2],
      committerName: fields[index + 3],
      committerEmail: fields[index + 4],
    });
  }
  return commits;
}

/** Validate identities for commits introduced by one pull request. */
export function pullRequestCommitProblems(commits, prAuthor) {
  if (commits.length === 0) return ["pull request introduces no inspectable commits"];
  const botPr = isGenuineBot(prAuthor);
  const problems = [];

  for (const commit of commits) {
    const humanAuthor = isExpectedHumanIdentity(commit.authorName, commit.authorEmail);
    const humanCommitter = isExpectedHumanIdentity(commit.committerName, commit.committerEmail);
    if (!botPr && humanAuthor && humanCommitter) continue;

    const botAuthor = botPr && isMatchingBotIdentity(commit.authorName, commit.authorEmail, prAuthor.login);
    const botCommitter =
      botPr &&
      (isMatchingBotIdentity(commit.committerName, commit.committerEmail, prAuthor.login) ||
        (commit.committerName === "GitHub" && commit.committerEmail === "noreply@github.com"));
    if (botAuthor && botCommitter) continue;

    problems.push(
      `${commit.oid}: author ${commit.authorName} <${commit.authorEmail}> and committer ` +
        `${commit.committerName} <${commit.committerEmail}> do not match the PR identity policy`,
    );
  }
  return problems;
}
