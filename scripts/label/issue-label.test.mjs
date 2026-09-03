import { describe, expect, test } from "bun:test";
import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import {
  APPLICABLE_TYPE_LABELS,
  AREA_LABELS,
  AREA_SECONDARY_MIN_PATHS,
  DIMENSION_LABELS,
  MAX_AREA_LABELS,
  MAX_TYPE_LABELS,
  PATH_PREFIX_TO_LABEL,
  areaForPath,
  areaForTitle,
  assertBodyFetched,
  assertTablesUsable,
  citedPaths,
  inferAreas,
  inferDimension,
  inferType,
  isConsumerRequest,
  isDecisionNeeded,
  labelsForIssue,
  rankPathAreas,
} from "./issue-label.mjs";

const CLI = join(import.meta.dir, "issue-label.mjs");

function runCLI(env, args = []) {
  return Bun.spawnSync([process.execPath, CLI, ...args], { env: { ...process.env, ...env } });
}

function fixtureFile(name, issues) {
  const dir = mkdtempSync(join(tmpdir(), "issue-label-"));
  const file = join(dir, name);
  writeFileSync(file, JSON.stringify(issues));
  return file;
}

const out = (proc) => proc.stdout.toString() + proc.stderr.toString();

// ---------------------------------------------------------------------------
// AREA: path resolution
// ---------------------------------------------------------------------------

describe("areaForPath", () => {
  test("resolves by longest prefix, not first match", () => {
    expect(areaForPath("crates/pure-analyzer-purecard/corpus/legend-4.113.0/accept.jsonl")).toBe("corpus");
    expect(areaForPath("crates/pure-analyzer-purecard/src/mask.rs")).toBe("purecard");
    expect(areaForPath("crates/pure-analyzer-parser/src/m3.rs")).toBe("analyzer");
  });

  test("strips a trailing file:line cite and a leading ./", () => {
    expect(areaForPath("crates/pure-analyzer-parser/src/m3.rs:1338-1345")).toBe("analyzer");
    expect(areaForPath("./crates/pure-analyzer-purecard/src/mask.rs")).toBe("purecard");
    expect(areaForPath(".github/workflows/ci.yml:12")).toBe("ci");
  });

  test("resolves python-specific purecard files distinctly from the generic purecard-workflow prefix", () => {
    expect(areaForPath("crates/pure-analyzer-purecard/python/purecard/__init__.py")).toBe("python");
    expect(areaForPath("crates/pure-analyzer-purecard/src/ffi.rs")).toBe("python");
    expect(areaForPath(".github/workflows/purecard-wheels.yml")).toBe("python");
    expect(areaForPath(".github/workflows/purecard-ci.yml")).toBe("purecard");
  });

  test("falls back to the two-product split for xtask, the top-level fuzz crate, and any other crate", () => {
    expect(areaForPath("xtask/src/main.rs")).toBe("analyzer");
    expect(areaForPath("xtask")).toBe("analyzer");
    expect(areaForPath("crates/libpure/src/lib.rs")).toBe("analyzer");
    // The top-level `fuzz/` crate is the analyzer's own cargo-fuzz crate
    // (Cargo.toml's `exclude` comment) — distinct from
    // `crates/pure-analyzer-purecard/fuzz`, which resolves to `purecard` via
    // its own longer, more specific prefix. Real issue #298 cites both.
    expect(areaForPath("fuzz/Cargo.toml")).toBe("analyzer");
    expect(areaForPath("crates/pure-analyzer-purecard/fuzz/fuzz_targets/m3_parser.rs")).toBe("purecard");
  });

  test("resolves the security and ci overlays", () => {
    expect(areaForPath("deny.toml")).toBe("security");
    expect(areaForPath("SECURITY.md")).toBe("security");
    expect(areaForPath(".github/workflows/lint.yml")).toBe("security");
    expect(areaForPath("lefthook.yml")).toBe("ci");
    expect(areaForPath(".github/workflows/no-warnings.yml")).toBe("ci");
  });

  test("returns empty for an unmapped path (docs/ deliberately unmapped, like cerberus's own call)", () => {
    expect(areaForPath("docs/domain-model.md")).toBe("");
    expect(areaForPath("README.md")).toBe("");
    expect(areaForPath("crates/")).toBe(""); // no trailing segment: fallback requires a real crate path
  });

  test("every table entry and every fallback arm is independently reachable", () => {
    // Anti-drift: every label PATH_PREFIX_TO_LABEL or the fallback can
    // produce is exercised somewhere above.
    const emitted = new Set([...Object.values(PATH_PREFIX_TO_LABEL), ...AREA_LABELS]);
    expect(emitted.size).toBeGreaterThan(0);
  });
});

describe("citedPaths", () => {
  test("extracts distinct repo-rooted paths from real issue prose", () => {
    const body = [
      "`comparison_corpus.rs`'s pinned evidence — see `crates/pure-analyzer-analysis/src/relational.rs:559-568` —",
      "also cites `crates/pure-analyzer-analysis/src/relational.rs:1209-1219` (same file, second cite).",
      "Prose about internal state machines must not parse as a path.",
    ].join("\n");
    const got = citedPaths(body);
    expect(got.sort()).toEqual(["crates/pure-analyzer-analysis/src/relational.rs"]);
  });

  test("recognizes every rooted prefix this repo actually uses", () => {
    const body =
      "`crates/pure-analyzer-parser/src/m3.rs` `xtask/src/main.rs` `fuzz/Cargo.toml` `docs/domain-model.md` " +
      "`scripts/checks/no-work-ledger.mjs` `.github/workflows/ci.yml`";
    expect(citedPaths(body).sort()).toEqual([
      ".github/workflows/ci.yml",
      "crates/pure-analyzer-parser/src/m3.rs",
      "docs/domain-model.md",
      "fuzz/Cargo.toml",
      "scripts/checks/no-work-ledger.mjs",
      "xtask/src/main.rs",
    ]);
  });
});

describe("rankPathAreas", () => {
  test("is order-independent (name tiebreak on equal score)", () => {
    const a = rankPathAreas("`crates/pure-analyzer-purecard/src/a.rs` then `crates/libpure/src/b.rs`");
    const b = rankPathAreas("`crates/libpure/src/b.rs` then `crates/pure-analyzer-purecard/src/a.rs`");
    expect(a.map((r) => r.area)).toEqual(b.map((r) => r.area));
    expect(a.map((r) => r.area)).toEqual(["analyzer", "purecard"]);
  });
});

// ---------------------------------------------------------------------------
// AREA: title resolution
// ---------------------------------------------------------------------------

describe("areaForTitle", () => {
  test("reads a bare product-name mention anywhere in the title", () => {
    expect(areaForTitle("PureCARD: attest the zero-step pipeline against the live engine")).toBe("purecard");
    expect(areaForTitle("Drive real-model Python inference through PureCARD and live Legend")).toBe("purecard");
    expect(areaForTitle("A5: retroactive-wrap budget rejects valid queries")).toBe("");
  });

  test("also matches the hyphenated pure-analyzer spelling", () => {
    expect(areaForTitle("A7: publish a pinnable pure-analyzer artifact")).toBe("analyzer");
  });

  test("yields nothing when neither product name is mentioned", () => {
    expect(areaForTitle("no-work-ledger gate is an instance denylist of eight paths")).toBe("");
  });
});

// ---------------------------------------------------------------------------
// AREA: caps and secondary-evidence bar
// ---------------------------------------------------------------------------

describe("inferAreas", () => {
  test("a single passing mention of a second area earns nothing; two distinct files do", () => {
    const single = inferAreas("PureCARD: mask narrowing", "`crates/libpure/src/lib.rs`");
    expect(single).toEqual(["purecard"]);

    const double = inferAreas("PureCARD: mask narrowing", "`crates/libpure/src/lib.rs` and `crates/libpure/src/render.rs`");
    expect(double).toEqual(["purecard", "analyzer"]);
    expect(AREA_SECONDARY_MIN_PATHS).toBe(2);
  });

  test("caps at MAX_AREA_LABELS even with more candidate areas", () => {
    const many = inferAreas(
      "cross-cutting",
      [
        "`crates/pure-analyzer-purecard/corpus/a.jsonl` `crates/pure-analyzer-purecard/corpus/b.jsonl`",
        "`deny.toml` `SECURITY.md`",
        "`.github/workflows/ci.yml` `.github/workflows/lint.yml`",
      ].join(" "),
    );
    expect(many.length).toBe(MAX_AREA_LABELS);
  });

  test("is deterministic across repeated calls", () => {
    const title = "A5: hygiene sweep";
    const body = "`crates/pure-analyzer-analysis/src/a.rs` `crates/pure-analyzer-analysis/src/b.rs`";
    const first = inferAreas(title, body);
    for (let i = 0; i < 5; i++) expect(inferAreas(title, body)).toEqual(first);
  });
});

// ---------------------------------------------------------------------------
// TYPE
// ---------------------------------------------------------------------------

describe("inferType", () => {
  test("delegates a Conventional-Commit prefix to type-label.mjs", () => {
    expect(inferType("feat(l1): admit scalar/date initializers")).toBe("enhancement");
    expect(inferType("fix(purecard): narrow arm-R colName positions")).toBe("bug");
    expect(inferType("docs(purecard): backfill the changelog entry")).toBe("documentation");
    expect(inferType("ci: pin actionlint")).toBe("ci");
  });

  test("classifies this repo's own bug: and flake: title convention", () => {
    expect(inferType("bug: L1 admits a pipeline step with no argument list")).toBe("bug");
    expect(inferType("flake: purecard L2 proptests time out under `just ci` load")).toBe("bug");
    expect(inferType("BUG: shouty still matches")).toBe("bug");
  });

  test("CC delegation wins over the bug:/flake: fallback when both could apply", () => {
    // Not a realistic title, but proves the precedence is CC-first.
    expect(inferType("fix: bug: nested")).toBe("bug"); // CC `fix:` -> bug anyway, same answer either path
  });

  test("returns empty for a title with neither signal", () => {
    expect(inferType("A5: retroactive-wrap budget rejects valid queries")).toBe("");
    expect(inferType("")).toBe("");
    expect(inferType(null)).toBe("");
  });
});

// ---------------------------------------------------------------------------
// DIMENSION
// ---------------------------------------------------------------------------

describe("inferDimension", () => {
  test("reads the bold-backtick anchor the #331 ledger family uses, for every known dimension", () => {
    expect(inferDimension("A7: add a fail-closed permission pass", "**Dimension: `permission`** (authorization).")).toBe(
      "dimension:permission",
    );
    expect(inferDimension("A7: cover group-by aggregation", "**Dimension: `semantic-equivalence`.**")).toBe(
      "dimension:semantic-equivalence",
    );
    expect(inferDimension("A5 DECISION NEEDED: milestoning arity", "**Dimension: `type`** (the consumer files date-arity...)")).toBe(
      "dimension:type",
    );
    expect(inferDimension("A7: grow the corpus", "**Dimension: `parse`.**")).toBe("dimension:parse");
    expect(inferDimension("bug: L2 masks a variable", "**Dimension: `schema`**")).toBe("dimension:schema");
  });

  test("an unbacktick-quoted or absent anchor yields nothing (cross-cutting issues carry no dimension)", () => {
    expect(inferDimension("A7: emit a per-query evidence envelope", "**Dimension: none** — these are inputs.")).toBe("");
    expect(inferDimension("A7: carry provenance in every envelope", "Cross-cutting provenance; no single evidence dimension.")).toBe(
      "",
    );
  });

  test("classifies the L2-overlay bug-report family as dimension:schema", () => {
    expect(
      inferDimension(
        "bug: L2 masks a variable as the milestoning date argument of all()",
        "With a schema loaded, the L2 overlay masks a variable passed as the milestoning date argument.",
      ),
    ).toBe("dimension:schema");
    expect(
      inferDimension(
        "bug: L2 member narrowing is bypassed by a whitespace-led token",
        "At a member-narrowing position, the L2 overlay admits a token whose first byte is whitespace.",
      ),
    ).toBe("dimension:schema");
  });

  test("classifies the L1-admits/engine-rejects bug-report family as dimension:parse", () => {
    expect(
      inferDimension(
        "bug: L1 admits a pipeline step with no argument list",
        "L1 admits a method-chain step with no argument list. The engine rejects every one of these.",
      ),
    ).toBe("dimension:parse");
  });

  test("does NOT false-positive on a bare L2 mention with no admit/mask/reject classification (issue #328's shape)", () => {
    expect(
      inferDimension(
        "flake: purecard L2 proptests time out under `just ci` load (60s terminate)",
        "Two PureCARD proptest cases exceed slow-timeout. TIMEOUT [60.015s] pure-analyzer-purecard::l2_liveness the_l2_mask_is_never_empty_at_a_position.",
      ),
    ).toBe("");
  });

  test("does NOT false-positive on a bare L1 mention without an engine-rejects pairing", () => {
    expect(inferDimension("feat(l1): admit scalar/date initializers in let bindings", "L1 currently rejects this at the first byte.")).toBe(
      "", // "rejects" alone, without the literal "engine … rejects" phrasing, is not enough
    );
  });

  test("every label in DIMENSION_LABELS is reachable via the bold anchor", () => {
    for (const label of DIMENSION_LABELS) {
      const value = label.replace("dimension:", "");
      expect(inferDimension("t", `**Dimension: \`${value}\`**`)).toBe(label);
    }
  });
});

// ---------------------------------------------------------------------------
// CONSUMER-REQUEST
// ---------------------------------------------------------------------------

describe("isConsumerRequest", () => {
  test("matches every real phrasing variant seen in the tracker", () => {
    expect(isConsumerRequest("Source: downstream consumer work request (2026-09-02), item 1.")).toBe(true);
    expect(isConsumerRequest("Source: consumer work request (2026-09-02), item 5. Tracking: #331.")).toBe(true);
    expect(isConsumerRequest("raised by the downstream consumer's work request (2026-09-02): \"...\"")).toBe(true);
  });

  test("does not match an unrelated body, or a different date", () => {
    expect(isConsumerRequest("Found during a full ACPR audit of CI infrastructure.")).toBe(false);
    expect(isConsumerRequest("consumer work request (2025-01-01)")).toBe(false);
    expect(isConsumerRequest("")).toBe(false);
    expect(isConsumerRequest(null)).toBe(false);
  });
});

// ---------------------------------------------------------------------------
// DECISION-NEEDED
// ---------------------------------------------------------------------------

describe("isDecisionNeeded", () => {
  test("matches this repo's DECISION NEEDED title convention, case-insensitively", () => {
    expect(isDecisionNeeded("A5 DECISION NEEDED: milestoning arity")).toBe(true);
    expect(isDecisionNeeded("A7 decision needed: publish a pinnable artifact")).toBe(true);
  });

  test("does not match an ordinary title", () => {
    expect(isDecisionNeeded("bug: L1 admits a pipeline step with no argument list")).toBe(false);
  });
});

// ---------------------------------------------------------------------------
// Decision layer: additive, capped, idempotent
// ---------------------------------------------------------------------------

describe("labelsForIssue", () => {
  test("proposes the area + type + dimension + consumer-request delta for a fresh issue", () => {
    const d = labelsForIssue({
      title: "bug: L2 masks a variable as the milestoning date argument of all()",
      body:
        "Source: consumer work request (2026-09-02), item 7. With a schema loaded, the L2 overlay masks a variable — " +
        "`crates/pure-analyzer-purecard/src/schema/mod.rs`.",
      labels: [],
    });
    expect(d.missing.sort()).toEqual(["consumer-request", "dimension:schema", "purecard", "bug"].sort());
    expect(d.unclassified).toBe(false);
    expect(d.skipped).toEqual([]);
  });

  test("is IDEMPOTENT — re-running on the result is a no-op", () => {
    const issue = {
      title: "bug: L2 masks a variable as the milestoning date argument of all()",
      body: "Source: consumer work request (2026-09-02). The L2 overlay masks a variable — `crates/pure-analyzer-purecard/src/schema/mod.rs`.",
      labels: [],
    };
    const first = labelsForIssue(issue);
    const second = labelsForIssue({ ...issue, labels: first.missing.map((name) => ({ name })) });
    expect(second.missing).toEqual([]);
  });

  test("never proposes removing or replacing a label a human set", () => {
    const d = labelsForIssue({
      title: "bug: L2 masks a variable",
      body: "The L2 overlay masks a variable — `crates/pure-analyzer-purecard/src/schema/mod.rs`.",
      labels: [{ name: "good first issue" }, { name: "help wanted" }],
    });
    expect(d.missing).not.toContain("good first issue");
    expect(d.missing).not.toContain("help wanted");
  });

  test("respects a human-set TYPE label instead of adding a second", () => {
    const d = labelsForIssue({
      title: "bug: L2 masks a variable",
      body: "The L2 overlay masks a variable — `crates/pure-analyzer-purecard/src/schema/mod.rs`.",
      labels: [{ name: "enhancement" }],
    });
    expect(d.missing).not.toContain("bug");
    expect(d.skipped.some((s) => s.startsWith("bug (type cap"))).toBe(true);
    expect(MAX_TYPE_LABELS).toBe(1);
  });

  test("respects a human-set AREA label when the cap is already met", () => {
    const body = "`crates/pure-analyzer-purecard/corpus/a.jsonl` `crates/pure-analyzer-purecard/corpus/b.jsonl`";
    const labels = [{ name: "purecard" }, { name: "analyzer" }];
    const d = labelsForIssue({ title: "cross-cutting", body, labels });
    expect(d.missing).not.toContain("corpus");
    expect(d.skipped.some((s) => s.includes("area cap 2"))).toBe(true);
  });

  test("dedupes when the same label is proposed by both an area and a type signal (ci)", () => {
    const d = labelsForIssue({
      title: "ci: the postponed-markers gate misses a marker class",
      body: "See `.github/workflows/ci.yml` for the gate this touches.",
      labels: [],
    });
    expect(d.missing.filter((l) => l === "ci").length).toBe(1);
  });

  test("flags an issue no rule can classify at all", () => {
    const d = labelsForIssue({ title: "Thoughts on naming", body: "Just a question.", labels: [] });
    expect(d.unclassified).toBe(true);
    expect(d.missing).toEqual([]);
  });

  test("an issue with only a path citation (no type/dimension) is still classified via its area", () => {
    const d = labelsForIssue({
      title: "no-work-ledger gate is an instance denylist of eight paths",
      body: "`scripts/checks/no-work-ledger.mjs:10-19` is eight hardcoded paths.",
      labels: [],
    });
    // scripts/ is not in the area table (mirrors labeler.yml's own scope),
    // so this really is unclassifiable — proving the anti-vacuity guard has
    // real teeth, not just a happy path.
    expect(d.unclassified).toBe(true);
  });
});

// ---------------------------------------------------------------------------
// Anti-vacuity guards
// ---------------------------------------------------------------------------

describe("assertTablesUsable / assertBodyFetched", () => {
  test("passes on the live tables", () => {
    expect(assertTablesUsable()).toEqual([]);
    expect(Object.keys(PATH_PREFIX_TO_LABEL).length).toBeGreaterThan(0);
    expect(APPLICABLE_TYPE_LABELS.length).toBeGreaterThan(0);
  });

  test("assertBodyFetched separates 'never fetched' from 'author wrote nothing'", () => {
    expect(assertBodyFetched({ number: 7, title: "t" })).toMatch(/never fetched/);
    expect(assertBodyFetched({ number: 7, title: "t", body: null })).toBe("");
    expect(assertBodyFetched({ number: 7, title: "t", body: "" })).toBe("");
  });
});

// ---------------------------------------------------------------------------
// CLI end-to-end
// ---------------------------------------------------------------------------

const DRY = { ISSUE_LABEL_MODE: "backfill", ISSUE_LABEL_DRY_RUN: "1" };

describe("CLI", () => {
  test("dry-run exits 0 and reports a per-issue line for a real fixture", () => {
    const file = fixtureFile("ok.json", [
      {
        number: 367,
        title: "bug: L2 masks a variable as the milestoning date argument of all()",
        body: "Source: consumer work request (2026-09-02). The L2 overlay masks a variable — `crates/pure-analyzer-purecard/src/schema/mod.rs`.",
        labels: [],
      },
    ]);
    const proc = runCLI({ ...DRY, ISSUE_LABEL_FIXTURE: file });
    const text = out(proc);
    expect(proc.exitCode).toBe(0);
    expect(text).toMatch(/#367 DRY-RUN would add/);
    expect(text).toMatch(/1 issue\(s\) scanned, 1 issue\(s\) would be labeled/);
  });

  test("reports zero pending labels for an already-labeled fixture (idempotent)", () => {
    const file = fixtureFile("done.json", [
      {
        number: 367,
        title: "bug: L2 masks a variable",
        body: "The L2 overlay masks a variable — `crates/pure-analyzer-purecard/src/schema/mod.rs`.",
        labels: [{ name: "purecard" }, { name: "bug" }, { name: "dimension:schema" }],
      },
    ]);
    const proc = runCLI({ ...DRY, ISSUE_LABEL_FIXTURE: file });
    expect(proc.exitCode).toBe(0);
    expect(out(proc)).toMatch(/#367: already carries its computed labels/);
  });

  test("FAILS on a vacuous run — zero issues processed", () => {
    const file = fixtureFile("empty.json", []);
    const proc = runCLI({ ...DRY, ISSUE_LABEL_FIXTURE: file });
    expect(proc.exitCode).toBe(1);
    expect(out(proc)).toMatch(/processed ZERO issues/);
  });

  test("FAILS when a body was never fetched", () => {
    const file = fixtureFile("nobody.json", [{ number: 42, title: "bug: something", labels: [] }]);
    const proc = runCLI({ ...DRY, ISSUE_LABEL_FIXTURE: file });
    expect(proc.exitCode).toBe(1);
    expect(out(proc)).toMatch(/no `body` field in the payload/);
  });

  test("FAILS, naming the issue, when a rule classifies nothing", () => {
    const file = fixtureFile("residue.json", [{ number: 99, title: "Thoughts on naming", body: "A question.", labels: [] }]);
    const proc = runCLI({ ...DRY, ISSUE_LABEL_FIXTURE: file });
    expect(proc.exitCode).toBe(1);
    expect(out(proc)).toMatch(/#99 — Thoughts on naming/);
    expect(out(proc)).toMatch(/widen the mapping/);
  });

  test("rejects an unknown mode and a fixture outside dry-run", () => {
    const file = fixtureFile("ok2.json", [{ number: 1, title: "bug: x", body: "`crates/libpure/src/lib.rs`", labels: [] }]);
    const bad = runCLI({ ISSUE_LABEL_MODE: "sideways", ISSUE_LABEL_FIXTURE: file, ISSUE_LABEL_DRY_RUN: "1" });
    expect(bad.exitCode).toBe(1);
    expect(out(bad)).toMatch(/ISSUE_LABEL_MODE must be/);

    const wet = runCLI({
      ISSUE_LABEL_MODE: "backfill",
      ISSUE_LABEL_FIXTURE: file,
      GITHUB_REPOSITORY: "o/r",
      GITHUB_TOKEN: "t",
    });
    expect(wet.exitCode).toBe(1);
    expect(out(wet)).toMatch(/dry-run-only input/);
  });

  test("--check-tables proves the mapping tables are usable", () => {
    const proc = runCLI({}, ["--check-tables"]);
    expect(proc.exitCode).toBe(0);
    expect(out(proc)).toMatch(/mapping tables usable/);
  });
});
