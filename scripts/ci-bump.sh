#!/usr/bin/env bash
# scripts/ci-bump.sh <bundle> — cut the release commit and tag on a CI
# checkout.
#
# Runs `just release` on a local `main` (cocogitto bumps only on the
# branch named in cog.toml, and CI checks out a detached commit). When it
# cuts a release, writes the tag and its commit to <bundle>, a git bundle
# that needs the checked-out commit, and writes `tag`, `commit` and
# `tag_id` (the object the tag ref names) to $GITHUB_OUTPUT. Writes
# nothing when no release is due.
set -euo pipefail

bundle=${1:?usage: ci-bump.sh <bundle>}
: "${GITHUB_OUTPUT:?ci-bump: GITHUB_OUTPUT is not set}"

git checkout -q -B main
git config user.name "github-actions[bot]"
git config user.email "41898282+github-actions[bot]@users.noreply.github.com"
before=$(git rev-parse HEAD)
just release
if [ "$(git rev-parse HEAD)" != "$before" ]; then
    tag=$(git describe --tags --exact-match --match 'v[0-9]*' HEAD)
    git bundle create -q "$bundle" "refs/tags/${tag}" "^${before}"
    {
        echo "tag=${tag}"
        echo "commit=$(git rev-parse HEAD)"
        echo "tag_id=$(git rev-parse "refs/tags/${tag}")"
    } >>"$GITHUB_OUTPUT"
fi
