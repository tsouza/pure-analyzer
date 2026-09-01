# Pure Analyzer user guide

`pure-analyzer` is a deterministic command-line static analyzer for Legend
Pure. It validates source syntax, lints model-aware queries, formats supported
source files, and explains registered diagnostic and reason identifiers.

## Commands

| Command                                              | Purpose                                                                                                     |
| ---------------------------------------------------- | ----------------------------------------------------------------------------------------------------------- |
| `pure-analyzer validate <input>...`                  | Check grammar and shallow well-formedness. No model is required.                                            |
| `pure-analyzer lint <input>... [--model <model>]...` | Check model-aware milestoning arity, unknown properties, and cardinality misuse when models are supplied.   |
| `pure-analyzer fmt <input>...`                       | Canonically format input. File input is updated by default; standard input is formatted to standard output. |
| `pure-analyzer explain <identifier>`                 | Explain one exact registered diagnostic or reason identifier.                                               |
| `pure-analyzer completions bash`                     | Print Bash completion code to standard output.                                                              |

Use `pure-analyzer <command> --help` for the complete flag syntax.

### Inputs

Every analysis command takes one or more input paths, globs, or a single `-`
for standard input. Globs support `*`, `?`, character classes such as `[ab]`,
and recursive `**`; matches are processed in deterministic path order and
duplicate files are analyzed once. Quote globs when the shell would otherwise
expand them:

```text
pure-analyzer validate 'src/**/*.pure'
pure-analyzer fmt 'queries/*.pure' --check
```

Inputs must resolve to regular files. A missing file, a glob with no matches,
or a second `-` is a usage error. A glob cannot traverse above the current
working directory.

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

`lint` retains model-free validation without a model. Supply one or more model
sources for model-aware checks and fixes. Repeat `--model` for a PMCD JSON model
or a Pure model file; `.json` selects PMCD and `.pure` selects Pure:

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

Normal `validate` and `lint` diagnostics go to standard output. Formatter
diagnostics and every `lint --fix` diagnostic go to standard error. `explain`
writes one requested explanation to standard output. This leaves standard
output clean for fixed source, diffs, or script-consumable explain JSON.
`--quiet` suppresses normal diagnostic rendering without changing exit status
or requested source/diff output.

This makes it safe to route data and diagnostics independently:

```text
pure-analyzer lint query.pure --model model.json --fix --diff --format sarif \
  > fixes.diff 2> diagnostics.sarif
```

## Exit status

| Status | Meaning                                                                         |
| ------ | ------------------------------------------------------------------------------- |
| `0`    | Successful execution with no actionable diagnostic or unapplied preview change. |
| `1`    | Actionable diagnostic, or a change detected by a non-writing check/diff mode.   |
| `2`    | Reserved for a future indecisive result.                                        |
| `3`    | Usage, input, model, or configuration failure.                                  |
| `4`    | Internal failure, including a failed safe-write invariant.                      |

Errors in the `3` and `4` classes are written to standard error without normal
diagnostic output on standard output.

## Safe file updates

Default `fmt` and file-based `lint --fix` stage the complete set of replacements
before installing them. They refuse symbolic links and non-regular files,
re-check each analyzed snapshot, and use atomic path exchange where the platform
provides it. If safe atomic exchange is unavailable, the command fails rather
than falling back to an unsafe overwrite.

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
