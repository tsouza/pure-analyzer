// Conventional Commits enforcement (constitution §2). Shared by the commit-msg
// git hook (lefthook) and the CI commitlint gate.
export default {
  extends: ["@commitlint/config-conventional"],
  rules: {
    // Dependabot's auto-generated body (compare URL + YAML trailer) isn't
    // prose we control, and a merely long dependency name can push one line
    // past 100 chars (observed on the markdownlint-cli2-action bump). This
    // disables line-wrapping enforcement only; header/type/subject rules
    // still apply to every commit, bot or human.
    "body-max-line-length": [0, "always"],
  },
};
