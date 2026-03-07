#!/usr/bin/env bash
set -euo pipefail

if ! command -v nix-shell >/dev/null 2>&1; then
  echo "error: nix-shell is required but was not found in PATH" >&2
  exit 1
fi

echo "Installing Git hooks with the pinned dev shell..."
nix-shell --run 'pre-commit install --hook-type pre-commit --hook-type pre-push'

echo "Dev setup complete."
echo "Use 'nix-shell' for the pinned tool environment."
echo "Run 'just ci' for the canonical gate."
