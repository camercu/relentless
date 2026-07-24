// commitlint configuration. Run via the commit-msg pre-commit hook and
// the Commitlint CI job. Reference: https://commitlint.js.org/
module.exports = {
  extends: ['@commitlint/config-conventional'],
  ignores: [
    // Dependabot commit bodies routinely embed release-note URLs that
    // exceed body-max-line-length. Their subjects are already
    // conventional (`chore(deps):` / `ci(deps):`), so skipping the body
    // lint is the targeted fix; human commits still get the full rule set.
    (commit) => /Signed-off-by:\s*dependabot\[bot\]/m.test(commit),
  ],
};
