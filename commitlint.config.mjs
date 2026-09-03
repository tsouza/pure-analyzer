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
    // A squash-merge commit's body is the PR's own "Implementation evidence"
    // prose, which routinely carries an unbroken backtick-wrapped identifier
    // or a chain of two/three joined by `/` (a paired test-name reference,
    // a long crate path) past 100 chars with no natural wrap point — the
    // same shape body-max-line-length is already relaxed for above.
    // commitlint's conventional-commits-parser can classify a body paragraph
    // this deep into a long, multi-section message as part of the FOOTER
    // rather than the body (observed on PR #375's own merge commit, whose
    // `n3f_forbids_.../n3i_forbids_...` paired test-name line — mid-body,
    // nowhere near the trailing `Co-authored-by:` trailer — tripped
    // footer-max-line-length instead of the already-disabled
    // body-max-line-length). Relaxed for the identical reason and to the
    // identical scope.
    "footer-max-line-length": [0, "always"],
  },
};
