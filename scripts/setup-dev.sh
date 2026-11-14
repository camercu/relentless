#!/usr/bin/env bash
set -euo pipefail

echo "==> Setting up development environment"

# Ensure we're inside Nix shell (fail fast if not)
if ! command -v nix-shell >/dev/null 2>&1; then
  echo "ERROR: nix-shell is not available. Install Nix first." >&2
  exit 1
fi

for tool in pre-commit cog; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "ERROR: $tool is not available. Run inside 'nix-shell'." >&2
    exit 1
  fi
done

echo "==> Installing pre-commit hooks"
pre-commit install --hook-type pre-commit --hook-type pre-push --hook-type commit-msg

echo "==> Verifying cocogitto"
cog --version

echo "==> Setup complete"
echo "Use 'nix-shell' for the pinned tool environment."
echo "Run 'just ci' for the canonical gate."
