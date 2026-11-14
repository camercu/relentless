#!/usr/bin/env bash
# scripts/test-release.sh — check which version `just release` cuts.
#
# Builds a throwaway repository from this working tree (tracked and
# untracked files, minus ignored ones), seeds a tag, adds empty commits,
# runs `just release`, and compares the newest tag. Uncommitted changes
# are what the test exercises, and a repository with no commits yet works.
set -euo pipefail

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
# The long-history case below makes enough loose objects to start a
# background `git maintenance` run, and `git describe` then fails to read
# an object that is being packed.
git config gc.auto 0
git config maintenance.auto false
git add -A
git commit -q -m "chore: working tree under test"
base=$(git rev-parse HEAD)
seed="v$(sed -nE 's/^version = "(.*)"/\1/p' Cargo.toml | head -1)"
IFS=. read -r major minor patch <<<"${seed#v}"
if [ "$major" != "0" ]; then
    echo "test-release: cases assume a 0.x crate version; rewrite them for ${seed}" >&2
    exit 1
fi
next_patch="v${major}.${minor}.$((patch + 1))"
next_minor="v${major}.$((minor + 1)).0"

failures=0

reset() {
    git reset -q --hard "$base"
    git tag -l | xargs -r git tag -d >/dev/null
}

newest_tag() {
    git describe --tags --abbrev=0 --match 'v[0-9]*' 2>/dev/null || echo none
}

# expect_tag <want> <commit message>...
# Seeds the tag, adds one empty commit per message, releases, compares.
expect_tag() {
    local want="$1" got msg
    shift
    reset
    git tag "$seed"
    for msg in "$@"; do
        git commit -q --allow-empty -m "$msg"
    done
    if ! just release >"$work/out.log" 2>&1; then
        echo "FAIL [$*]: just release exited non-zero"
        sed 's/^/    /' "$work/out.log"
        failures=$((failures + 1))
        return
    fi
    got=$(newest_tag)
    if [ "$got" != "$want" ]; then
        echo "FAIL [$*]: want ${want}, got ${got}"
        failures=$((failures + 1))
    else
        echo "ok   [$*] -> ${got}"
    fi
}

# expect_refusal <label> <setup command>
# `just release` must exit non-zero and leave HEAD where it was.
expect_refusal() {
    local label="$1" head
    reset
    eval "$2"
    git commit -q --allow-empty -m "fix: bug"
    head=$(git rev-parse HEAD)
    if just release >"$work/out.log" 2>&1; then
        echo "FAIL [${label}]: just release succeeded"
        failures=$((failures + 1))
    elif [ "$(git rev-parse HEAD)" != "$head" ]; then
        echo "FAIL [${label}]: HEAD moved"
        failures=$((failures + 1))
    else
        echo "ok   [${label}] -> refused"
    fi
}

expect_tag "$next_patch" "fix: bug"
expect_tag "$next_patch" "perf: faster"
expect_tag "$next_minor" "feat: thing"
expect_tag "$seed" "docs: words" "ci: fix the gate" "chore(deps): bump"
# Breaking changes before 1.0.0 bump the minor version, whatever the type.
expect_tag "$next_minor" "feat!: thing"
expect_tag "$next_minor" "fix!: bug"
expect_tag "$next_minor" "refactor!: shape"
expect_tag "$next_minor" "fix(core)!: bug"
expect_tag "$next_minor" "fix: bug

BREAKING CHANGE: the output differs"
# A breaking commit on top of a long history. The log here is larger than
# a pipe buffer, which is what made a piped `grep -q` miss the match.
reset
git tag "$seed"
filler=$(printf 'x%.0s' {1..200})
for i in $(seq 1 400); do
    git commit -q --allow-empty -m "chore: ${filler} ${i}"
done
git commit -q --allow-empty -m "fix!: bug"
if ! just release >"$work/out.log" 2>&1; then
    echo "FAIL [fix! after 400 commits]: just release exited non-zero"
    tail -20 "$work/out.log" | sed 's/^/    /'
    failures=$((failures + 1))
elif [ "$(newest_tag)" != "$next_minor" ]; then
    echo "FAIL [fix! after 400 commits]: want ${next_minor}, got $(newest_tag)"
    failures=$((failures + 1))
else
    echo "ok   [fix! after 400 commits] -> ${next_minor}"
fi

# A stale or missing tag must not lower the workspace version.
expect_refusal "no tag" ":"
expect_refusal "tag behind Cargo.toml" "git tag v0.0.1"

# The bump commit carries the version files and the changelog, sets the
# package version and nothing else in Cargo.toml, and keeps the older
# changelog entries below the new one.
reset
git tag "$seed"
git commit -q --allow-empty -m "feat: thing"
just release >"$work/out.log" 2>&1
files=$(git show --name-only --format= HEAD | sort | tr '\n' ' ')
manifest_diff=$(git diff --unified=0 HEAD~1 HEAD -- Cargo.toml | grep -E '^[-+][^-+]' || true)
want_diff=$(printf -- '-version = "%s"\n+version = "%s"' "${seed#v}" "${next_minor#v}")
if [ "$files" != "CHANGELOG.md Cargo.lock Cargo.toml " ]; then
    echo "FAIL [bump commit files]: got '${files}'"
    failures=$((failures + 1))
elif [ "$manifest_diff" != "$want_diff" ]; then
    echo "FAIL [bump commit files]: Cargo.toml change was:"
    printf '%s\n' "$manifest_diff" | sed 's/^/    /'
    failures=$((failures + 1))
elif git cat-file -e HEAD~1:CHANGELOG.md 2>/dev/null \
    && ! diff -q <(git show HEAD~1:CHANGELOG.md | sed '1,/^- - -$/d') \
        <(tail -n "$(git show HEAD~1:CHANGELOG.md | sed '1,/^- - -$/d' | wc -l)" CHANGELOG.md) >/dev/null; then
    echo "FAIL [bump commit files]: older changelog entries changed"
    failures=$((failures + 1))
else
    echo "ok   [bump commit files]"
fi

# The release workflow runs the publish scripts by path, so they must be
# executable. A stub `gh` records the call and the notes.
reset
git tag "$seed"
git commit -q --allow-empty -m "feat!: thing"
just release >"$work/out.log" 2>&1
mkdir "$work/bin"
cat >"$work/bin/gh" <<'GH'
#!/usr/bin/env bash
# No release exists yet.
if [ "$1 $2" = "release view" ]; then exit 1; fi
printf '%s\n' "$*" >"$GH_STUB_LOG"
while [ "$#" -gt 0 ]; do
    if [ "$1" = "--notes-file" ]; then cat "$2" >>"$GH_STUB_LOG"; fi
    shift
done
GH
chmod +x "$work/bin/gh"
if ! GH_STUB_LOG="$work/gh.log" PATH="$work/bin:$PATH" \
    "$repo/scripts/publish-release.sh" "$next_minor" >"$work/out.log" 2>&1; then
    echo "FAIL [publish script]: did not run"
    sed 's/^/    /' "$work/out.log"
    failures=$((failures + 1))
elif ! head -1 "$work/gh.log" | grep -qF -- "release create ${next_minor} --verify-tag"; then
    echo "FAIL [publish script]: unexpected gh call: $(head -1 "$work/gh.log")"
    failures=$((failures + 1))
elif ! grep -qF -- '**BREAKING** thing' "$work/gh.log" || grep -qF '<span' "$work/gh.log"; then
    echo "FAIL [publish script]: breaking marker not rewritten"
    sed 's/^/    /' "$work/gh.log"
    failures=$((failures + 1))
else
    echo "ok   [publish script]"
fi

# A retry must not create a second release for the same tag.
cat >"$work/bin/gh" <<'GH'
#!/usr/bin/env bash
# The release already exists.
if [ "$1 $2" = "release view" ]; then exit 0; fi
printf '%s\n' "$*" >"$GH_STUB_LOG"
GH
rm -f "$work/gh.log"
if ! GH_STUB_LOG="$work/gh.log" PATH="$work/bin:$PATH" \
    "$repo/scripts/publish-release.sh" "$next_minor" >"$work/out.log" 2>&1; then
    echo "FAIL [publish script retry]: exited non-zero"
    sed 's/^/    /' "$work/out.log"
    failures=$((failures + 1))
elif [ -e "$work/gh.log" ]; then
    echo "FAIL [publish script retry]: called gh $(head -1 "$work/gh.log")"
    failures=$((failures + 1))
else
    echo "ok   [publish script retry]"
fi

if [ ! -x "$repo/scripts/publish-crate.sh" ]; then
    echo "FAIL [publish-crate script]: not executable"
    failures=$((failures + 1))
fi

if [ "$failures" -ne 0 ]; then
    echo "test-release: ${failures} failure(s)"
    exit 1
fi
echo "test-release: all cases pass"
