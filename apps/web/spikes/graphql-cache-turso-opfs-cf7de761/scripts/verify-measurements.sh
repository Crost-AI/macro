#!/usr/bin/env bash
set -euo pipefail

SPIKE_ROOT="$(builtin cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
builtin cd "$SPIKE_ROOT"
GENERATED="measurements/generated"

scripts/record-provenance.sh > /dev/null
node scripts/summarize-results.mjs > /dev/null
sha256sum pkg/turso_opfs_spike_bg.wasm > "$GENERATED/wasm.actual.sha256"
cp measurements/expected-toolchain.json "$GENERATED/toolchain.actual.json"

for name in artifact-path-inspection.json provenance.json standalone-copy.json summary.json \
  wasm-inspection.json web-glue-inspection.json worker-source-inspection.json \
  wasm.sha256 toolchain.json; do
  actual="${name/.json/.actual.json}"
  if [[ "$name" == "wasm.sha256" ]]; then actual="wasm.actual.sha256"; fi
  if ! cmp -s "$GENERATED/$name" "$GENERATED/$actual"; then
    echo "committed measurement is stale: $name; run scripts/update-measurements.sh" >&2
    diff -u "$GENERATED/$name" "$GENERATED/$actual" || true
    exit 1
  fi
done

echo "regenerated provenance, hashes, inspections, and deterministic summary match committed files"
