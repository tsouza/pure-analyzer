import { describe, expect, test } from "bun:test";
import {
  gitIdentityProblems,
  githubAuthPolicy,
  githubEventProblems,
  githubInvocationProblem,
  githubLoginProblem,
  parseGitIdent,
  parseCommitIdentities,
  PROJECT_GIT_REMOTE,
  pullRequestCommitProblems,
} from "./project-identity.mjs";

const expectedIdent = "Thiago Souza <122435+tsouza@users.noreply.github.com> 1787753472 +0000";
const validGit = {
  authorIdent: expectedIdent,
  committerIdent: expectedIdent,
  fetchRemotes: [PROJECT_GIT_REMOTE],
  pushRemotes: [PROJECT_GIT_REMOTE],
};

describe("local Git identity", () => {
  test("parses the effective Git identity", () => {
    expect(parseGitIdent(expectedIdent)).toEqual({
      name: "Thiago Souza",
      email: "122435+tsouza@users.noreply.github.com",
    });
  });

  test("accepts only the exact effective author, committer, and SSH alias", () => {
    expect(gitIdentityProblems(validGit)).toEqual([]);
    expect(
      gitIdentityProblems({
        ...validGit,
        authorIdent: "Thiago Souza <tsouza-squid@users.noreply.github.com> 1 +0000",
        pushRemotes: ["git@github.com:tsouza/pure-analyzer.git"],
      }),
    ).toEqual([
      'effective Git author email must be exactly "122435+tsouza@users.noreply.github.com"',
      `origin must have exactly one push URL: ${PROJECT_GIT_REMOTE}`,
    ]);
  });

  test("pre-push rejects another named remote or URL", () => {
    expect(
      gitIdentityProblems({
        ...validGit,
        hookRemoteName: "fork",
        hookRemoteUrl: "git@github.com-tsouza:tsouza-squid/pure-analyzer.git",
      }),
    ).toHaveLength(2);
  });

  test("rejects additional fetch or push URLs even when the first is correct", () => {
    expect(
      gitIdentityProblems({
        ...validGit,
        fetchRemotes: [PROJECT_GIT_REMOTE, "git@github.com:other/fork.git"],
        pushRemotes: [PROJECT_GIT_REMOTE, "git@github.com:other/fork.git"],
      }),
    ).toHaveLength(2);
  });

  test("rejects command-local Git SSH transport overrides", () => {
    expect(
      gitIdentityProblems({
        ...validGit,
        transportEnvironment: {
          GIT_SSH: "/tmp/fake-ssh",
          GIT_SSH_COMMAND: "ssh -i /tmp/squid-key",
        },
      }),
    ).toEqual([
      "Git transport override GIT_SSH is forbidden",
      "Git transport override GIT_SSH_COMMAND is forbidden",
    ]);
  });
});

describe("GitHub CLI identity", () => {
  test("requires the exact live login", () => {
    expect(githubLoginProblem("tsouza")).toBeNull();
    expect(githubLoginProblem("tsouza-squid")).toContain("must be exactly tsouza");
    expect(githubLoginProblem("TSOUZA")).toContain("must be exactly tsouza");
    expect(githubLoginProblem("")).toContain("did not return");
  });

  test("allows only an explicit switch to tsouza", () => {
    expect(githubAuthPolicy(["auth", "switch", "--user", "tsouza"]).action).toBe("repair-switch");
    expect(githubAuthPolicy(["auth", "switch", "-u", "tsouza-squid"]).action).toBe("block");
    expect(githubAuthPolicy(["auth", "switch"]).action).toBe("block");
    expect(githubAuthPolicy(["auth", "login"]).action).toBe("block");
    expect(
      githubAuthPolicy([
        "auth",
        "switch",
        "--user",
        "tsouza-squid",
        "--user",
        "tsouza",
      ]).action,
    ).toBe("block");
  });

  test("rejects Authorization/hostname overrides and extension commands", () => {
    expect(githubInvocationProblem(["api", "user"])).toBeNull();
    expect(
      githubInvocationProblem(["api", "-H", "authorization: Bearer another-token", "user"]),
    ).toContain("Authorization");
    expect(
      githubInvocationProblem(["api", "-H=Authorization: Bearer another-token", "user"]),
    ).toContain("Authorization");
    expect(githubInvocationProblem(["api", "--hostname=example.com", "user"])).toContain(
      "hostname",
    );
    expect(githubInvocationProblem(["auth", "status", "-h", "example.com"])).toContain(
      "hostname",
    );
    expect(githubInvocationProblem(["auth", "token", "-h=example.com"])).toContain(
      "hostname",
    );
    expect(githubInvocationProblem(["auth", "token", "-hexample.com"])).toContain("hostname");
    expect(githubInvocationProblem(["auth", "status", "--show-token"])).toContain("printing");
    expect(githubInvocationProblem(["auth", "status", "--show-token=true"])).toContain(
      "printing",
    );
    expect(githubInvocationProblem(["auth", "status", "-t"])).toContain("printing");
    expect(githubInvocationProblem(["auth", "status", "-t=true"])).toContain("printing");
    expect(githubInvocationProblem(["auth", "status", "-ttrue"])).toContain("printing");
    for (const cluster of ["-at", "-ta", "-at=true", "-ta=false", "-ait", "-tia"]) {
      expect(githubInvocationProblem(["auth", "status", cluster])).toContain("printing");
    }
    expect(githubInvocationProblem(["auth", "status", "-a"])).toBeNull();
    expect(githubInvocationProblem(["auth", "status", "-ahost"])).toContain("hostname");
    expect(
      githubInvocationProblem([
        "api",
        "-iHAuthorization: Bearer another-token",
        "user",
      ]),
    ).toContain("Authorization");
    expect(githubInvocationProblem(["third-party-extension", "write"])).toContain("extensions");
  });
});

describe("GitHub Actions event identity", () => {
  const context = (user) => ({
    eventName: "pull_request",
    actor: user.login,
    triggeringActor: user.login,
    payload: { sender: user, pull_request: { user } },
  });

  test("accepts the maintainer and genuine bots", () => {
    expect(githubEventProblems(context({ login: "tsouza", type: "User" }))).toEqual([]);
    expect(githubEventProblems(context({ login: "dependabot[bot]", type: "Bot" }))).toEqual([]);
  });

  test("rejects every other human and bot-shaped user accounts", () => {
    expect(githubEventProblems(context({ login: "tsouza-squid", type: "User" }))).not.toEqual([]);
    expect(githubEventProblems(context({ login: "another-human", type: "User" }))).not.toEqual([]);
    expect(githubEventProblems(context({ login: "dependabot[bot]", type: "User" }))).not.toEqual([]);
  });

  test("rejects every non-project human actor even outside a PR event", () => {
    const problems = githubEventProblems({
      eventName: "merge_group",
      actor: "tsouza-squid",
      triggeringActor: "tsouza",
      payload: { sender: { login: "tsouza", type: "User" } },
    });
    expect(problems[0]).toContain("workflow actor must be tsouza");

    expect(
      githubEventProblems({
        eventName: "push",
        actor: "tsouza-in-automode",
        triggeringActor: "tsouza",
        payload: { sender: { login: "tsouza", type: "User" } },
      }),
    ).toHaveLength(1);
  });

  test("allows reserved bot actors outside a PR event", () => {
    expect(
      githubEventProblems({
        eventName: "merge_group",
        actor: "github-merge-queue[bot]",
        triggeringActor: "dependabot[bot]",
        payload: { sender: { login: "github-merge-queue[bot]", type: "Bot" } },
      }),
    ).toEqual([]);
  });

  test("rejects a foreign human actor even when the PR author is a bot", () => {
    expect(
      githubEventProblems({
        eventName: "pull_request",
        actor: "another-human",
        triggeringActor: "another-human",
        payload: {
          sender: { login: "another-human", type: "User" },
          pull_request: { user: { login: "dependabot[bot]", type: "Bot" } },
        },
      }),
    ).toHaveLength(3);
  });

  test("fails closed when required actor or sender context is missing", () => {
    expect(
      githubEventProblems({
        eventName: "push",
        actor: "",
        triggeringActor: undefined,
        payload: {},
      }),
    ).toEqual([
      "workflow actor is missing from the GitHub event context",
      "event sender is missing from the GitHub event context",
    ]);
  });
});

describe("pull-request commit identities", () => {
  const human = {
    oid: "a".repeat(40),
    authorName: "Thiago Souza",
    authorEmail: "122435+tsouza@users.noreply.github.com",
    committerName: "Thiago Souza",
    committerEmail: "122435+tsouza@users.noreply.github.com",
  };
  const dependabot = {
    oid: "b".repeat(40),
    authorName: "dependabot[bot]",
    authorEmail: "49699333+dependabot[bot]@users.noreply.github.com",
    committerName: "GitHub",
    committerEmail: "noreply@github.com",
  };

  test("parses NUL-delimited git log records", () => {
    const raw = Object.values(human).join("\0") + "\0";
    expect(parseCommitIdentities(raw)).toEqual([human]);
  });

  test("accepts exact human commits only on a tsouza PR", () => {
    const pr = { login: "tsouza", type: "User" };
    expect(pullRequestCommitProblems([human], pr)).toEqual([]);
    expect(pullRequestCommitProblems([dependabot], pr)).toHaveLength(1);
  });

  test("accepts a matching bot noreply author and GitHub committer on a bot PR", () => {
    const pr = { login: "dependabot[bot]", type: "Bot" };
    expect(pullRequestCommitProblems([dependabot], pr)).toEqual([]);
    expect(
      pullRequestCommitProblems(
        [{ ...dependabot, authorEmail: "dependabot[bot]@example.com" }],
        pr,
      ),
    ).toHaveLength(1);
  });
});
