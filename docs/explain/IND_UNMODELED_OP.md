# `IND_UNMODELED_OP`

[Back to the explain index](README.md)

- Kind: `reason`
- Classification: `recoverable`

## Meaning

A relational operator has no sound semantic model in the analyzer.

## Limit

This recoverable limitation does not make either query erroneous.

## Remedy

Avoid the operator for a core comparison or establish its semantics separately.
