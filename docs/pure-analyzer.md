# Pure Analyzer user guide

`pure-analyzer` is a deterministic command-line static analyzer for Legend
Pure. It validates source syntax, lints model-aware queries, formats supported
source files, compares supported relational query pairs, and explains registered
diagnostic and reason identifiers.

## Commands

| Command                                                  | Purpose                                                                                                                                                                   |
| -------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `pure-analyzer validate <input>...`                      | Check grammar and shallow well-formedness. No model is required.                                                                                                          |
| `pure-analyzer lint <input>... [--model <model>]...`     | Check model-aware milestoning arity, unknown properties, and cardinality misuse when models are supplied.                                                                 |
| `pure-analyzer fmt <input>...`                           | Format input, preserving source layout and comments. File input is updated by default; standard input is formatted to standard output.                                    |
| `pure-analyzer eq <left> <right> [--model <model>]...`   | Sound, fail-closed M4a relational comparison, reporting a proven structural schema distinction when one exists. Also invocable as `diff`, an alias for this same command. |
| `pure-analyzer explain <identifier>`                     | Explain one exact registered diagnostic or reason identifier.                                                                                                             |
| `pure-analyzer completions bash`                         | Print Bash completion code to standard output.                                                                                                                            |

Use `pure-analyzer <command> --help` for the complete flag syntax.

### Inputs

`validate`, `lint`, and `fmt` take one or more input paths, globs, or a single
`-` for standard input. Globs support `*`, `?`, character classes such as
`[ab]`, and recursive `**`; matches are processed in deterministic path order
and duplicate files are analyzed once. Quote globs when the shell would
otherwise expand them:

```text
pure-analyzer validate 'src/**/*.pure'
pure-analyzer fmt 'queries/*.pure' --check
```

Inputs must resolve to regular files. A missing file, a glob with no matches,
or a second `-` is a usage error. A glob cannot traverse above the current
working directory.

`eq` and `diff` each take exactly two positional operands. Each file/glob
operand must resolve to exactly one file; using the same path for both operands
is valid. At most one operand may be `-`, because standard input is a single
snapshot.

`fmt -` formats standard input to standard output. In-place formatter writes
cannot mix file input and standard input. `lint --fix` cannot write standard
input; use one of its non-writing modes instead.

## Validate

`validate` needs no model:

```text
pure-analyzer validate 'src/**/*.pure'
pure-analyzer validate query.pure --strict
```

`--strict` promotes shape-level validation warnings to errors. Use
`--no-strict` to override configured strict validation for one invocation.

## Lint and fixes

`lint` retains model-free validation without a model. `eq` and `diff` lower
both inputs against one optional model graph. Supply one or more model sources
for model-aware checks, fixes, or comparison. Repeat `--model` for a PMCD JSON
model or a Pure model file; `.json` selects PMCD and `.pure` selects Pure:

```text
pure-analyzer lint query.pure --model model.json --format json
pure-analyzer lint query.pure --model shared.pure --model local.json
```

Model paths can also come from configuration. They are deduplicated after
resolution; an unreadable model, unsupported extension, or unusable model is a
usage error.

`--fix` enables machine-applicable fixes. Its modes are deliberately explicit:

| Invocation              | Result                                                                         |
| ----------------------- | ------------------------------------------------------------------------------ |
| `lint … --fix`          | Apply fixes to file inputs.                                                    |
| `lint … --fix --check`  | Report whether fixes would change input; do not write.                         |
| `lint … --fix --stdout` | Print one fixed input to standard output; requires exactly one resolved input. |
| `lint … --fix --diff`   | Print compact fixed diffs; do not write.                                       |

For example, a CI check can reject unapplied fixes without changing the
checkout:

```text
pure-analyzer lint 'queries/**/*.pure' --model model.pure --fix --check
```

## Format

The default `fmt` mode updates file inputs. Use a read-only mode when reviewing
or automating changes:

| Invocation               | Result                                                                         |
| ------------------------ | ------------------------------------------------------------------------------ |
| `fmt <input>...`         | Format file inputs in place, or format sole standard input to standard output. |
| `fmt <input>... --check` | Exit nonzero when formatting would change input; do not write.                 |
| `fmt <input> --stdout`   | Print formatted text; requires exactly one resolved input.                     |
| `fmt <input>... --diff`  | Print compact formatting diffs; do not write.                                  |

`--line-width <positive-integer>` overrides the configured layout width for
one invocation.

### Canonical emission

`fmt --canonical` is a separate mode from the layout formatter above. It does
not rewrite a source file; it emits the proven relational normal form of one
query to standard output, and it never writes input.

| Invocation                                       | Result                                                                |
| ------------------------------------------------ | --------------------------------------------------------------------- |
| `fmt <input> --canonical [--model <model>]...`   | Emit the proven normal form, or report an indecisive result.          |

Because a normal form is derived from the lowered query rather than from the
source text, canonical mode **discards comments and all source layout**. It
never re-attaches them, and it never claims to have preserved them. Use the
default `fmt` mode when comment and layout preservation matter; the two modes
are mutually exclusive, and `--canonical` is rejected together with `--check`,
`--stdout`, `--diff`, and `--line-width`.

Canonical mode is fail-closed. When the query lies outside the sound canonical
subset, it emits no text and reports the exact reason with the source origin
that produced it.

| Exit status | Meaning                                                       |
| ----------- | ------------------------------------------------------------- |
| `0`         | A proven normal form was emitted.                             |
| `2`         | The request was indecisive; no normal form was emitted.       |

## Compare

`eq` (also invocable as `diff`, a plain alias for the same command — not a
separate mode) runs one conservative M4a comparison. It proves equivalence
only when both lowered queries have the same normal-form identity, and proves
non-equivalence only for incompatible ordered output schemas, always
reporting the exact schema distinction that proved it. Every other case is
indecisive; the command never invokes an engine, synthesizes a data witness,
or guesses from partial facts.

```text
pure-analyzer eq left.pure right.pure --model model.pure
pure-analyzer diff left.pure right.pure --model model.json --format json
```

Human and JSON output include the typed outcome. A structural refutation names
its canonical `primary_origin` and `secondary_origin` and exact schema detail;
it intentionally contains no M4b witness. Malformed queries, unsupported
syntax, absent or unresolved model facts, and normalization limits render a
typed indecision. `--format sarif` is not available for comparison commands.

## Explain

`explain` resolves one exact, case-sensitive registered diagnostic identifier
such as `PUR2001` or reason identifier such as `IND_WINDOW` through the shared
catalog:

```text
pure-analyzer explain PUR2001
pure-analyzer explain IND_WINDOW --format json
```

The default human rendering and `--format json` both write the complete
explanation to standard output. JSON is one pretty-printed object containing
the identifier, kind, classification, meaning, limit, remedy, and documentation
URL. `--format sarif` is not available for explanations. An unknown identifier
or unsupported output format is a usage error written to standard error.

## Configuration and diagnostic policy

Configuration is resolved from lowest to highest precedence:

1. Built-in defaults.
2. User configuration: `pure-analyzer/config.toml` below `XDG_CONFIG_HOME`,
   or below `~/.config` when that variable is absent; on Windows, below
   `APPDATA`.
3. The nearest `.pure-analyzer.toml` found from the working directory upward,
   or the file named by `--config`.
4. `PURE_ANALYZER_*` environment variables.
5. Command-line flags.

`--config path` replaces repository-config discovery but still allows the user
configuration layer beneath it. `--no-config` disables both file layers only;
environment variables and flags still apply. `--print-config` writes the
complete resolved versioned configuration to standard output and does not need
a subcommand.

The file schema is versioned and rejects unknown keys. This is a complete
shape, with the built-in defaults shown:

```toml
version = 1
jobs = 1

[output]
format = "human" # human, json, or sarif
color = "auto"   # auto, always, or never
quiet = false

[lint]
select = []
ignore = []
deny = []
warn = []

[validate]
strict = false

[fmt]
line-width = 100

[model]
paths = []
```

Model paths in a configuration file are relative to that file. The policy
lists accept registered diagnostic codes such as `PUR2002` or one trailing-star
prefix such as `PUR2*`. `select` retains matching diagnostics, `ignore`
suppresses them, `deny` promotes them to errors, and `warn` makes them warnings.
The same code cannot be both denied and warned.

The supported environment variables mirror these settings:

| Variable                                                                                   | Value                                                             |
| ------------------------------------------------------------------------------------------ | ----------------------------------------------------------------- |
| `PURE_ANALYZER_JOBS`                                                                       | Positive integer source-file concurrency.                         |
| `PURE_ANALYZER_FORMAT`                                                                     | `human`, `json`, or `sarif`.                                      |
| `PURE_ANALYZER_COLOR`                                                                      | `auto`, `always`, or `never`.                                     |
| `PURE_ANALYZER_QUIET`, `PURE_ANALYZER_STRICT`                                              | `true` or `false`.                                                |
| `PURE_ANALYZER_SELECT`, `PURE_ANALYZER_IGNORE`, `PURE_ANALYZER_DENY`, `PURE_ANALYZER_WARN` | Comma-separated policy patterns.                                  |
| `PURE_ANALYZER_FMT_LINE_WIDTH`                                                             | Positive integer formatting width.                                |
| `PURE_ANALYZER_MODEL_PATHS`                                                                | Model paths separated with the operating system's path separator. |

An unknown `PURE_ANALYZER_*` variable is rejected rather than ignored.

## Output and streams

`--format` selects human-readable diagnostics (the default), versioned JSON
(`1.0`), or SARIF (`2.1.0`). `--color auto` detects whether the human-output
destination is a terminal; `always` and `never` force the choice.

Normal `validate`, `lint`, `eq`, and `diff` output goes to standard output.
Formatter diagnostics and every `lint --fix` diagnostic go to standard error.
`explain` writes one requested explanation to standard output. This leaves
standard output clean for fixed source, diffs, or script-consumable JSON.
`--quiet` suppresses normal rendering without changing exit status or requested
source/diff output.

This makes it safe to route data and diagnostics independently:

```text
pure-analyzer lint query.pure --model model.json --fix --diff --format sarif \
  > fixes.diff 2> diagnostics.sarif
```

## Exit status

| Status | Meaning                                                                                                                    |
| ------ | -------------------------------------------------------------------------------------------------------------------------- |
| `0`    | Successful execution with no actionable diagnostic or unapplied preview change; `eq`/`diff` proved equivalent.             |
| `1`    | Actionable diagnostic, a change detected by a non-writing check/diff mode, or `eq`/`diff` proved a structural distinction. |
| `2`    | `eq`/`diff` could not make a sound M4a commitment.                                                                         |
| `3`    | Usage, input, model, or configuration failure.                                                                             |
| `4`    | Internal failure, including a failed safe-write invariant.                                                                 |

Errors in the `3` and `4` classes are written to standard error without normal
diagnostic output on standard output.

## Safe file updates

Default `fmt` and file-based `lint --fix` stage the complete set of replacements
before installing any of them, and only start installing once every one has
staged successfully. They refuse symbolic links and non-regular files, re-check
each analyzed snapshot immediately before its own exchange, and use atomic path
exchange where the platform provides it — a file's content is replaced
completely or not at all, and its containing directory is fsynced immediately
after the exchange, so a completed replacement survives a crash or power loss.
If safe atomic exchange is unavailable, the command fails rather than falling
back to an unsafe overwrite.

This durability guarantee is **per file, not per invocation**: a run over many
files that is killed (`SIGKILL`, power loss) between two files' exchanges is not
rolled forward or back on its next run — there is no cross-file journal — so it
can leave an earlier file already replaced and a later one untouched. Every
file is still exactly its old or its new content; none is ever torn or
corrupted. A software error caught *during* a run (a stale snapshot, a failed
exchange) is different and stronger: it rolls back every file that run had
already installed, so a handled failure never leaves a partial edit behind.

A leftover `.<name>.pure-analyzer-stage-<pid>-<n>` file beside a target is the
signature of a run that was killed before it could clean up its own staging
file. It is never installed or silently deleted by a later run — deleting it
automatically could destroy a different, still-running invocation's live
staging file — a later run that touches the same directory instead logs a
warning naming it, so it can be inspected and removed by hand.

Formatter recovery diagnostics prevent default file writes. Use `--check`,
`--stdout`, or `--diff` to inspect input that needs recovery without modifying
it.

## Common failures

Use the error text and exit status to distinguish operational failures:

- Check paths and quote globs when an input is missing or a pattern matches no
  files.
- Supply valid, readable `.json` or `.pure` models to `lint`.
- Use `--print-config` to inspect precedence, then fix malformed TOML,
  unsupported configuration versions, invalid values, or unknown environment
  variables.
- Use a positive `--jobs` and `--line-width` value.
- Use preview modes for standard input when a write-capable command would need
  to modify it.
