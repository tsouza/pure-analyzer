---
description: Scaffold and fill in a feature spec, then run the plan→implement→verify loop.
argument-hint: <feature-name>
allowed-tools: Bash(just spec:*), Bash(git status:*), Read, Write, Edit, Glob, Grep
---

Use the **`spec`** skill to plan the change named `$ARGUMENTS`.

1. Scaffold the spec: `just spec $ARGUMENTS`
2. Fill in the template (context, goal, non-goals, acceptance criteria → tests,
   risks, dependencies). Resolve open questions before any code.
3. Convert each acceptance criterion into a concrete test at the right layer
   (see the `layered-testing` skill).
4. Report the spec path and the acceptance-criteria → test mapping. Do not start
   implementing until the spec is filled in.

If `$ARGUMENTS` is empty, ask for the feature name first.
