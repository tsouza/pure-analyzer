# Spec: readme-domain-focus

- Status: draft
- Created: 2026-07-22
- Owner: user request (mid-session feedback)

## Problem

The bootstrapped README.md leads with the starter kit's AI-driven-engineering
methodology ("Why it exists" framed around deterministic gates / generator-
reviewer split / self-learning, plus a "Working with the agent" section) before
a reader learns what pure-analyzer actually is or does. A newcomer landing on
the repo shouldn't have to read about the agent workflow to find out this is a
static-analysis toolchain for Legend Pure with a milestoning-arity checker.
User feedback: focus the README on the pure-analyzer domain, not the kit's own
point of view; mention the constitution/methodology only as a brief pointer.

## Goals

- [x] README leads with what pure-analyzer is, why it exists (the milestoning
      arity footgun — a domain reason, not a methodology reason), and what its
      subcommands do (validate/lint/eq/fmt/LSP).
- [x] Crate layout section stays (factual, not methodology-preachy).
- [x] Methodology/constitution mentioned only as a one-line pointer to
      CONTRIBUTING.md, not as a headline section.

## Non-goals

- Not rewriting CLAUDE.md, constitution.md, or docs/methodology/ — those
  remain the correct home for methodology content.
- Not removing the "optional gates" (CodSpeed/public-api) operational note —
  out of scope for this pass; can return in a follow-up if still wanted.

## Design

Docs-only change to `README.md` at the repo root. No crate touched, no DAG
impact.

## API / contract impact

None — documentation only.

## Testing plan

`just lint-md` (markdown lint) is the only applicable gate; no code paths
touched.

## Risks & rollout

None — a direct, reversible doc edit.
