#!/usr/bin/env bash
set -euo pipefail

SPIKE_ROOT="$(builtin cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
builtin cd "$SPIKE_ROOT"
WASM="pkg/turso_opfs_spike_bg.wasm"
ALLOWLIST="tools/inspect-wasm/expected-opfs-web-imports.tsv"
ACTUAL="measurements/generated"

cargo run --locked -q -p inspect-opfs-wasm -- \
  --assert-browser-contract --expected-imports "$ALLOWLIST" "$WASM" \
  > "$ACTUAL/wasm-inspection.actual.json"
node -e 'JSON.parse(require("fs").readFileSync(process.argv[1]))' \
  "$ACTUAL/wasm-inspection.actual.json"
node scripts/inspect-web-glue.cjs pkg/turso_opfs_spike.js \
  > "$ACTUAL/web-glue-inspection.actual.json"
node scripts/inspect-worker-source.cjs harness/worker.js harness/main.js \
  > "$ACTUAL/worker-source-inspection.actual.json"
node scripts/inspect-artifact-paths.mjs \
  > "$ACTUAL/artifact-path-inspection.actual.json"

if rg -n 'unsafe impl (Send|Sync)|#!\[allow\(unsafe_code\)\]' src; then
  echo "adapter contains an unsafe Send/Sync escape hatch" >&2
  exit 1
fi

echo "structural WASM/import/glue/source and absolute-artifact-path inspection passed"
