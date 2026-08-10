#!/usr/bin/env bash
set -euo pipefail

SPIKE_ROOT="$(builtin cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
builtin cd "$SPIKE_ROOT"
GENERATED="measurements/generated"

scripts/check-toolchain.sh
scripts/build.sh
scripts/inspect.sh
scripts/run-browser-matrix.sh
node scripts/assert-browser-results.mjs
node scripts/summarize-results.mjs > /dev/null
sha256sum pkg/turso_opfs_spike_bg.wasm > "$GENERATED/wasm.actual.sha256"
cp measurements/expected-toolchain.json "$GENERATED/toolchain.actual.json"

cp "$GENERATED/browser-matrix.actual.json" "$GENERATED/browser-matrix.json"
cp "$GENERATED/summary.actual.json" "$GENERATED/summary.json"
cp "$GENERATED/wasm-inspection.actual.json" "$GENERATED/wasm-inspection.json"
cp "$GENERATED/web-glue-inspection.actual.json" "$GENERATED/web-glue-inspection.json"
cp "$GENERATED/worker-source-inspection.actual.json" "$GENERATED/worker-source-inspection.json"
cp "$GENERATED/wasm.actual.sha256" "$GENERATED/wasm.sha256"
cp "$GENERATED/toolchain.actual.json" "$GENERATED/toolchain.json"

echo "updated committed WP-02 toolchain, hashes, inspections, browser results, and summary"
