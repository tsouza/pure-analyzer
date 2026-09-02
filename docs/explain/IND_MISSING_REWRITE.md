# `IND_MISSING_REWRITE`

[Back to the explain index](README.md)

- Kind: `reason`
- Classification: `recoverable`

## Meaning

The normalizer lacks a known sound rewrite for the observed structural difference.

## Limit

This recoverable limitation does not make either query erroneous.

## Remedy

Use a simpler shared form or investigate a sound normalization rule.

## Verified M4a examples

These links are generated from the verified comparison corpus; query, model, and oracle details remain in that executable corpus.

- [`different-literal-filters-remain-indecisive`](../../crates/pure-analyzer-analysis/corpus/legend-4.113.0/comparison.jsonl#L3): verified `indecisive` verdict with reason `IND_MISSING_REWRITE`.

This `recoverable` reason records engineering backlog: a conservative implementation limitation. The result stays indecisive and the input stays valid.
