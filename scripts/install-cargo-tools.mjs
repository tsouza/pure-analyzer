#!/usr/bin/env bun
// Installs pinned cargo-based dev tools with --locked (reproducible builds).
// Cargo tools aren't distributed via mise, so this is the single source of
// truth for versions; CI and local dev both run this script
// (via `mise run install-cargo-tools`).
import { $ } from "bun";

const TOOLS = [
  "cargo-nextest@0.9.138",
  "cargo-mutants@27.1.0",
  "cargo-deny@0.19.9",
  "cargo-audit@0.22.2",
  "cargo-llvm-cov@0.8.7",
  "cargo-machete@0.9.2",
  "cargo-semver-checks@0.48.0",
  "cargo-public-api@0.52.0",
  "cargo-fuzz@0.13.2",
];

for (const tool of TOOLS) {
  await $`cargo install --locked ${tool}`;
}

// Wire the git hooks declared in lefthook.yml (commit-msg → commitlint,
// pre-commit → fmt/markdownlint/gitleaks, …). A declared-but-uninstalled hook is
// not a gate: without this step a fresh clone silently skips the local checks and
// only discovers a bad commit message in CI. This is part of the one-command
// onboarding (`mise run install-cargo-tools`), so setup can't leave the hooks
// unwired. Skipped in CI, whose ephemeral checkout makes no commits and whose
// gates already run commitlint directly.
if (!process.env.CI) {
  await $`lefthook install`;
}
