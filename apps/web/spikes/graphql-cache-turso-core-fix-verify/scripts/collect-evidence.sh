#!/usr/bin/env bash
set -euo pipefail

SPIKE_ROOT="$(builtin cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DESTINATION=${1:-$SPIKE_ROOT/measurements/generated}
rm -rf "$DESTINATION"
mkdir -p "$DESTINATION"
for file in \
  source-provenance.json \
  source-risk.json \
  native-parent.json \
  native-head.json \
  wasm-runtime-matrix.json \
  structural-summary.json \
  artifact-hashes.tsv \
  artifact-path-inspection.json \
  toolchain.json \
  cargo-tree-parent-wasm.txt \
  cargo-tree-head-wasm.txt; do
  cp "$SPIKE_ROOT/target/evidence/$file" "$DESTINATION/$file"
done
