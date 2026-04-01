#!/usr/bin/env bash
set -euo pipefail

echo "==> Setting up development environment"

# Ensure we're inside Nix shell (fail fast if not)
if ! command -v nix-shell >/dev/null 2>&1; then
  echo "ERROR: nix-shell is not available. Install Nix first." >&2
  exit 1
fi

if ! command -v node >/dev/null 2>&1; then
  echo "ERROR: node is not available. Run inside 'nix-shell'." >&2
  exit 1
fi

if ! command -v pre-commit >/dev/null 2>&1; then
  echo "ERROR: pre-commit is not available. Run inside 'nix-shell'." >&2
  exit 1
fi

echo "==> Installing Node dependencies"
if [ -f package-lock.json ]; then
  npm ci
else
  npm install
fi

echo "==> Installing pre-commit hooks"
pre-commit install --hook-type pre-commit --hook-type pre-push --hook-type commit-msg

echo "==> Verifying commitlint"
npx commitlint --version

echo "==> Verifying semantic-release"
npx semantic-release --version

echo "==> Setup complete"
echo "Use 'nix-shell' for the pinned tool environment."
echo "Run 'just ci' for the canonical gate."
