// type-label.mjs — the single source of truth mapping a Conventional-Commit
// header to this repo's type label(s). Shared by two callers:
//
//   1. pr-label.mjs    — the PR title IS the squash-merge commit subject, so
//                         it always carries the type.
//   2. issue-label.mjs — delegates here first, before falling back to its
//                         own curated bug/flake title-prefix rule, so the CC
//                         mapping has exactly one definition in the repo.
//
// Ported from the sibling `cerberus` repo's `.github/scripts/pr-type-label.mjs`
// (see that file for the pattern this is adapted from), adjusted to this
// repo's actual label set (`gh label list -R tsouza/pure-analyzer`):
//
//   feat -> enhancement   fix -> bug          docs -> documentation
//   ci -> ci              test -> test        refactor -> refactor
//   perf -> performance   chore -> chore      build -> build
//   revert -> revert      style -> (none; cosmetic, no label)
// Scope override (checked before the bare-type table):
//   *(deps) -> dependencies   (e.g. chore(deps), ci(deps) — this repo's
//   Dependabot config uses both prefixes; see .github/dependabot.yml)
//
// Cerberus additionally special-cases `chore(release) -> release + chore`
// because ITS release-plz config emits `chore(release): ...` PR titles. This
// repo's release-plz PRs are titled plain `chore: release vX.Y.Z` (no scope
// — see PR #342), so that override would be unreachable dead code here; it
// is deliberately not ported. `chore: release …` falls through to the bare
// `chore` mapping, which is correct.
//
// Dependency-light by design: no imports at all, so any caller (a Bun
// script, a future github-script step) can load it with zero setup.
//
// argv `--self-test` runs the in-process assertion suite and exits.

// The bare type -> label table. `style` is intentionally absent: a purely
// cosmetic change carries no tracking label.
export const TYPE_TO_LABEL = Object.freeze({
  feat: "enhancement",
  fix: "bug",
  docs: "documentation",
  ci: "ci",
  test: "test",
  refactor: "refactor",
  perf: "performance",
  chore: "chore",
  build: "build",
  revert: "revert",
});

// Conventional-Commit header: `type(scope)!: subject`. The scope and the
// `!` breaking-change marker are optional. Case-insensitive on the type so
// a stray capital doesn't silently drop the label.
const HEADER = /^([a-z]+)(?:\(([^)]*)\))?!?:/i;

// labelsForTitle returns the array of labels a PR/issue with this title
// should carry from its Conventional-Commit prefix. An empty array means
// "no type label applies" (no CC prefix, a `style:` change, or an unknown
// type).
export function labelsForTitle(title) {
  const m = String(title ?? "").match(HEADER);
  if (!m) return [];
  const type = m[1].toLowerCase();
  const scope = (m[2] || "").toLowerCase();

  if (scope === "deps") return ["dependencies"];
  const label = TYPE_TO_LABEL[type];
  return label ? [label] : [];
}

function selfTest() {
  const assert = (cond, msg) => {
    if (!cond) throw new Error("self-test: " + msg);
  };
  const eq = (title, want, why) => {
    const got = labelsForTitle(title);
    assert(
      got.length === want.length && got.every((v, i) => v === want[i]),
      `${why}: labelsForTitle(${JSON.stringify(title)}) = [${got}] want [${want}]`,
    );
  };

  eq("feat: add a walk recipe", ["enhancement"], "feat -> enhancement");
  eq("fix: cursor overflow", ["bug"], "fix -> bug");
  eq("docs: tidy README", ["documentation"], "docs -> documentation");
  eq("ci: pin actionlint", ["ci"], "ci -> ci");
  eq("test: add fixture", ["test"], "test -> test");
  eq("refactor: extract emitter", ["refactor"], "refactor -> refactor");
  eq("perf: prune scan", ["performance"], "perf -> performance");
  eq("chore: tidy", ["chore"], "chore -> chore");
  eq("build: bump goreleaser", ["build"], "build -> build");
  eq("revert: undo #123", ["revert"], "revert -> revert");

  eq("style: gofmt", [], "style -> (none)");

  eq("chore(deps): bump x from 1 to 2", ["dependencies"], "chore(deps) -> dependencies");
  eq("ci(deps): bump action", ["dependencies"], "ci(deps) -> dependencies");
  eq("fix(deps): pin transitive", ["dependencies"], "any *(deps) -> dependencies");

  eq("chore: release v1.2.3", ["chore"], "plain release commit falls through to chore (no scope override)");
  eq("chore(release): v1.2.3", ["chore"], "chore(release) is not special-cased here (unused convention)");

  eq("feat!: breaking", ["enhancement"], "feat! -> enhancement");
  eq("feat(purecard)!: breaking scoped", ["enhancement"], "feat(scope)! -> enhancement");
  eq("FIX: shouty", ["bug"], "case-insensitive type");

  eq("", [], "empty title");
  eq("no conventional prefix here", [], "no CC prefix");
  eq("wibble: unknown type", [], "unknown type -> (none)");
  eq("Merge branch main", [], "merge subject -> (none)");
  eq(null, [], "null title");

  process.stdout.write("::notice::type-label --self-test: all assertions passed\n");
}

if (process.argv.includes("--self-test")) {
  selfTest();
  process.exit(0);
}
