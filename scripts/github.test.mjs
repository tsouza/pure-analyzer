import { describe, expect, test } from "bun:test";
import { githubEnvironment, runGuardedGithub } from "./github.mjs";

describe("runGuardedGithub", () => {
  const credential = Object.freeze({ token: "tsouza-token", host: "github.com" });

  test("pins credentials and ignores token, host, and config-location overrides", () => {
    const environment = githubEnvironment(credential, {
      PATH: "/usr/bin",
      HOME: "/tmp/redirected-home",
      USERPROFILE: "C:\\Temp\\redirected-profile",
      APPDATA: "C:\\Temp\\redirected-appdata",
      LOCALAPPDATA: "C:\\Temp\\redirected-local-appdata",
      XDG_CONFIG_HOME: "/tmp/redirected-config",
      XDG_CONFIG_DIRS: "/tmp/redirected-config-dirs",
      GH_CONFIG_DIR: "/tmp/redirected-gh",
      GH_HOST: "example.com",
      GH_TOKEN: "squid-token",
      GITHUB_TOKEN: "squid-token",
    });
    expect(environment.PATH).toBe("/usr/bin");
    expect(environment.HOME).not.toBe("/tmp/redirected-home");
    expect(environment.GH_HOST).toBe("github.com");
    expect(environment.GH_TOKEN).toBe(credential.token);
    for (const name of ["XDG_CONFIG_HOME", "XDG_CONFIG_DIRS", "GH_CONFIG_DIR", "GITHUB_TOKEN"]) {
      expect(environment[name]).toBeUndefined();
    }
    if (process.platform !== "win32") {
      expect(environment.USERPROFILE).toBeUndefined();
      expect(environment.APPDATA).toBeUndefined();
      expect(environment.LOCALAPPDATA).toBeUndefined();
    }
  });

  test("verifies and executes with one pinned tsouza credential", async () => {
    const calls = [];
    const result = await runGuardedGithub(["pr", "create"], {
      acquireCredential: async () => {
        calls.push("acquire");
        return credential;
      },
      verifyCredential: async (received) => {
        calls.push(["verify", received]);
        return "tsouza\n";
      },
      execute: async (args, received) => {
        calls.push(["execute", args, received]);
        return 0;
      },
    });
    expect(result).toBe(0);
    expect(calls).toEqual([
      "acquire",
      ["verify", credential],
      ["execute", ["pr", "create"], credential],
    ]);
  });

  test("an active-account race cannot alter the execution credential", async () => {
    let activeAccount = "tsouza";
    const seen = [];
    await runGuardedGithub(["issue", "create"], {
      acquireCredential: async () => credential,
      verifyCredential: async (received) => {
        seen.push(received);
        activeAccount = "tsouza-squid";
        return "tsouza";
      },
      execute: async (_args, received) => {
        seen.push(received);
        expect(activeAccount).toBe("tsouza-squid");
        return 0;
      },
    });
    expect(seen).toEqual([credential, credential]);
  });

  test("never executes under a forbidden, malformed, or unavailable credential", async () => {
    let executed = false;
    for (const scenario of [
      {
        acquireCredential: async () => credential,
        verifyCredential: async () => "tsouza-squid",
      },
      {
        acquireCredential: async () => ({ token: "tsouza-token", host: "example.com" }),
        verifyCredential: async () => "tsouza",
      },
      {
        acquireCredential: async () => credential,
        verifyCredential: async () => {
          throw new Error("network unavailable");
        },
      },
      {
        acquireCredential: async () => {
          throw new Error("credential unavailable");
        },
        verifyCredential: async () => "tsouza",
      },
    ]) {
      await expect(
        runGuardedGithub(["issue", "create"], {
          ...scenario,
          execute: async () => {
            executed = true;
            return 0;
          },
        }),
      ).rejects.toThrow();
    }
    expect(executed).toBeFalse();
  });

  test("never includes the pinned token in identity failures", async () => {
    for (const verifyCredential of [
      async () => "tsouza-squid",
      async () => {
        throw new Error("network unavailable");
      },
    ]) {
      try {
        await runGuardedGithub(["issue", "create"], {
          acquireCredential: async () => credential,
          verifyCredential,
          execute: async () => 0,
        });
        throw new Error("expected identity guard failure");
      } catch (error) {
        expect(error.message).not.toContain(credential.token);
      }
    }
  });

  test("rejects explicit credentials before acquisition or execution", async () => {
    let effects = 0;
    for (const args of [
      ["api", "-H", "Authorization: Bearer squid-token", "repos/x/y/issues"],
      ["api", "-H=Authorization: Bearer squid-token", "repos/x/y/issues"],
    ]) {
      await expect(
        runGuardedGithub(args, {
          acquireCredential: async () => {
            effects += 1;
            return credential;
          },
          verifyCredential: async () => {
            effects += 1;
            return "tsouza";
          },
          execute: async () => {
            effects += 1;
            return 0;
          },
        }),
      ).rejects.toThrow("Authorization headers are forbidden");
    }
    expect(effects).toBe(0);
  });

  test("rejects every token-display shorthand cluster before any effect", async () => {
    let effects = 0;
    for (const flag of [
      "--show-token=true",
      "-t",
      "-t=true",
      "-tfalse",
      "-at",
      "-ta",
      "-at=true",
      "-ta=false",
      "-ait",
      "-tia",
    ]) {
      await expect(
        runGuardedGithub(["auth", "status", flag], {
          acquireCredential: async () => {
            effects += 1;
            return credential;
          },
          verifyCredential: async () => {
            effects += 1;
            return "tsouza";
          },
          execute: async () => {
            effects += 1;
            return 0;
          },
        }),
      ).rejects.toThrow("printing GitHub authentication tokens is forbidden");
    }
    expect(effects).toBe(0);
  });

  test("rejects extensions before credential acquisition or execution", async () => {
    await expect(
      runGuardedGithub(["third-party-extension", "write"], {
        acquireCredential: async () => credential,
        verifyCredential: async () => "tsouza",
        execute: async () => 0,
      }),
    ).rejects.toThrow("aliases and extensions are forbidden");
  });

  test("permits only the exact repair switch without credential acquisition", async () => {
    let acquisitions = 0;
    const execute = async (_args, received) => {
      expect(received).toBeNull();
      return 0;
    };
    expect(
      await runGuardedGithub(["auth", "switch", "--user", "tsouza"], {
        acquireCredential: async () => {
          acquisitions += 1;
          return credential;
        },
        verifyCredential: async () => "tsouza",
        execute,
      }),
    ).toBe(0);
    expect(acquisitions).toBe(0);
    await expect(
      runGuardedGithub(["auth", "switch", "--user", "tsouza-squid"], {
        acquireCredential: async () => credential,
        verifyCredential: async () => "tsouza",
        execute,
      }),
    ).rejects.toThrow("must name --user tsouza");
  });
});
