#!/usr/bin/env bash
# scripts/check-fix-scope.sh — reject `fix` commits whose scope is tooling.
#
# Reads commit subjects on stdin, one per line. `fix` cuts a patch
# release, so a `fix(ci):`, `fix(release):`, `fix(build):` or `fix(deps):`
# commit would publish a version with no change to the crate. Use `ci:`
# or `build:` for those. cog lowercases the type, so `Fix(ci):` counts too.
# Exits 1 and names each offending subject.
set -euo pipefail

bad=$(grep -iE '^fix\((ci|release|build|deps)\)!?:' || true)
if [ -n "$bad" ]; then
    echo "check-fix-scope: a tooling fix would cut a release with no crate change:" >&2
    printf '  %s\n' "$bad" >&2
    echo "  hint: use ci: for CI fixes and build: for release or dependency tooling" >&2
    exit 1
fi
