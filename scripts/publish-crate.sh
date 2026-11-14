#!/usr/bin/env bash
# scripts/publish-crate.sh <tag> — publish the crate at <tag> to crates.io.
#
# Run from a checkout of the tag. Skips the upload when crates.io already
# has that version, so a release run that failed after this step can be
# retried. Needs CARGO_REGISTRY_TOKEN.
set -euo pipefail

tag="${1:?usage: publish-crate.sh <tag>}"
version="${tag#v}"

# Read the [package] table only.
crate=$(sed -nE '/^\[package\]/,/^\[/ s/^name = "(.*)"/\1/p' Cargo.toml)
manifest_version=$(sed -nE '/^\[package\]/,/^\[/ s/^version = "(.*)"/\1/p' Cargo.toml)
if [ "$manifest_version" != "$version" ]; then
    echo "publish-crate: Cargo.toml is at ${manifest_version}, not ${version}; check out ${tag}" >&2
    exit 1
fi

# `--registry` makes cargo ask crates.io. Without it, cargo answers from
# this workspace, which always has the version. Exit 0 means published;
# anything else, an unreachable registry included, falls through to the
# upload, which then fails loudly if the version exists after all.
if cargo info --registry crates-io "${crate}@${version}" >/dev/null 2>&1; then
    echo "publish-crate: ${crate} ${version} is already on crates.io"
    exit 0
fi

cargo publish --locked
