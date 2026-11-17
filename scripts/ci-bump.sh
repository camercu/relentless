#!/usr/bin/env bash
# scripts/ci-bump.sh — cut the release commit and tag on a CI checkout.
#
# Runs `just release` on a local `main` (cocogitto bumps only on the
# branch named in cog.toml, and CI checks out a detached commit). When it
# cuts a release, writes `tag=<tag>` and `tree=<tree of the bump commit>`
# to $GITHUB_OUTPUT. Writes nothing when no release is due.
set -euo pipefail

: "${GITHUB_OUTPUT:?ci-bump: GITHUB_OUTPUT is not set}"

git checkout -q -B main
git config user.name "github-actions[bot]"
git config user.email "41898282+github-actions[bot]@users.noreply.github.com"
before=$(git rev-parse HEAD)
just release
if [ "$(git rev-parse HEAD)" != "$before" ]; then
    {
        echo "tag=$(git describe --tags --exact-match --match 'v[0-9]*' HEAD)"
        echo "tree=$(git rev-parse 'HEAD^{tree}')"
    } >>"$GITHUB_OUTPUT"
fi
