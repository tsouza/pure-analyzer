import { describe, expect, test } from "bun:test";
import { guardClaudePayload, projectInvocation } from "./claude-project-identity.mjs";

const payload = (command) => ({ tool_name: "Bash", tool_input: { command }, cwd: process.cwd() });

describe("Claude project identity guard", () => {
  test("finds wrapped and chained direct invocations", () => {
    expect(projectInvocation("env GH_HOST=github.com gh pr merge 12")?.executable).toBe("gh");
    expect(projectInvocation("git -C /tmp push origin HEAD")?.args[0]).toBe("push");
    expect(projectInvocation("git -C/tmp commit -m x")?.args[0]).toBe("commit");
    expect(projectInvocation("command -p git commit -m x")?.args[0]).toBe("commit");
    expect(projectInvocation("exec -a name git commit -m x")?.args[0]).toBe("commit");
    expect(projectInvocation("C:\\Tools\\gh.exe pr create")?.executable).toBe("gh");
  });

  test("blocks every raw gh operation in favor of the checked wrapper", async () => {
    const result = await guardClaudePayload(payload("git status && gh pr create"), {
      checkGit: () => [],
    });
    expect(result.allowed).toBeFalse();
    expect(result.reason).toContain("raw gh is forbidden");
  });

  test("blocks a wrong auth switch and allows the exact repair", async () => {
    const blocked = await guardClaudePayload(payload("gh auth switch --user tsouza-squid"), {
    });
    expect(blocked.allowed).toBeFalse();

    const repaired = await guardClaudePayload(payload("gh auth switch --user tsouza"), {
    });
    expect(repaired.allowed).toBeTrue();
  });

  test("blocks commit and push when the local identity checker fails", async () => {
    for (const command of [
      "git commit -m 'x'",
      "cd /tmp && git push origin HEAD",
      "command -p git commit -m x",
      "exec -a name git commit -m x",
    ]) {
      const result = await guardClaudePayload(payload(command), {
        checkGit: () => ["effective Git author is wrong"],
        readLogin: async () => "tsouza",
      });
      expect(result.allowed).toBeFalse();
    }
  });

  test("checks the repository selected by an attached git -C option", async () => {
    const checked = [];
    const result = await guardClaudePayload(payload("git -C/tmp commit -m x"), {
      checkGit: (directory) => {
        checked.push(directory);
        return ["effective Git author is wrong"];
      },
    });
    expect(result.allowed).toBeFalse();
    expect(checked).toEqual(["/tmp"]);
  });

  test("also blocks raw diagnostic gh commands", async () => {
    const result = await guardClaudePayload(payload("gh auth status"));
    expect(result.allowed).toBeFalse();
    expect(result.reason).toContain("raw gh is forbidden");
  });

  test.each([
    "GH_TOKEN=other gh issue create",
    "GH_CONFIG_DIR=/tmp/other gh pr merge 1",
    "HOME=/tmp git push origin HEAD",
    "XDG_CONFIG_HOME=/tmp git push origin HEAD",
    "HOME=/tmp just github issue create",
    "XDG_CONFIG_HOME=/tmp bun scripts/github.mjs pr create",
    "APPDATA=C:\\Temp git push origin HEAD",
    "HOMEDRIVE=Z: git push origin HEAD",
    "USERPROFILE=C:\\Temp git push origin HEAD",
    "GIT_SSH_COMMAND='ssh -i /tmp/squid' git push origin HEAD",
    "GIT_SSH=/tmp/fake-ssh git push origin HEAD",
    "GIT_SSH_VARIANT=simple git push origin HEAD",
    "GIT_PROXY_COMMAND=/tmp/fake-proxy git push origin HEAD",
    "GIT_ASKPASS=/tmp/fake-askpass git push origin HEAD",
    "SSH_AUTH_SOCK=/tmp/squid-agent git push origin HEAD",
    "SSH_ASKPASS=/tmp/fake-askpass git push origin HEAD",
    "GIT_AUTHOR_EMAIL=other@example.com git commit -m x",
    "git -c user.email=other@example.com commit -m x",
    "git -c include.path=/tmp/other-config commit -m x",
    "git --config-env=user.email=OTHER_EMAIL commit -m x",
    "git commit --author=Other --no-verify -m x",
    "git push --no-verify origin HEAD",
    "git --git-dir /tmp/other/.git commit -m x",
  ])("blocks command-local bypass syntax: %s", async (command) => {
    const result = await guardClaudePayload(payload(command), {
      checkGit: () => [],
      readLogin: async () => "tsouza",
    });
    expect(result.allowed).toBeFalse();
    expect(result.reason).toContain("forbidden");
  });

  test.each([
    "(cd /tmp && git commit -m x)",
    "sh -c 'gh issue create'",
    "bash -lc 'git push origin HEAD'",
    "value=$(gh issue create)",
    "sh -c 'git -c include.path=x commit -m x'",
    "/bin/sh -c 'gh issue create'",
    "env -i gh issue create",
    "sudo -u root gh issue create",
    "xargs gh pr create",
    "eval 'gh issue create'",
    "C:\\Tools\\gh.exe pr create",
  ])("blocks indirect mutation syntax: %s", async (command) => {
    const result = await guardClaudePayload(payload(command), {
      checkGit: () => [],
      readLogin: async () => "tsouza",
    });
    expect(result.allowed).toBeFalse();
    expect(result.reason).toContain("forbidden");
  });

  test.each([
    "/tmp/gh auth switch --user tsouza",
    "./gh auth switch --user tsouza",
    "/usr/local/bin/gh auth switch --user tsouza",
    "C:\\Tools\\gh.exe auth switch --user tsouza",
  ])("blocks a path-prefixed fake gh repair executable: %s", async (command) => {
    const result = await guardClaudePayload(payload(command), { checkGit: () => [] });
    expect(result.allowed).toBeFalse();
    expect(result.reason).toContain("raw gh is forbidden");
  });

  test.each(["merge", "cherry-pick", "rebase", "am", "revert", "tag"])(
    "checks identity before git %s",
    async (subcommand) => {
      const result = await guardClaudePayload(payload(`git ${subcommand} example`), {
        checkGit: () => ["wrong identity"],
        readLogin: async () => "tsouza",
      });
      expect(result.allowed).toBeFalse();
    },
  );

  test("does not intercept unrelated commands or the checked wrapper", async () => {
    expect((await guardClaudePayload(payload("cargo test"))).allowed).toBeTrue();
    expect((await guardClaudePayload(payload("bun scripts/github.mjs pr view 12"))).allowed).toBeTrue();
  });

  test("fails closed on semantically malformed matched payloads", async () => {
    expect((await guardClaudePayload({})).allowed).toBeFalse();
    expect((await guardClaudePayload({ tool_name: "Bash", tool_input: {} })).allowed).toBeFalse();
    expect(
      (await guardClaudePayload({ tool_name: "Bash", tool_input: { command: 42 } })).allowed,
    ).toBeFalse();
  });
});
