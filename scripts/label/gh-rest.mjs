// gh-rest.mjs — minimal GitHub REST client shared by issue-label.mjs and
// pr-label.mjs. Dependency-light by design: `node:` builtins + Bun's global
// `fetch` only, so a bare `ubuntu-latest` runner needs no `actions/github-
// script` step and no npm install — a Bun `run:` line is the whole job.
//
// Kept to exactly the four operations both labelers need (list open issues,
// list open PRs, add labels, low-level JSON GET/POST) so the two scripts
// never carry two copies of the same fetch-with-auth-headers boilerplate.

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
