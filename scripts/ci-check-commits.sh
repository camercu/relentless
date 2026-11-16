#!/usr/bin/env bash
# scripts/ci-check-commits.sh — check the commits a CI run brings in
# against Conventional Commits.
#
# Reads the GitHub event from the environment: EVENT_NAME, REF (the
# pushed ref), PR_BASE (origin/<base branch> on a pull request),
# PUSH_BEFORE and PUSH_AFTER. `just check-commits` runs it on HEAD as a
# push of a ref other than main. `just test-release` runs every branch.
set -euo pipefail

zero=0000000000000000000000000000000000000000
# check [range] — no range means the whole history.
check() {
    cog check "$@"
}

if [ "$EVENT_NAME" = "pull_request" ]; then
    check "${PR_BASE}..HEAD"
    exit
fi

if [ "$REF" = "refs/heads/main" ]; then
    if [ "$PUSH_BEFORE" != "$zero" ] \
        && git cat-file -e "${PUSH_BEFORE}^{commit}" 2>/dev/null; then
        check "${PUSH_BEFORE}..${PUSH_AFTER}"
        exit
    fi
    # The first push of the repository, or a force-push (which trunk
    # rules forbid): check the whole history.
    check
    exit
fi

# Any other branch or tag: check what main lacks. A ref on a main commit
# brings nothing new. Say so here: `cog check X..X` still checks X.
if base=$(git merge-base origin/main "$PUSH_AFTER" 2>/dev/null); then
    if [ "$base" = "$(git rev-parse "${PUSH_AFTER}^{commit}")" ]; then
        echo "ci-check-commits: ${REF} adds no commits to main"
        exit 0
    fi
    check "${base}..${PUSH_AFTER}"
    exit
fi
echo "ci-check-commits: no origin/main to compare against; checking the whole history" >&2
check
