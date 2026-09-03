#!/usr/bin/env bun
// issue-label.mjs — deterministic issue labeler for `issue-label.yml`.
//
// This repo's PRs are labeled two independent ways already: `labeler.yml`
// (path-based, `actions/labeler`) and this PR's new `pr-label.mjs` (title-
// based, Conventional-Commit type). ISSUES had NEITHER: `actions/labeler` is
// PR-only, so every issue — #366-#369 included — sat with zero labels no
// matter how the repo's label taxonomy grew. This module is the issue-side
// counterpart, ported from the sibling `cerberus` repo's
// `.github/scripts/issue-label.mjs` (see that file for the pattern this
// adapts) and reworked around THIS repo's own label taxonomy and evidence.
//
// Five independent inference passes run over every issue, each described in
// depth at its own definition below:
//
//   1. AREA   — purecard / analyzer (the product split) plus a corpus /
//               python / security / ci overlay, from file paths cited in the
//               body and a bare product-name mention in the title. The path
//               table intentionally reuses `.github/labeler.yml`'s own glob
//               boundaries for the overlay categories so the PR path-labeler
//               and this issue path-inference can never drift onto two
//               different ideas of what counts as "corpus" or "security".
//   2. TYPE   — a Conventional-Commit title prefix, delegated to
//               `type-label.mjs` (the same table `pr-label.mjs` uses) so
//               there is exactly one CC->label mapping in the repo; falling
//               back to a curated `bug:` / `flake:` title-prefix rule for
//               this repo's own bug-report convention (a bare CC type token
//               cannot express "bug", since it isn't a CC type — and per
//               constitution §3, "flakes are bugs").
//   3. DIMENSION — this repo's `dimension:*` labels tag which of the
//               downstream NL->Pure consumer's evidence dimensions an issue
//               speaks to. See DIMENSION_ANCHOR / L2_OVERLAY_SIGNAL /
//               PARSE_REJECT_SIGNAL below for the real, investigated
//               evidence behind each signal — and what was deliberately left
//               unautomated.
//   4. CONSUMER-REQUEST — whether the issue originates from the downstream
//               consumer's dated work request. See CONSUMER_REQUEST_SIGNAL.
//   5. DECISION-NEEDED — this repo's own "MAINTAINER DECISION NEEDED" title
//               convention (#334, #335).
//
// All five are PURE, DETERMINISTIC functions of (title, body) — no LLM, no
// network guess, no heuristic depending on wall-clock or issue ordering. The
// same issue always yields the same labels.
//
// The apply step is ADDITIVE and IDEMPOTENT: it only ever POSTs labels the
// issue is missing, never removes or replaces one a human set, and caps the
// AREA and TYPE totals (MAX_AREA_LABELS / MAX_TYPE_LABELS) counting labels
// already present. Re-running on an already-labeled issue is a no-op.
//
// ANTI-VACUITY. A labeler that silently labels nothing is worse than none:
//   - the mapping tables are non-empty at module load (assertTablesUsable);
//   - every issue processed must have had its body actually fetched (a
//     missing `body` KEY is a fetch failure and fails the run; a genuinely
//     empty body is reported, not silently treated as "no signal");
//   - in backfill mode the run FAILS if it processed zero issues, or if it
//     applied zero labels while unlabeled issues remain;
//   - every issue that the rules cannot classify at all (no area, no type,
//     no dimension, not a consumer-request, not decision-needed) is reported
//     by number in an ::error:: — never silently skipped.
//
// ENV CONTRACT
//   GITHUB_TOKEN      (required, unless ISSUE_LABEL_DRY_RUN=1 with a fixture)
//                      — token with `issues: write`.
//   GITHUB_REPOSITORY (required) — `owner/repo`.
//   ISSUE_LABEL_MODE  (required) — `event` | `backfill`.
//                       event    : label the single issue in the webhook
//                                  payload at $GITHUB_EVENT_PATH.
//                       backfill : walk every OPEN issue and apply missing
//                                  labels.
//   GITHUB_EVENT_PATH (required in `event` mode) — webhook payload JSON.
//   ISSUE_LABEL_DRY_RUN (optional) — `1`/`true` computes and reports the
//                       labels but applies nothing.
//   ISSUE_LABEL_FIXTURE (optional, dry-run only) — path to a JSON array of
//                       `{number, title, body, labels}` to read INSTEAD of
//                       the API, so a dry run reproduces offline.
//   GITHUB_API_URL    (optional) — API base; defaults to https://api.github.com.
//
// Exits 0 on success, 1 on any failure (unclassifiable issue, vacuous run,
// API error, unfetched body).
//
// argv `--check-tables` runs the mapping-table assertion and exits. Not
// spelled `--self-test` like `type-label.mjs`: importing that module runs
// its own top level, whose `--self-test` branch would consume the flag and
// exit before any of this file's code ran.

import { readFileSync } from "node:fs";
import process from "node:process";

import { die, error, notice } from "../lib/ci.mjs";
import { addLabels, DEFAULT_API_URL, listOpenIssues } from "./gh-rest.mjs";
import { labelsForTitle, TYPE_TO_LABEL } from "./type-label.mjs";

// ---------------------------------------------------------------------------
// AREA
// ---------------------------------------------------------------------------

// Repository subtree -> label, by LONGEST matching prefix. Two things this
// table intentionally does NOT reinvent:
//
//   - the corpus / python / security / ci glob boundaries are the exact same
//     ones `.github/labeler.yml` already uses for its PR path-labeler (see
//     that file) — reusing them means an issue and a PR that cite the same
//     path are always classified the same way;
//   - `docs/` is deliberately unmapped, same call cerberus made for its own
//     `docs/`: the `documentation` TYPE label (via a `docs:` CC title)
//     already carries that, and a doc path is usually cited as evidence
//     about some OTHER area's code, not the area itself.
//
// `mise.lock` is cited in both the `ci` and `security` glob sets in
// `.github/labeler.yml` (a real PR gets both labels independently there); a
// single prefix->label table can only hold one value per key, so it is
// filed here under the narrower, rarer `ci` reading.
export const PATH_PREFIX_TO_LABEL = Object.freeze({
  // PureCARD sub-areas — most specific first, so the longest-prefix scan
  // below picks e.g. `corpus` over the broader `purecard` for a corpus path.
  "crates/pure-analyzer-purecard/corpus": "corpus",
  "crates/pure-analyzer-purecard/python": "python",
  "crates/pure-analyzer-purecard/src/ffi.rs": "python",
  "crates/pure-analyzer-purecard/pyproject.toml": "python",
  "crates/pure-analyzer-purecard/uv.lock": "python",
  ".github/workflows/purecard-wheels.yml": "python",
  ".github/workflows/purecard-wheels-build.yml": "python",
  ".github/workflows/purecard-publish.yml": "python",
  "crates/pure-analyzer-purecard": "purecard",

  // Security-relevant config (`.github/labeler.yml`'s `security` glob set).
  ".cargo/audit.toml": "security",
  ".gitleaks.toml": "security",
  "deny.toml": "security",
  "supply-chain": "security",
  "SECURITY.md": "security",
  CODEOWNERS: "security",
  ".github/ISSUE_TEMPLATE/config.yml": "security",
  ".github/zizmor.yml": "security",
  ".github/workflows/security.yml": "security",
  ".github/workflows/lint.yml": "security",

  // CI/automation infra (`.github/labeler.yml`'s `ci` glob set).
  ".github/workflows": "ci",
  ".mise.toml": "ci",
  "mise.lock": "ci",
  "lefthook.yml": "ci",
});

// Any other `purecard-*.yml` workflow file (not one of the python-specific
// ones enumerated above, which win on their longer exact match) is still a
// purecard-area citation. This is a filename-prefix match, not a directory
// boundary — `.github/labeler.yml`'s own glob is `purecard-*.yml`, the same
// non-directory shape — so it is folded into the SAME longest-match scan as
// a virtual candidate below, rather than tried only as a last-resort
// fallback: a fallback-only check would never beat the broader
// `.github/workflows` -> `ci` table entry for a file like
// `purecard-ci.yml`, which cites python-independent purecard CI, not the
// repo's generic CI.
const PURECARD_WORKFLOW_PREFIX = ".github/workflows/purecard-";

// The two-product split (constitution ADR-0004): any crate NOT under
// `crates/pure-analyzer-purecard` (already resolved above) is the analyzer
// product, and so is `xtask` (shared infra, but not PureCARD's own tree) and
// the top-level `fuzz/` crate — Cargo.toml's workspace `exclude` comment is
// explicit that `fuzz` is the analyzer's own cargo-fuzz crate, distinct from
// `crates/pure-analyzer-purecard/fuzz` (already resolved to `purecard` above,
// nested under the crate path that wins there). Found and added during this
// PR's own dry-run validation against the live tracker (issue #298 cites
// both fuzz crates; the top-level one was falling through unclassified
// before `fuzz` was added as a citable root — see PATH_CITATION below).
// A fallback rather than an enumerated per-crate table on purpose: with only
// two products, listing all eleven non-purecard `crates/*` members by hand
// would drift the moment a twelfth is added (DRY/KISS) — unlike cerberus,
// which has over a dozen genuinely distinct product areas and so enumerates
// them. Requires a real segment after the root (`crates/`  alone, with
// nothing after it, names no crate).
function fallbackLabel(cleanPath) {
  if (cleanPath.startsWith("crates/") && cleanPath.length > "crates/".length) return "analyzer";
  if (cleanPath === "xtask" || cleanPath.startsWith("xtask/")) return "analyzer";
  if (cleanPath === "fuzz" || cleanPath.startsWith("fuzz/")) return "analyzer";
  return "";
}

// areaForPath resolves one repo-rooted path to its label by longest matching
// prefix — the table above plus the purecard-workflow virtual prefix in the
// same comparison — falling back to the two-product split when nothing
// matches, or '' when even that does not apply.
export function areaForPath(path) {
  const clean = String(path ?? "")
    .replace(/^\.\//, "")
    .replace(/:\d+.*$/, ""); // strip a trailing `:1338-1345` line cite
  let best = "";
  let bestLen = 0;
  for (const [prefix, label] of Object.entries(PATH_PREFIX_TO_LABEL)) {
    if (clean !== prefix && !clean.startsWith(`${prefix}/`)) continue;
    if (prefix.length > bestLen) {
      best = label;
      bestLen = prefix.length;
    }
  }
  if (clean.startsWith(PURECARD_WORKFLOW_PREFIX) && PURECARD_WORKFLOW_PREFIX.length > bestLen) {
    best = "purecard";
    bestLen = PURECARD_WORKFLOW_PREFIX.length;
  }
  return best || fallbackLabel(clean);
}

function escapeRegExp(s) {
  return s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

// A path citation is any repo-rooted path in the body: either a directory
// root that actually exists in this repo (so prose like "internal
// state/machine" — a hypothetical — cannot masquerade as a path), or one of
// PATH_PREFIX_TO_LABEL's own bare (non-directory) filenames, matched as a
// literal token so `deny.toml` / `SECURITY.md` / `mise.lock` etc. are
// citable even though they carry no leading directory. `docs` and `scripts`
// carry no table entry (deliberately unmapped, like cerberus's own `docs/`
// call) but stay citable — a future table entry gains reachability for
// free, and a "no signal" path citation is still visible for debugging.
const DIR_ROOTS = ["crates", "xtask", "fuzz", "docs", "scripts", "\\.cargo", "\\.github"];
const BARE_FILE_CITATIONS = Object.keys(PATH_PREFIX_TO_LABEL).filter((k) => !k.includes("/"));
const PATH_CITATION = new RegExp(
  `(?:^|[\\s\`([{'"<|>,])((?:${DIR_ROOTS.join("|")})/[A-Za-z0-9_.-]+(?:/[A-Za-z0-9_.-]+)*|${BARE_FILE_CITATIONS.map(escapeRegExp).join("|")})`,
  "g",
);

/** citedPaths returns the DISTINCT repo-rooted paths a body mentions. */
export function citedPaths(body) {
  const out = new Set();
  const text = String(body ?? "");
  PATH_CITATION.lastIndex = 0;
  let m;
  while ((m = PATH_CITATION.exec(text)) !== null) out.add(m[1]);
  return [...out];
}

/**
 * rankPathAreas returns [{ area, paths }] sorted by distinct-path count
 * descending, then area name ascending. The name tiebreak keeps the result
 * independent of the order paths happen to appear in the body.
 */
export function rankPathAreas(body) {
  const byArea = new Map();
  for (const p of citedPaths(body)) {
    const area = areaForPath(p);
    if (!area) continue;
    byArea.set(area, (byArea.get(area) ?? 0) + 1);
  }
  return [...byArea.entries()]
    .map(([area, paths]) => ({ area, paths }))
    .sort((a, b) => b.paths - a.paths || a.area.localeCompare(b.area));
}

// A bare product-name mention anywhere in the title — this repo's issue
// titles carry no `<area>:` prefix convention (unlike cerberus's `promql:`),
// but do routinely name the product ("PureCARD: attest the zero-step
// pipeline…", "A5: front-end pipeline…" does NOT — analysis is inferred from
// paths instead).
const TITLE_AREA_KEYWORDS = Object.freeze([
  [/\bpurecard\b/i, "purecard"],
  [/\banalyzer\b/i, "analyzer"],
]);

/** areaForTitle resolves the title's own bare mention of a product name, or ''. */
export function areaForTitle(title) {
  const text = String(title ?? "");
  for (const [pattern, area] of TITLE_AREA_KEYWORDS) {
    if (pattern.test(text)) return area;
  }
  return "";
}

/** At most this many area labels on one issue, counting any a human already applied. */
export const MAX_AREA_LABELS = 2;
/** A SECONDARY area (past the highest-ranked one) must be cited by at least this many DISTINCT paths. */
export const AREA_SECONDARY_MIN_PATHS = 2;
/** The complete set of labels `inferAreas` can emit — used by the cap accounting below. */
export const AREA_LABELS = Object.freeze(["purecard", "analyzer", "corpus", "python", "security", "ci"]);

/**
 * inferAreas returns the ordered area labels an issue should carry. The
 * title's own bare product mention ranks first when present; path-derived
 * areas fill the remaining slots in rank order, with every SECONDARY area
 * required to clear AREA_SECONDARY_MIN_PATHS.
 */
export function inferAreas(title, body) {
  const out = [];
  const titleArea = areaForTitle(title);
  if (titleArea) out.push(titleArea);

  for (const { area, paths } of rankPathAreas(body)) {
    if (out.length >= MAX_AREA_LABELS) break;
    if (out.includes(area)) continue;
    if (out.length > 0 && paths < AREA_SECONDARY_MIN_PATHS) continue;
    out.push(area);
  }
  return out;
}

// ---------------------------------------------------------------------------
// TYPE
// ---------------------------------------------------------------------------

// This repo's own bug-report title convention: "bug: …" and "flake: …" are
// not Conventional-Commit types (CC has no `bug` token), but they already
// spell the exact label name. Constitution §3 — "Flakes are bugs" — is why
// `flake:` maps here too, not to a separate label. Deliberately narrow: a
// wider prose-signal scorer (cerberus's TYPE_SIGNALS, scanning the body for
// "wrong answer" / "unbounded" / "duplicated" phrasing) was considered and
// dropped — this repo hasn't accumulated the corpus of real issues needed to
// curate that table with the same evidence cerberus had, and a guessed
// phrase list is exactly the imprecision the dimension-label investigation
// below argues against. An issue with no CC prefix and no bug:/flake: title
// is left TYPE-less rather than guessed at; see the PR body for the residue
// this leaves and how it is surfaced (reported, not silently dropped).
//
// Carries the same optional `(scope)` HEADER in type-label.mjs allows, e.g.
// "bug(l2): …" — issue #391 used that form and this fallback rejected it
// (bare "bug:" only), leaving the issue fully unclassified and failing the
// issue-label workflow on every event it fired on.
const BUG_TITLE_PREFIX = /^(?:bug|flake)(?:\([^)]*\))?:/i;

/** inferType returns the single TYPE label an issue should carry, or '' when nothing matched. */
export function inferType(title) {
  const fromCC = labelsForTitle(title);
  if (fromCC.length > 0) return fromCC[0];
  return BUG_TITLE_PREFIX.test(String(title ?? "")) ? "bug" : "";
}

/** Type labels this module may apply. Anything outside the set is a mapping bug, not a label to invent on the fly. */
export const APPLICABLE_TYPE_LABELS = Object.freeze([
  ...new Set([...Object.values(TYPE_TO_LABEL), "dependencies", "bug"]),
]);
/** At most one TYPE label is applied per issue — "what kind of work is this" must stay answerable. */
export const MAX_TYPE_LABELS = 1;

// ---------------------------------------------------------------------------
// DIMENSION
// ---------------------------------------------------------------------------

// This repo's `dimension:*` labels tag which of the downstream NL->Pure
// consumer's evidence dimensions (#331) an issue speaks to. Investigated
// against every dimension:*-labeled issue in the tracker (`gh issue list
// --state all --label dimension:X`) before writing any of this — see the PR
// body for the full survey. Two independent, INDEPENDENTLY VERIFIED signals:
//
//   1. DIMENSION_ANCHOR — the literal bold-backtick tag the #331 ledger's own
//      children (#332-#341) use to state their own dimension, e.g.
//      "**Dimension: `permission`**". This is the issue AUTHOR's own
//      classification, not an inference, and is authoritative wherever
//      present. `**Dimension: none**` (no backticks — #341) and issues with
//      no anchor at all (#335, #336, #340 — genuinely cross-cutting, no
//      single dimension) correctly fall through to no match.
//
//   2. The PureCARD L1/L2 bug-report family (#351/#353/#354/#367/#368/#369),
//      which predates the ledger and never carries the bold anchor. This
//      repo's own domain model
//      (crates/pure-analyzer-purecard/docs/domain-model.md) names L1
//      "Syntactic" and L2 "SchemaConsistent" — exactly the `parse` and
//      `schema` dimensions. Every OPEN issue containing the phrase "L2
//      overlay" is a real dimension:schema report; every OPEN issue
//      containing both "L1" and "engine rejects" is a real dimension:parse
//      report. Verified NOT to false-positive on #328 (an open flake report
//      that mentions "L2" — `l2_liveness` proptests timing out — but never
//      the phrase "L2 overlay", and never "engine rejects").
//
// `dimension:type`, `dimension:semantic-equivalence`, and
// `dimension:permission` currently have exactly ONE labeled issue each
// (#334, #333, #332/#338) and no signal beyond the bold anchor — no second,
// independent phrasing exists yet to corroborate a body-prose rule, so none
// is invented. If a future issue states its dimension only in prose, it is
// left unlabeled rather than guessed — precision over coverage, matching the
// discipline the anchor and L1/L2 signals themselves were held to.
export const DIMENSION_LABELS = Object.freeze([
  "dimension:parse",
  "dimension:type",
  "dimension:schema",
  "dimension:semantic-equivalence",
  "dimension:permission",
]);

const DIMENSION_ANCHOR = /\*\*Dimension:\s*`([a-z-]+)`/i;
const L2_OVERLAY_SIGNAL = /\bL2 overlay\b/;
const L1_MENTION_SIGNAL = /\bL1\b/;
const ENGINE_REJECTS_SIGNAL = /\bengine('s)? rejects?\b/i;

/** inferDimension returns the single `dimension:*` label an issue should carry, or ''. */
export function inferDimension(title, body) {
  const text = `${title ?? ""}\n${body ?? ""}`;
  const anchor = text.match(DIMENSION_ANCHOR);
  if (anchor) {
    const label = `dimension:${anchor[1].toLowerCase()}`;
    return DIMENSION_LABELS.includes(label) ? label : "";
  }
  if (L2_OVERLAY_SIGNAL.test(text)) return "dimension:schema";
  if (L1_MENTION_SIGNAL.test(text) && ENGINE_REJECTS_SIGNAL.test(text)) return "dimension:parse";
  return "";
}

// ---------------------------------------------------------------------------
// CONSUMER-REQUEST
// ---------------------------------------------------------------------------

// The literal date the `consumer-request` label (and this signal) is scoped
// to: every issue sourced from the downstream consumer's 2026-09-02 work
// request states so in a "Source: … consumer[']s work request (2026-09-02)"
// line (checked against #331-#341's real bodies — the apostrophe/possessive
// spelling varies, the date does not). A FUTURE work request landing on a
// different date will not be auto-detected by this signal; that is a known,
// accepted limitation of a date-anchored phrase — see the PR body — rather
// than a reason to weaken the match to something less literal.
const CONSUMER_REQUEST_DATE = "2026-09-02";
const CONSUMER_REQUEST_SIGNAL = new RegExp(String.raw`consumer'?s?\s+work request\s+\(${CONSUMER_REQUEST_DATE}\)`, "i");

/** isConsumerRequest reports whether the body states the dated work-request anchor. */
export function isConsumerRequest(body) {
  return CONSUMER_REQUEST_SIGNAL.test(String(body ?? ""));
}

// ---------------------------------------------------------------------------
// DECISION-NEEDED
// ---------------------------------------------------------------------------

// This repo's own title convention for a maintainer-decision issue (#334
// "A5 DECISION NEEDED: …", #335 "A7 DECISION NEEDED: …" — both already
// labeled decision-needed by hand).
const DECISION_NEEDED_SIGNAL = /decision needed/i;

/** isDecisionNeeded reports whether the title carries the "DECISION NEEDED" convention. */
export function isDecisionNeeded(title) {
  return DECISION_NEEDED_SIGNAL.test(String(title ?? ""));
}

// ---------------------------------------------------------------------------
// Per-issue decision
// ---------------------------------------------------------------------------

/**
 * labelsForIssue computes the FULL decision for one issue:
 *   { areas, type, dimension, consumerRequest, decisionNeeded, missing, skipped, unclassified }
 * `missing` is the additive delta to POST; `skipped` explains any computed
 * label that was withheld because a cap was already met by labels a human
 * set. `unclassified` is true when NONE of the five passes produced
 * anything.
 */
export function labelsForIssue({ title, body, labels }) {
  const have = new Set((labels ?? []).map((l) => (typeof l === "string" ? l : l.name)));
  const haveAreas = [...have].filter((l) => AREA_LABELS.includes(l));
  const haveTypes = [...have].filter((l) => APPLICABLE_TYPE_LABELS.includes(l));

  const areas = inferAreas(title, body);
  const type = inferType(title);
  const dimension = inferDimension(title, body);
  const consumerRequest = isConsumerRequest(body);
  const decisionNeeded = isDecisionNeeded(title);

  const proposed = [];
  const skipped = [];

  let areaBudget = MAX_AREA_LABELS - haveAreas.length;
  for (const area of areas) {
    if (have.has(area)) continue;
    if (areaBudget <= 0) {
      skipped.push(`${area} (area cap ${MAX_AREA_LABELS} already met by: ${haveAreas.join(", ")})`);
      continue;
    }
    proposed.push(area);
    areaBudget--;
  }

  if (type && !have.has(type)) {
    if (haveTypes.length >= MAX_TYPE_LABELS) {
      skipped.push(`${type} (type cap ${MAX_TYPE_LABELS} already met by: ${haveTypes.join(", ")})`);
    } else {
      proposed.push(type);
    }
  }

  if (dimension && !have.has(dimension)) proposed.push(dimension);
  if (consumerRequest && !have.has("consumer-request")) proposed.push("consumer-request");
  if (decisionNeeded && !have.has("decision-needed")) proposed.push("decision-needed");

  // `ci` can be proposed as BOTH an area (a `.github/workflows/...`
  // citation) and a type (a `ci:` Conventional-Commit title) — the two
  // tables share that repo label for genuinely different reasons. Dedupe
  // once here rather than special-casing either table around the other's
  // vocabulary.
  const missing = [...new Set(proposed)];

  return {
    areas,
    type,
    dimension,
    consumerRequest,
    decisionNeeded,
    missing,
    skipped,
    unclassified: areas.length === 0 && !type && !dimension && !consumerRequest && !decisionNeeded,
  };
}

// ---------------------------------------------------------------------------
// Anti-vacuity guards
// ---------------------------------------------------------------------------

/** assertTablesUsable fails the run if a mapping table has been emptied. */
export function assertTablesUsable() {
  const problems = [];
  if (Object.keys(PATH_PREFIX_TO_LABEL).length === 0) problems.push("PATH_PREFIX_TO_LABEL is empty");
  if (Object.keys(TYPE_TO_LABEL).length === 0) problems.push("TYPE_TO_LABEL (from type-label.mjs) is empty");
  if (DIMENSION_LABELS.length === 0) problems.push("DIMENSION_LABELS is empty");
  if (AREA_LABELS.length === 0) problems.push("AREA_LABELS is empty");
  return problems;
}

/**
 * assertBodyFetched distinguishes "the API never gave us a body" (a fetch
 * failure that would make every issue look signal-free) from "the author
 * wrote no body" (real, and reported). A missing KEY is the former.
 */
export function assertBodyFetched(issue) {
  if (!Object.prototype.hasOwnProperty.call(issue, "body")) {
    return `issue #${issue.number}: no \`body\` field in the payload — the body was never fetched`;
  }
  return "";
}

// ---------------------------------------------------------------------------
// Runner
// ---------------------------------------------------------------------------

function truthy(v) {
  return v === "1" || String(v).toLowerCase() === "true";
}

async function main() {
  const tableProblems = assertTablesUsable();
  if (tableProblems.length > 0) {
    die(`issue-label mapping tables are unusable:\n  - ${tableProblems.join("\n  - ")}`);
  }

  const mode = process.env.ISSUE_LABEL_MODE ?? "";
  if (mode !== "event" && mode !== "backfill") {
    die(`ISSUE_LABEL_MODE must be 'event' or 'backfill' (got ${JSON.stringify(mode)})`);
  }
  const dryRun = truthy(process.env.ISSUE_LABEL_DRY_RUN);
  const api = process.env.GITHUB_API_URL || DEFAULT_API_URL;
  const repo = process.env.GITHUB_REPOSITORY ?? "";
  const token = process.env.GITHUB_TOKEN ?? "";
  const fixture = process.env.ISSUE_LABEL_FIXTURE ?? "";

  if (!fixture && !repo) die("GITHUB_REPOSITORY is required (owner/repo)");
  if (!fixture && !token) die("GITHUB_TOKEN is required");

  let issues;
  if (fixture) {
    if (!dryRun) die("ISSUE_LABEL_FIXTURE is a dry-run-only input; set ISSUE_LABEL_DRY_RUN=1");
    issues = JSON.parse(readFileSync(fixture, "utf8"));
  } else if (mode === "event") {
    const eventPath = process.env.GITHUB_EVENT_PATH ?? "";
    if (!eventPath) die("GITHUB_EVENT_PATH is required in event mode");
    const payload = JSON.parse(readFileSync(eventPath, "utf8"));
    if (!payload.issue) die("event payload carries no `issue` object");
    issues = [payload.issue];
  } else {
    issues = await listOpenIssues(api, repo, token);
  }

  if (!Array.isArray(issues)) die("the issue set is not an array — nothing to process");

  // ANTI-VACUITY: a run that saw no issues at all is a broken run, not a
  // clean one. In event mode the payload always carries exactly one.
  if (issues.length === 0) {
    die(`issue-label (${mode}) processed ZERO issues — the fetch returned nothing`);
  }

  const unfetched = [];
  const unclassified = [];
  const emptyBodies = [];
  let applied = 0;
  let touched = 0;
  let alreadyComplete = 0;
  let unlabeledRemaining = 0;

  for (const issue of issues) {
    const problem = assertBodyFetched(issue);
    if (problem) {
      unfetched.push(problem);
      continue;
    }
    const number = issue.number;
    const title = issue.title ?? "";
    const body = issue.body ?? "";
    const have = (issue.labels ?? []).map((l) => (typeof l === "string" ? l : l.name));
    if (body.trim() === "") emptyBodies.push(number);

    const decision = labelsForIssue({ title, body, labels: issue.labels });

    if (decision.unclassified) {
      unclassified.push(`#${number} — ${title}`);
      if (have.length === 0) unlabeledRemaining++;
      continue;
    }

    const detail =
      `areas=[${decision.areas.join(", ") || "-"}] type=${decision.type || "-"} dimension=${decision.dimension || "-"}` +
      (decision.skipped.length > 0 ? ` withheld=[${decision.skipped.join("; ")}]` : "");

    if (decision.missing.length === 0) {
      alreadyComplete++;
      notice(`#${number}: already carries its computed labels — ${detail}`);
      continue;
    }

    if (dryRun) {
      notice(`#${number} DRY-RUN would add [${decision.missing.join(", ")}] — ${detail} — ${title}`);
    } else {
      await addLabels(api, repo, token, number, decision.missing);
      notice(`#${number} labeled [${decision.missing.join(", ")}] — ${detail} — ${title}`);
    }
    applied += decision.missing.length;
    touched++;
  }

  const failures = [];

  if (unfetched.length > 0) {
    failures.push(`${unfetched.length} issue(s) had no body fetched:\n  - ${unfetched.join("\n  - ")}`);
  }

  // ANTI-VACUITY: an unclassifiable issue is REPORTED, never silently
  // skipped — a growing residue means the mapping is too narrow.
  if (unclassified.length > 0) {
    failures.push(
      `${unclassified.length} issue(s) matched no area, type, dimension, consumer-request, or decision-needed rule ` +
        `— widen the mapping (PATH_PREFIX_TO_LABEL / BUG_TITLE_PREFIX / DIMENSION signals):\n  - ${unclassified.join("\n  - ")}`,
    );
  }

  // ANTI-VACUITY: in backfill mode, applying nothing while unlabeled issues
  // remain is exactly the hollow-green shape this gate exists to prevent.
  if (mode === "backfill" && !dryRun && applied === 0 && unlabeledRemaining > 0) {
    failures.push(`backfill applied ZERO labels while ${unlabeledRemaining} issue(s) remain unlabeled — the labeler is inert`);
  }

  if (emptyBodies.length > 0) {
    notice(`${emptyBodies.length} issue(s) have an empty body (title-only inference): ${emptyBodies.join(", ")}`);
  }

  notice(
    `issue-label (${mode}${dryRun ? ", dry-run" : ""}): ${issues.length} issue(s) scanned, ` +
      `${touched} issue(s) ${dryRun ? "would be" : ""} labeled, ${applied} label(s) ` +
      `${dryRun ? "pending" : "applied"}, ${alreadyComplete} already complete, ${unclassified.length} unclassified.`,
  );

  if (failures.length > 0) {
    for (const f of failures) error(f);
    process.exit(1);
  }

  notice(
    `issue-label (${mode}${dryRun ? ", dry-run" : ""}) complete: ${touched}/${issues.length} issue(s) ` +
      `${dryRun ? "would receive" : "received"} ${applied} label(s).`,
  );
}

if (process.argv.includes("--check-tables")) {
  const problems = assertTablesUsable();
  if (problems.length > 0) die(`issue-label --check-tables: ${problems.join("; ")}`);
  notice("issue-label --check-tables: mapping tables usable");
  process.exit(0);
}

if (import.meta.main) {
  main().catch((e) => {
    error(String(e?.stack ?? e));
    process.exit(1);
  });
}
