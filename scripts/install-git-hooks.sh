#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

current="$(git config --get core.hooksPath || true)"
if [[ "$current" == ".githooks" ]]; then
  echo "git hooks already installed (core.hooksPath=.githooks)"
  exit 0
fi

git config core.hooksPath .githooks
echo "git hooks installed (core.hooksPath=.githooks)"
