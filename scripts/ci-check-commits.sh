#!/usr/bin/env bash
# scripts/ci-check-commits.sh — check the commits a CI run brings in
# against Conventional Commits.
#
# Reads the GitHub event from the environment: EVENT_NAME, REF (the
# pushed ref), PR_BASE (origin/<base branch> on a pull request),
# PUSH_BEFORE and PUSH_AFTER. `just test-release` runs every branch.
set -euo pipefail

zero=0000000000000000000000000000000000000000

if [ "$EVENT_NAME" = "pull_request" ]; then
    exec cog check "${PR_BASE}..HEAD"
fi

if [ "$REF" = "refs/heads/main" ]; then
    if [ "$PUSH_BEFORE" != "$zero" ] \
        && git cat-file -e "${PUSH_BEFORE}^{commit}" 2>/dev/null; then
        exec cog check "${PUSH_BEFORE}..${PUSH_AFTER}"
    fi
    # The first push of the repository, or a force-push (which trunk
    # rules forbid): check the whole history.
    exec cog check
fi

# Any other branch or tag: check what main lacks. A ref on a main commit
# brings nothing new. Say so here: `cog check X..X` still checks X.
if base=$(git merge-base origin/main "$PUSH_AFTER" 2>/dev/null); then
    if [ "$base" = "$(git rev-parse "$PUSH_AFTER")" ]; then
        echo "ci-check-commits: ${REF} adds no commits to main"
        exit 0
    fi
    exec cog check "${base}..${PUSH_AFTER}"
fi
exec cog check
