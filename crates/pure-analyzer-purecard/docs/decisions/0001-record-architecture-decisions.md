# 0001. Record architecture decisions

- **Status:** Accepted
- **Date:** 2026-07-04
- **Deciders:** Project maintainer

## Context

This kit is built primarily by an AI agent across many small changes. Decisions
made once — why a crate boundary exists, why a dependency was chosen, why a gate
is set where it is — are easily re-litigated or silently reversed when the person
(or agent) who made them isn't in the room. Comments and commit messages decay;
they don't give a future reader a durable, discoverable record of *why*.

We need a lightweight, greppable log of significant decisions that both humans and
the agent can consult before changing something load-bearing.

## Decision

We will record architecturally significant decisions as Architecture Decision
Records (ADRs), following Michael Nygard's format, as sequentially numbered
Markdown files in `docs/decisions/`. Each ADR is immutable once Accepted; to
change a decision we write a new ADR that supersedes the old one and update the
old one's status.

`docs/decisions/0000-adr-template.md` is the template. The agent proposes ADRs
through the normal PR flow; ADRs are **EVOLVABLE** content.

## Alternatives considered

- **Only commit messages / PR descriptions.** Not discoverable as a set, not
  stable (PRs get squashed, history gets rewritten), and no clear "current
  decisions" view.
- **A wiki or external doc.** Drifts from the code, isn't versioned with it, and
  the agent can't reliably read it in-session. Keeping ADRs in-repo means they
  travel with the code and are visible to the agent.
- **A single running decisions log.** Merge-conflict prone and hard to supersede
  cleanly; one file per decision keeps diffs and history clean.

## Consequences

- Before changing a load-bearing choice, an author checks `docs/decisions/` and,
  if reversing a decision, writes a superseding ADR.
- CLAUDE.md links here so the agent treats ADRs as part of the ledger.
- Small, cheap, and greppable; the main cost is the discipline to actually write
  one when a decision is significant. The reviewer flags significant undocumented
  decisions.
