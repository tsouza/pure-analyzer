---
name: Bug report
about: Report a defect so we can reproduce and fix it (and add a regression test).
title: "bug: "
labels: ["bug"]
---

## Summary

A clear, one-sentence description of the bug.

## Expected behavior

What you expected to happen.

## Actual behavior

What actually happened. Include the full error / panic message and a backtrace
(`RUST_BACKTRACE=1`) if there is one.

## Reproduction

Minimal steps or a failing test. The more deterministic, the faster the fix.

```bash
# commands / code to reproduce
```

## Environment

- Product (`pure-analyzer` or `purecard`):
- Version / commit:
- OS + arch:
- `rustc --version`:
- Python version (if the wheel/FFI is involved):

## Corpus / oracle context

If this involves PureCARD, identify the corpus fixture, tokenizer revision, or
Legend version used. State whether the failure reproduces in a hermetic replay
or only against a live external oracle.

## Additional context

Logs, a captured chaos/fuzz seed, or anything else relevant.
