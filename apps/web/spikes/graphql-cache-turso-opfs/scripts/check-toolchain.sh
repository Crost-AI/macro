#!/usr/bin/env bash
set -euo pipefail

SPIKE_ROOT="$(builtin cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
builtin cd "$SPIKE_ROOT"
EXPECTED="measurements/expected-toolchain.json"

check_exact() {
  local label="$1" expected="$2" actual="$3"
  if [[ "$actual" != "$expected" ]]; then
    printf '%s mismatch\nexpected: %s\nactual:   %s\n' "$label" "$expected" "$actual" >&2
    exit 1
  fi
}

check_exact rust "$(jq -r .rust "$EXPECTED")" "$(rustc --version)"
check_exact cargo "$(jq -r .cargo "$EXPECTED")" "$(cargo --version)"
check_exact wasm-bindgen-version "$(jq -r .wasmBindgen.version "$EXPECTED")" "$(wasm-bindgen --version)"
check_exact wasm-bindgen-derivation "$(jq -r .wasmBindgen.executable "$EXPECTED")" "$(command -v wasm-bindgen)"
check_exact node-version "$(jq -r .node.version "$EXPECTED")" "$(node --version)"
check_exact node-derivation "$(jq -r .node.executable "$EXPECTED")" "$(command -v node)"

PLAYWRIGHT_PATH="$(jq -r .playwright.nodePath "$EXPECTED")"
if [[ ! -f "$PLAYWRIGHT_PATH/playwright/package.json" ]]; then
  echo "pinned existing Playwright derivation is unavailable" >&2
  exit 1
fi
check_exact playwright-version "$(jq -r .playwright.version "$EXPECTED")" \
  "$(NODE_PATH="$PLAYWRIGHT_PATH" node -p "require('playwright/package.json').version")"
for browser in chromium firefox webkit-wpe; do
  executable="$(jq -r ".browsers[\"$browser\"].executable" "$EXPECTED")"
  if [[ ! -x "$executable" ]]; then
    echo "pinned $browser derivation is unavailable: $executable" >&2
    exit 1
  fi
done

echo "exact Rust/wasm-bindgen/Node/Playwright/browser derivations are present"
