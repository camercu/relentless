#!/usr/bin/env bash
# scripts/publish-release.sh <tag> — create the GitHub Release for a tag.
#
# The tag must already exist on the remote. Notes come from cocogitto's
# changelog for that tag. Skips a tag that already has a release, so a
# failed release run can be retried. Needs `gh` with GH_TOKEN set.
set -euo pipefail

tag="${1:?usage: publish-release.sh <tag>}"

if ! git rev-parse -q --verify "refs/tags/${tag}" >/dev/null; then
    echo "publish-release: no tag '${tag}' in this clone" >&2
    exit 1
fi

if gh release view "$tag" >/dev/null 2>&1; then
    echo "publish-release: ${tag} already has a GitHub Release"
    exit 0
fi

notes=$(mktemp)
trap 'rm -f "$notes"' EXIT

# cocogitto marks a breaking change with an inline-styled <span> and no
# space after it. GitHub strips the style, which leaves "BREAKINGsubject".
cog changelog --at "$tag" \
    | sed -E 's#<span[^>]*>BREAKING</span> *#**BREAKING** #' >"$notes"

gh release create "$tag" --verify-tag --title "$tag" --notes-file "$notes"
