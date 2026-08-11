#!/usr/bin/env bash
set -euo pipefail

SPIKE_ROOT="$(builtin cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
builtin cd "$SPIKE_ROOT"
EXPECTED="measurements/expected-toolchain.json"

scripts/check-toolchain.sh
export NODE_PATH="$(jq -r .playwright.nodePath "$EXPECTED")"
export CHROMIUM_PATH="$(jq -r '.browsers.chromium.executable' "$EXPECTED")"
export FIREFOX_PATH="$(jq -r '.browsers.firefox.executable' "$EXPECTED")"
export WEBKIT_PATH="$(jq -r '.browsers["webkit-wpe"].executable' "$EXPECTED")"

node scripts/browser-matrix.mjs
