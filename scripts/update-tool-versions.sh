#!/usr/bin/env bash
# Update .tool-versions and shell.nix to the latest available versions.
# Usage:
#   ./scripts/update-tool-versions.sh          # apply updates
#   ./scripts/update-tool-versions.sh --check   # dry-run; exit 1 if outdated
set -euo pipefail

CHECK_ONLY=false
if [[ "${1:-}" == "--check" ]]; then
    CHECK_ONLY=true
fi

TOOL_VERSIONS_FILE=".tool-versions"
SHELL_NIX_FILE="shell.nix"

# ── Name mappings ────────────────────────────────────────────
# .tool-versions name → nix attribute name
declare -A NIX_ATTR=(
    [just]="just"
    [cargo-deny]="cargo-deny"
    [cargo-nextest]="cargo-nextest"
    [cargo-semver-checks]="cargo-semver-checks"
    [cargo-mutants]="cargo-mutants"
    [typos-cli]="typos"
    [taplo-cli]="taplo"
)

# ── Resolve latest nixpkgs-unstable commit ───────────────────
echo "Resolving latest nixpkgs-unstable commit..."
NIXPKGS_REV=$(curl -sfL \
    -H "Accept: application/vnd.github.v3+json" \
    "https://api.github.com/repos/NixOS/nixpkgs/commits?sha=nixpkgs-unstable&per_page=1" \
    | python3 -c "import sys,json; print(json.load(sys.stdin)[0]['sha'])")
NIXPKGS_URL="https://github.com/NixOS/nixpkgs/archive/${NIXPKGS_REV}.tar.gz"
echo "  nixpkgs-unstable: ${NIXPKGS_REV:0:12}"

# ── Prefetch nixpkgs tarball ─────────────────────────────────
echo "Prefetching nixpkgs tarball (this may take a moment)..."
NIXPKGS_SHA256=$(nix-prefetch-url --unpack --type sha256 "$NIXPKGS_URL" 2>/dev/null)
echo "  sha256: $NIXPKGS_SHA256"

# ── Query nix package versions ───────────────────────────────
echo "Querying package versions from nixpkgs..."
declare -A LATEST

# Build a single nix expression that returns all versions at once.
nix_expr="let pkgs = import (builtins.fetchTarball \"$NIXPKGS_URL\") {}; in builtins.toJSON {"
for tool_name in "${!NIX_ATTR[@]}"; do
    nix_attr="${NIX_ATTR[$tool_name]}"
    # Nix attr set keys with hyphens need quoting
    nix_expr+=" \"${tool_name}\" = pkgs.${nix_attr}.version;"
done
nix_expr+=" }"

versions_json=$(nix-instantiate --eval -E "$nix_expr" 2>/dev/null | python3 -c "
import sys, json
# nix-instantiate --eval wraps the JSON string in quotes and escapes inner quotes
raw = sys.stdin.read().strip()
# Strip outer quotes and unescape
inner = raw.strip('\"').replace('\\\\\"', '\"')
data = json.loads(inner)
for k, v in sorted(data.items()):
    print(f'{k}={v}')
")

while IFS='=' read -r tool_name version; do
    LATEST[$tool_name]="$version"
done <<< "$versions_json"

# ── Query latest stable Rust version ────────────────────────
echo "Querying latest stable Rust version..."
RUST_VERSION=$(curl -sfL "https://static.rust-lang.org/dist/channel-rust-stable.toml" \
    | grep -A1 '^\[pkg\.rust\]' | grep '^version' | sed 's/version = "\([^ ]*\).*/\1/')
LATEST[rust]="$RUST_VERSION"

# ── Compare versions ────────────────────────────────────────
echo ""
outdated=0

while read -r name version; do
    latest="${LATEST[$name]:-}"
    if [[ -z "$latest" ]]; then
        printf "  %-22s %s (skipped — no upstream version found)\n" "$name" "$version"
        continue
    fi
    if [[ "$version" != "$latest" ]]; then
        printf "  %-22s %s → %s\n" "$name" "$version" "$latest"
        outdated=1
    fi
done < <(grep -v '^#' "$TOOL_VERSIONS_FILE" | grep -v '^$')

if [[ "$outdated" -eq 0 ]]; then
    echo "All tool versions are up to date."
    exit 0
fi

if [[ "$CHECK_ONLY" == true ]]; then
    echo ""
    echo "Tool versions are outdated (run 'just tool-versions-update' to update)."
    exit 1
fi

# ── Update .tool-versions ───────────────────────────────────
echo ""
echo "Updating $TOOL_VERSIONS_FILE..."
{
    # Preserve comment lines
    grep '^#' "$TOOL_VERSIONS_FILE" || true
    while read -r name version; do
        latest="${LATEST[$name]:-$version}"
        echo "$name $latest"
    done < <(grep -v '^#' "$TOOL_VERSIONS_FILE" | grep -v '^$')
} > "${TOOL_VERSIONS_FILE}.tmp"
mv "${TOOL_VERSIONS_FILE}.tmp" "$TOOL_VERSIONS_FILE"

# ── Update shell.nix ────────────────────────────────────────
echo "Updating $SHELL_NIX_FILE..."
# Replace the url line
sed -i.bak \
    "s|url = \"https://github.com/NixOS/nixpkgs/archive/.*\.tar\.gz\";|url = \"${NIXPKGS_URL}\";|" \
    "$SHELL_NIX_FILE"
# Replace the sha256 line
sed -i.bak \
    "s|sha256 = \".*\";|sha256 = \"${NIXPKGS_SHA256}\";|" \
    "$SHELL_NIX_FILE"
rm -f "${SHELL_NIX_FILE}.bak"

echo "Done. Review changes with 'git diff' before committing."
