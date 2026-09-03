# Diagnostic and reason reference

`pure-analyzer` uses stable diagnostic and conservative-reason identifiers. Each entry below links to its exact user-facing reference.

A `fundamental` reason marks a soundness boundary, while a `recoverable` reason marks an implementation limitation; neither makes a query erroneous.

## Diagnostics

- [`PUR0101`](PUR0101.md): `lexer`. An island reaches end-of-input before its matching terminator.
- [`PUR0102`](PUR0102.md): `lexer`. The lexer encountered input that is not a token in the supported Pure surface.
- [`PUR1200`](PUR1200.md): `parser`. Source tokens do not form a complete supported declaration or expression.
- [`PUR1201`](PUR1201.md): `parser`. A parenthesized value tuple is admitted for targeted validation but is not accepted in that position.
- [`PUR1202`](PUR1202.md): `parser`. A bracket index is neither a string literal nor an integer literal.
- [`PUR1204`](PUR1204.md): `parser`. Milestoning parentheses have an invalid surface shape.
- [`PUR1210`](PUR1210.md): `parser`. A relation join kind is outside the supported closed set.
- [`PUR2001`](PUR2001.md): `lint`. A navigation supplies a number of milestoning dates that disagrees with a known target stereotype.
- [`PUR2002`](PUR2002.md): `lint`. A closed-world source class has no property with the requested name.
- [`PUR2101`](PUR2101.md): `lint`. Local inference cannot determine the source of a navigation.
- [`PUR9000`](PUR9000.md): `tool`. A later model input replaces an earlier definition with the same identity.
- [`PUR9002`](PUR9002.md): `tool`. One model source declares the same fact more than once.
- [`PUR9003`](PUR9003.md): `tool`. A Pure association cannot be materialized without ambiguity.

## Conservative reasons

- [`IND_WINDOW`](IND_WINDOW.md): `fundamental`. Window and OLAP-frame equivalence is outside the sound core.
- [`IND_PARETO`](IND_PARETO.md): `fundamental`. Pareto and top-per-group equivalence depends on unmodeled tie semantics.
- [`IND_MULTISTEP_FISCAL`](IND_MULTISTEP_FISCAL.md): `fundamental`. Multi-step fiscal accumulation equivalence is outside the sound core.
- [`IND_DIVISION_RATIO`](IND_DIVISION_RATIO.md): `fundamental`. Division and ratio equivalence is outside the sound core.
- [`IND_MILESTONING_ASOF`](IND_MILESTONING_ASOF.md): `fundamental`. Bitemporal as-of equivalence is outside the sound core.
- [`IND_ORDER_UNDERDETERMINED`](IND_ORDER_UNDERDETERMINED.md): `fundamental`. The available facts do not prove a total order for an order-sensitive operation.
- [`IND_OPAQUE_PREDICATE`](IND_OPAQUE_PREDICATE.md): `fundamental`. A predicate falls outside the sound interpreted whitelist.
- [`IND_DIFFERENT_SOURCES`](IND_DIFFERENT_SOURCES.md): `fundamental`. The two queries read different named sources.
- [`IND_MISSING_REWRITE`](IND_MISSING_REWRITE.md): `recoverable`. The normalizer lacks a known sound rewrite for the observed structural difference.
- [`IND_UNMODELED_OP`](IND_UNMODELED_OP.md): `recoverable`. A relational operator has no sound semantic model in the analyzer.
- [`IND_OPAQUE_FUNCTION_IN_WITNESS`](IND_OPAQUE_FUNCTION_IN_WITNESS.md): `recoverable`. Witness evaluation encountered a function whose semantics are not interpreted.
- [`IND_UNRESOLVED_SCHEMA`](IND_UNRESOLVED_SCHEMA.md): `recoverable`. The available model facts do not resolve a schema property required for a hard conclusion.
- [`IND_WITNESS_BUDGET_EXHAUSTED`](IND_WITNESS_BUDGET_EXHAUSTED.md): `recoverable`. Deterministic witness enumeration exhausted its configured budget without a proof.
- [`IND_PREDICATE_NORMAL_FORM_GAP`](IND_PREDICATE_NORMAL_FORM_GAP.md): `recoverable`. Predicate normalization cannot reach a proven canonical form.
- [`IND_UNPARSEABLE`](IND_UNPARSEABLE.md): `recoverable`. An input or deep-parsed island did not parse far enough for a hard conclusion.
- [`MODEL_INCOMPLETE`](MODEL_INCOMPLETE.md): `recoverable`. Model coverage is insufficient for a hard conclusion.
- [`RELATION_ROW_TYPE_UNKNOWN`](RELATION_ROW_TYPE_UNKNOWN.md): `recoverable`. A relation row's column types are unavailable.
