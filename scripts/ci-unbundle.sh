#!/usr/bin/env bash
# scripts/ci-unbundle.sh <bundle> — check out the bump that ci-bump.sh
# bundled.
#
# WANT_TAG, WANT_COMMIT and WANT_TAG_ID are ci-bump.sh's outputs, taken
# from the job that wrote the bundle. Fetches the tag from the bundle,
# refuses unless the tag ref and its commit are exactly those, and checks
# out `main` at the commit. The bundle passes through jobs that build
# code, which can rewrite it; the expected values come from a job that
# built nothing, so a rewritten bundle fails here.
set -euo pipefail

bundle=${1:?usage: ci-unbundle.sh <bundle>}
: "${WANT_TAG:?ci-unbundle: WANT_TAG is not set}"
: "${WANT_COMMIT:?ci-unbundle: WANT_COMMIT is not set}"
: "${WANT_TAG_ID:?ci-unbundle: WANT_TAG_ID is not set}"

git fetch -q "$bundle" "refs/tags/${WANT_TAG}:refs/tags/${WANT_TAG}"
tag_id=$(git rev-parse "refs/tags/${WANT_TAG}")
commit=$(git rev-parse "refs/tags/${WANT_TAG}^{commit}")
if [ "$tag_id" != "$WANT_TAG_ID" ] || [ "$commit" != "$WANT_COMMIT" ]; then
    echo "ci-unbundle: ${WANT_TAG} in the bundle names ${tag_id} (commit ${commit});" \
        "the bump job cut ${WANT_TAG_ID} (commit ${WANT_COMMIT})" >&2
    exit 1
fi
git checkout -q -B main "$commit"
