#!/usr/bin/env bash
# scripts/test-release.sh — check the release recipe and publish scripts.
#
# Builds a scratch repository from this working tree (tracked and
# untracked files, minus ignored ones), seeds a tag, adds empty commits,
# runs `just release`, and checks the tag it cuts, the bump commit, and
# the refusals, from a 0.x and a 1.x version. Then runs the publish
# scripts against stub `gh` and `cargo`. Uncommitted changes are what the test exercises, and a
# repository with no commits yet works.
set -euo pipefail

for tool in just cog; do
    if ! command -v "$tool" >/dev/null 2>&1; then
        echo "test-release: $tool is not on PATH; run inside nix-shell" >&2
        exit 1
    fi
done

repo=$(cd "$(dirname "$0")/.." && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

mkdir "$work/clone"
# A file deleted in the working tree but still in the index has nothing
# to copy.
git -C "$repo" ls-files -z --cached --others --exclude-standard \
    | while IFS= read -r -d '' f; do
        if [ -e "$repo/$f" ]; then printf '%s\0' "$f"; fi
    done \
    | (cd "$repo" && tar --null -T - -cf -) \
    | tar -xf - -C "$work/clone"
cd "$work/clone"
git init -q -b main
git config user.name "test-release"
git config user.email "test-release@example.invalid"
git config commit.gpgsign false
git config tag.gpgsign false
git add -A
git commit -q -m "chore: working tree under test"
tree=$(git rev-parse HEAD)

failures=0

# use_version <x.y.z>
# Commits the crate at that version, which later cases start from, and
# sets seed and the next patch, minor and major tags.
use_version() {
    git reset -q --hard "$tree"
    git tag -l | xargs -r git tag -d >/dev/null
    just set-version "$1" >"$work/out.log" 2>&1
    git commit -q -am "chore: crate at $1"
    base=$(git rev-parse HEAD)
    seed="v$1"
    IFS=. read -r major minor patch <<<"$1"
    next_patch="v${major}.${minor}.$((patch + 1))"
    next_minor="v${major}.$((minor + 1)).0"
    next_major="v$((major + 1)).0.0"
}

pass() { echo "ok   [$1]${2:+ -> $2}"; }

# fail <label> <reason> [file to show]
fail() {
    echo "FAIL [$1]: $2"
    if [ -n "${3:-}" ] && [ -e "$3" ]; then sed 's/^/    /' "$3"; fi
    failures=$((failures + 1))
}

reset() {
    git reset -q --hard "$base"
    git tag -l | xargs -r git tag -d >/dev/null
}

newest_tag() {
    git describe --tags --abbrev=0 --match 'v[0-9]*' 2>/dev/null || echo none
}

# expect_tag <want> [commit message]...
# Seeds the tag, adds one empty commit per message, releases, compares.
# With want equal to the seed, HEAD must not move either. Sets `released`
# to 1 on success, so a later check that builds on the release can tell.
expect_tag() {
    local want="$1" label got msg before
    shift
    released=0
    label="${*:-no commits}"
    label="${label:0:60}"
    reset
    git tag "$seed"
    for msg in "$@"; do
        git commit -q --allow-empty -F - <<<"$msg"
    done
    before=$(git rev-parse HEAD)
    if ! just release >"$work/out.log" 2>&1; then
        fail "$label" "just release exited non-zero" "$work/out.log"
        return
    fi
    got=$(newest_tag)
    if [ "$got" != "$want" ]; then
        fail "$label" "want ${want}, got ${got}"
        return
    fi
    if [ "$want" = "$seed" ] && [ "$(git rev-parse HEAD)" != "$before" ]; then
        fail "$label" "no release was due, but HEAD moved"
        return
    fi
    released=1
    pass "$label" "$got"
}

# expect_refusal <label> [tag to seed]
# `just release` must exit non-zero and leave HEAD where it was.
expect_refusal() {
    local label="$1" head
    reset
    if [ -n "${2:-}" ]; then git tag "$2"; fi
    git commit -q --allow-empty -m "fix: bug"
    head=$(git rev-parse HEAD)
    if just release >"$work/out.log" 2>&1; then
        fail "$label" "just release succeeded"
    elif [ "$(git rev-parse HEAD)" != "$head" ]; then
        fail "$label" "HEAD moved"
    else
        pass "$label" refused
    fi
}

use_version 0.4.2
expect_tag "$next_patch" "fix: bug"
expect_tag "$next_patch" "perf: faster"
expect_tag "$next_patch" "revert: undo thing"
expect_tag "$next_minor" "feat: thing"
expect_tag "$next_minor" "fix: bug" "feat: thing"
expect_tag "$seed" "docs: words" "ci: fix the gate" "chore(deps): bump"
expect_tag "$seed"
# Breaking changes before 1.0.0 bump the minor version, whatever the type.
expect_tag "$next_minor" "feat!: thing"
expect_tag "$next_minor" "fix!: bug"
expect_tag "$next_minor" "refactor!: shape"
expect_tag "$next_minor" "fix(core)!: bug"
expect_tag "$next_minor" "Fix!: bug"
expect_tag "$next_minor" "docs: words

BREAKING CHANGE: the output differs"
expect_tag "$next_minor" "fix: bug

BREAKING CHANGE: the output differs"
expect_tag "$next_minor" "fix: bug

BREAKING-CHANGE: the output differs"
# A changelog larger than a pipe buffer is what makes a piped `grep -q`
# miss a breaking commit: grep exits on the match and the writer dies of
# SIGPIPE. The breaking commit comes last, so it is listed first.
huge=$(head -c 2000000 /dev/zero | tr '\0' x)
expect_tag "$next_minor" "fix: ${huge}" "fix!: bug"

# A stale or missing tag must not lower the crate version.
expect_refusal "no tag"
expect_refusal "tag behind Cargo.toml" v0.0.1

# From 1.0.0 on, a breaking change bumps the major version.
use_version 1.4.2
expect_tag "$next_patch" "fix: bug"
expect_tag "$next_minor" "feat: thing"
expect_tag "$next_major" "fix!: bug"
expect_tag "$next_major" "refactor!: shape"
expect_tag "$seed" "docs: words"
use_version 0.4.2

# The bump commit carries the version files and the changelog, is the
# tagged commit, sets the package version and nothing else in Cargo.toml,
# and keeps the older changelog entries below the new one.
label="bump commit"
expect_tag "$next_minor" "feat: thing"
if [ "$released" = 1 ]; then
    files=$(git show --name-only --format= HEAD | sort | paste -sd' ' -)
    manifest_diff=$(git diff --unified=0 HEAD~1 HEAD -- Cargo.toml | grep -E '^[-+][^-+]' || true)
    want_diff=$(printf -- '-version = "%s"\n+version = "%s"' "${seed#v}" "${next_minor#v}")
    if [ "$files" != "CHANGELOG.md Cargo.lock Cargo.toml" ]; then
        fail "$label" "files were '${files}'"
    elif [ "$(git describe --tags --exact-match HEAD 2>/dev/null)" != "$next_minor" ]; then
        fail "$label" "HEAD is not tagged ${next_minor}"
    elif [ "$manifest_diff" != "$want_diff" ]; then
        printf '%s\n' "$manifest_diff" >"$work/diff.txt"
        fail "$label" "Cargo.toml change was:" "$work/diff.txt"
    elif ! grep -qF "version = \"${next_minor#v}\"" Cargo.lock; then
        fail "$label" "Cargo.lock does not hold ${next_minor#v}"
    elif git cat-file -e HEAD~1:CHANGELOG.md 2>/dev/null \
        && old=$(git show HEAD~1:CHANGELOG.md | sed '1,/^- - -$/d') \
        && [ "$(tail -n "$(wc -l <<<"$old")" CHANGELOG.md)" != "$old" ]; then
        fail "$label" "older changelog entries changed"
    else
        pass "$label"
    fi
fi

# The release workflow runs the publish scripts by path, so they must be
# executable. Stubs on PATH record what the scripts call.
mkdir "$work/bin"
stub() { cat >"$work/bin/$1"; chmod +x "$work/bin/$1"; }
stubbed() { PATH="$work/bin:$PATH" STUB_LOG="$work/stub.log" "$@" >"$work/out.log" 2>&1; }

label="publish-release"
expect_tag "$next_minor" "feat!: thing"
if [ "$released" = 1 ]; then
    stub gh <<'GH'
#!/usr/bin/env bash
# No release exists yet.
if [ "$1 $2" = "release view" ]; then exit 1; fi
printf '%s\n' "$*" >"$STUB_LOG"
while [ "$#" -gt 0 ]; do
    if [ "$1" = "--notes-file" ]; then cat "$2" >>"$STUB_LOG"; fi
    shift
done
GH
    rm -f "$work/stub.log"
    if ! stubbed "$repo/scripts/publish-release.sh" "$next_minor"; then
        fail "$label" "exited non-zero" "$work/out.log"
    elif [ "$(head -1 "$work/stub.log" | cut -d' ' -f1-6)" \
        != "release create ${next_minor} --verify-tag --title ${next_minor}" ]; then
        fail "$label" "unexpected gh call: $(head -1 "$work/stub.log")"
    elif ! grep -qF -- "$next_minor" "$work/stub.log" \
        || ! grep -qF -- '**BREAKING** thing' "$work/stub.log" \
        || grep -qF '<span' "$work/stub.log"; then
        fail "$label" "notes are not the ${next_minor} entry with a plain breaking marker" "$work/stub.log"
    else
        pass "$label"
    fi

    # A retry must not create a second release for the same tag.
    stub gh <<'GH'
#!/usr/bin/env bash
# The release already exists.
if [ "$1 $2" = "release view" ]; then exit 0; fi
printf '%s\n' "$*" >"$STUB_LOG"
GH
    rm -f "$work/stub.log"
    if ! stubbed "$repo/scripts/publish-release.sh" "$next_minor"; then
        fail "$label retry" "exited non-zero" "$work/out.log"
    elif [ -e "$work/stub.log" ]; then
        fail "$label retry" "called gh $(head -1 "$work/stub.log")"
    elif ! grep -qF "already has a GitHub Release" "$work/out.log"; then
        fail "$label retry" "did not report the skip" "$work/out.log"
    else
        pass "$label retry"
    fi

    if stubbed "$repo/scripts/publish-release.sh" v9.9.9; then
        fail "$label unknown tag" "succeeded"
    else
        pass "$label unknown tag" refused
    fi
fi

# publish-crate.sh against a stub cargo. STUB_PUBLISHED=1 makes
# `cargo info` find the version on crates.io.
stub cargo <<'CARGO'
#!/usr/bin/env bash
printf 'cargo %s\n' "$*" >>"$STUB_LOG"
if [ "$1" = info ]; then [ "${STUB_PUBLISHED:-0}" = 1 ]; fi
CARGO

# crate_case <label> <manifest version> <tag> <published 0|1>
# Runs publish-crate.sh in a directory holding only a Cargo.toml.
crate_case() {
    local dir="$work/crate-$1"
    mkdir -p "$dir"
    printf '[package]\nname = "demo-crate"\nversion = "%s"\n' "$2" >"$dir/Cargo.toml"
    rm -f "$work/stub.log"
    (cd "$dir" && STUB_PUBLISHED="$4" stubbed "$repo/scripts/publish-crate.sh" "$3")
}

if crate_case skip 0.2.0 v0.2.0 1 \
    && grep -qx 'cargo info --registry crates-io demo-crate@0.2.0' "$work/stub.log" \
    && ! grep -q '^cargo publish' "$work/stub.log" \
    && grep -qF "already on crates.io" "$work/out.log"; then
    pass "publish-crate already published"
else
    fail "publish-crate already published" "did not ask crates.io and skip" "$work/stub.log"
fi
if crate_case new 0.3.0 v0.3.0 0 \
    && grep -qx 'cargo publish --locked' "$work/stub.log"; then
    pass "publish-crate new version"
else
    fail "publish-crate new version" "did not run cargo publish --locked" "$work/stub.log"
fi
if crate_case mismatch 0.2.0 v0.3.0 0; then
    fail "publish-crate version mismatch" "succeeded"
elif grep -q '^cargo' "$work/stub.log" 2>/dev/null; then
    fail "publish-crate version mismatch" "ran cargo" "$work/stub.log"
else
    pass "publish-crate version mismatch" refused
fi

if [ "$failures" -ne 0 ]; then
    echo "test-release: ${failures} failure(s)"
    exit 1
fi
echo "test-release: all cases pass"
