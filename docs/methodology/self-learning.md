# Methodology: Guardrail Ratchet

The repository improves by turning repeatable failures into tests, lints, and
other deterministic checks. It does not keep a checked-in project journal.
Scope, ownership, progress, and acceptance criteria belong in GitHub Issues;
implementation evidence belongs in the corresponding PR.

## Two tiers

**EVOLVABLE** material describes durable present product or repository truth:

- the domain rules in [`constitution.md`](../../constitution.md),
- [`domain-model.md`](../domain-model.md),
- ADRs in [`decisions/`](../decisions/),
- maintained methodology and agent guidance.

Change it when the code or a durable decision changes. Do not use it for task
lists, status reports, or plans.

**PROTECTED** material is the quality bar. It may only become stricter without
an explicit maintainer decision:

- test thresholds (mutation score, coverage floor),
- the forbid-skip / postponed-marker gates,
- `cargo-deny` policy,
- cargo-vet audits, exemptions, publisher trust, criteria mappings, and import
  coverage,
- tool-install URL and checksum locks,
- the anti-gaming suites and reviewer configuration.

The default-branch ruleset and deterministic checks protect what can be checked
mechanically. A maintainer makes the residual judgment for changes that cannot
be classified mechanically.

## During a change

1. Fix a discovered problem in the current PR when it is in scope; otherwise
   open or update a GitHub Issue with its acceptance criteria.
2. Add a test, lint, hook, or rule when the failure class is mechanically
   decidable. The PR records the evidence for that addition.
3. Record an enduring architectural decision in an ADR only when it defines a
   lasting constraint or ownership boundary.

This keeps the repository focused on the software and its enduring contracts,
while GitHub carries the mutable work record.
