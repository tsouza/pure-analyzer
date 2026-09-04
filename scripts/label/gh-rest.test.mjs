import { afterEach, describe, expect, test } from "bun:test";

import {
  addIssueComment,
  addLabels,
  createIssue,
  DEFAULT_API_URL,
  ghJSON,
  listOpenIssues,
  listOpenPullRequests,
} from "./gh-rest.mjs";

const originalFetch = globalThis.fetch;

afterEach(() => {
  globalThis.fetch = originalFetch;
});

/** Install a fake fetch that resolves each call from `handler(url, init)`. */
function stubFetch(handler) {
  globalThis.fetch = async (url, init) => handler(url, init);
}

describe("ghJSON", () => {
  test("sends the expected auth + version headers and parses JSON", async () => {
    let seenUrl;
    let seenHeaders;
    stubFetch(async (url, init) => {
      seenUrl = url;
      seenHeaders = init.headers;
      return new Response(JSON.stringify({ ok: true }), { status: 200 });
    });
    const body = await ghJSON("https://api.github.com/x", "tok");
    expect(body).toEqual({ ok: true });
    expect(seenUrl).toBe("https://api.github.com/x");
    expect(seenHeaders.authorization).toBe("Bearer tok");
    expect(seenHeaders["x-github-api-version"]).toBe("2022-11-28");
  });

  test("returns null for a 204", async () => {
    stubFetch(async () => new Response(null, { status: 204 }));
    expect(await ghJSON("https://api.github.com/x", "tok")).toBeNull();
  });

  test("throws with the status and body on a non-2xx response", async () => {
    stubFetch(async () => new Response("nope", { status: 404, statusText: "Not Found" }));
    await expect(ghJSON("https://api.github.com/x", "tok")).rejects.toThrow(/404 Not Found: nope/);
  });
});

describe("listOpenIssues", () => {
  test("paginates, drops pull requests, and stops on a short page", async () => {
    const page1 = Array.from({ length: 100 }, (_, i) => ({ number: i + 1 }));
    page1[0].pull_request = {}; // GitHub folds PRs into /issues; must be dropped.
    const page2 = [{ number: 101 }];
    let call = 0;
    stubFetch(async (url) => {
      call++;
      expect(String(url)).toContain("state=open");
      const body = String(url).includes("page=2") ? page2 : page1;
      return new Response(JSON.stringify(body), { status: 200 });
    });
    const issues = await listOpenIssues(DEFAULT_API_URL, "o/r", "tok");
    expect(call).toBe(2);
    expect(issues.length).toBe(100); // 99 real issues from page1 + 1 from page2
    expect(issues.some((i) => i.pull_request)).toBe(false);
  });

  test("throws on an unexpected non-array response", async () => {
    stubFetch(async () => new Response(JSON.stringify({ message: "bad credentials" }), { status: 200 }));
    await expect(listOpenIssues(DEFAULT_API_URL, "o/r", "tok")).rejects.toThrow(/unexpected non-array/);
  });
});

describe("listOpenPullRequests", () => {
  test("paginates and stops on a short page", async () => {
    const page1 = Array.from({ length: 100 }, (_, i) => ({ number: i + 1 }));
    const page2 = [{ number: 101 }];
    stubFetch(async (url) => {
      const body = String(url).includes("page=2") ? page2 : page1;
      return new Response(JSON.stringify(body), { status: 200 });
    });
    const prs = await listOpenPullRequests(DEFAULT_API_URL, "o/r", "tok");
    expect(prs.length).toBe(101);
  });
});

describe("addLabels", () => {
  test("POSTs the labels array to the shared issues/labels endpoint", async () => {
    let seenUrl;
    let seenMethod;
    let seenBody;
    stubFetch(async (url, init) => {
      seenUrl = url;
      seenMethod = init.method;
      seenBody = JSON.parse(init.body);
      return new Response(JSON.stringify([]), { status: 200 });
    });
    await addLabels(DEFAULT_API_URL, "o/r", "tok", 42, ["bug", "analyzer"]);
    expect(seenUrl).toBe("https://api.github.com/repos/o/r/issues/42/labels");
    expect(seenMethod).toBe("POST");
    expect(seenBody).toEqual({ labels: ["bug", "analyzer"] });
  });
});

describe("createIssue", () => {
  test("POSTs title/body/labels to the issues endpoint and returns the created issue", async () => {
    let seenUrl;
    let seenMethod;
    let seenBody;
    stubFetch(async (url, init) => {
      seenUrl = url;
      seenMethod = init.method;
      seenBody = JSON.parse(init.body);
      return new Response(JSON.stringify({ number: 999, html_url: "https://x/999" }), { status: 201 });
    });
    const issue = await createIssue(DEFAULT_API_URL, "o/r", "tok", {
      title: "t",
      body: "b",
      labels: ["l"],
    });
    expect(seenUrl).toBe("https://api.github.com/repos/o/r/issues");
    expect(seenMethod).toBe("POST");
    expect(seenBody).toEqual({ title: "t", body: "b", labels: ["l"] });
    expect(issue).toEqual({ number: 999, html_url: "https://x/999" });
  });
});

describe("addIssueComment", () => {
  test("POSTs a comment body to the issue's comments endpoint and returns it", async () => {
    let seenUrl;
    let seenMethod;
    let seenBody;
    stubFetch(async (url, init) => {
      seenUrl = url;
      seenMethod = init.method;
      seenBody = JSON.parse(init.body);
      return new Response(JSON.stringify({ id: 1, body: "c" }), { status: 201 });
    });
    const comment = await addIssueComment(DEFAULT_API_URL, "o/r", "tok", 42, "c");
    expect(seenUrl).toBe("https://api.github.com/repos/o/r/issues/42/comments");
    expect(seenMethod).toBe("POST");
    expect(seenBody).toEqual({ body: "c" });
    expect(comment).toEqual({ id: 1, body: "c" });
  });
});
