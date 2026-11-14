#!/usr/bin/env bash
# scripts/publish-crate.sh <tag> — publish the crate at <tag> to crates.io.
#
# Run from a checkout of the tag. Skips the upload when crates.io already
# has that version, so a release run that failed after this step can be
# retried. Needs CARGO_REGISTRY_TOKEN.
set -euo pipefail

tag="${1:?usage: publish-crate.sh <tag>}"
version="${tag#v}"

crate=$(sed -nE 's/^name = "(.*)"/\1/p' Cargo.toml | head -1)
manifest_version=$(sed -nE 's/^version = "(.*)"/\1/p' Cargo.toml | head -1)
if [ "$manifest_version" != "$version" ]; then
    echo "publish-crate: Cargo.toml is at ${manifest_version}, not ${version}; check out ${tag}" >&2
    exit 1
fi

# The sparse index lists every published version of a crate, one JSON
# object per line, at a path built from the lowercased name's length and
# letters.
name=$(tr '[:upper:]' '[:lower:]' <<<"$crate")
case "${#name}" in
    1) index_path="1/${name}" ;;
    2) index_path="2/${name}" ;;
    3) index_path="3/${name:0:1}/${name}" ;;
    *) index_path="${name:0:2}/${name:2:2}/${name}" ;;
esac
published=$(curl -fsSL "https://index.crates.io/${index_path}" || true)
if grep -qF "\"vers\":\"${version}\"" <<<"$published"; then
    echo "publish-crate: ${crate} ${version} is already on crates.io"
    exit 0
fi

cargo publish --locked
