# Methodology: Twelve-Factor Engineering

The monorepo's shared engineering guidance is indexed in the root
[methodology overview](../../../../docs/methodology/overview.md). PureCARD
follows its repository-wide change loop, deterministic gates, and review model
through the root `just` and CI entry points.

The root twelve-factor page describes the analyzer CLI/LSP product and is not a
PureCARD contract. PureCARD has no separate twelve-factor policy today; if its
host integration grows a service or configuration lifecycle, document the
product-specific adaptation here rather than inheriting analyzer rules.
