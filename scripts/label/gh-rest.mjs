// gh-rest.mjs — minimal GitHub REST client shared by issue-label.mjs,
// pr-label.mjs, and mutation-nightly-failure.mjs. Dependency-light by
// design: `node:` builtins + Bun's global `fetch` only, so a bare
// `ubuntu-latest` runner needs no `actions/github-script` step and no npm
// install — a Bun `run:` line is the whole job.
//
// Kept to exactly the operations its callers need (list open issues, list
// open PRs, add labels, create an issue, comment on one, low-level JSON
// GET/POST) so no script carries its own copy of the same
// fetch-with-auth-headers boilerplate.

export const DEFAULT_API_URL = "https://api.github.com";
const PER_PAGE = 100;

function apiHeaders(token) {
  return {
    accept: "application/vnd.github+json",
    authorization: `Bearer ${token}`,
    "x-github-api-version": "2022-11-28",
    "user-agent": "pure-analyzer-issue-pr-labeler",
  };
}

/** GET/POST `url` as GitHub REST JSON, throwing on a non-2xx response. */
export async function ghJSON(url, token, init = {}) {
  const res = await fetch(url, { ...init, headers: { ...apiHeaders(token), ...(init.headers ?? {}) } });
  if (!res.ok) {
    throw new Error(`${init.method ?? "GET"} ${url} -> ${res.status} ${res.statusText}: ${await res.text()}`);
  }
  return res.status === 204 ? null : res.json();
}

/** Paginate `/repos/{repo}/issues?state=open`, dropping the PRs GitHub folds into that endpoint. */
export async function listOpenIssues(api, repo, token) {
  const out = [];
  for (let page = 1; ; page++) {
    const url = `${api}/repos/${repo}/issues?state=open&per_page=${PER_PAGE}&page=${page}`;
    const batch = await ghJSON(url, token);
    if (!Array.isArray(batch)) throw new Error(`unexpected non-array response from ${url}`);
    for (const it of batch) {
      if (it.pull_request) continue;
      out.push(it);
    }
    if (batch.length < PER_PAGE) break;
  }
  return out;
}

/** Paginate `/repos/{repo}/pulls?state=open`. */
export async function listOpenPullRequests(api, repo, token) {
  const out = [];
  for (let page = 1; ; page++) {
    const url = `${api}/repos/${repo}/pulls?state=open&per_page=${PER_PAGE}&page=${page}`;
    const batch = await ghJSON(url, token);
    if (!Array.isArray(batch)) throw new Error(`unexpected non-array response from ${url}`);
    out.push(...batch);
    if (batch.length < PER_PAGE) break;
  }
  return out;
}

/** POST `labels` onto issue/PR `number` (the labels endpoint is shared by both). */
export async function addLabels(api, repo, token, number, labels) {
  await ghJSON(`${api}/repos/${repo}/issues/${number}/labels`, token, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ labels }),
  });
}

/** Create a new issue and return it. */
export async function createIssue(api, repo, token, { title, body, labels }) {
  return ghJSON(`${api}/repos/${repo}/issues`, token, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ title, body, labels }),
  });
}

/** Post a comment onto issue/PR `number` and return it. */
export async function addIssueComment(api, repo, token, number, body) {
  return ghJSON(`${api}/repos/${repo}/issues/${number}/comments`, token, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ body }),
  });
}
