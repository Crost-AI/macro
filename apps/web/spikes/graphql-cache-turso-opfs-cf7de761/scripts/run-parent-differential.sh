#!/usr/bin/env bash
set -euo pipefail

SPIKE_ROOT="$(builtin cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
builtin cd "$SPIKE_ROOT"
HEAD_REVISION="cf7de76172d61057007097e2dee7c47002cdc559"
PARENT_REVISION="79163249538197d01dec5ea7f65519454ed792e2"

scripts/check-toolchain.sh
if [[ ! -d pkg ]]; then
  echo "head package is missing; run scripts/build.sh first" >&2
  exit 1
fi
rm -rf pkg-head
mv pkg pkg-head
restore_head() {
  rm -rf pkg
  mv pkg-head pkg
  scripts/materialize-turso.sh "$HEAD_REVISION" >/dev/null
}
trap restore_head EXIT

scripts/materialize-turso.sh "$PARENT_REVISION"
CARGO_TARGET_DIR=target-parent scripts/cargo-wasm.sh build --locked --release \
  --target wasm32-unknown-unknown --no-default-features --features wasm-minimum
wasm-bindgen --target web --out-dir pkg \
  target-parent/wasm32-unknown-unknown/release/turso_opfs_spike.wasm

BROWSERS=chromium,firefox \
COLD_RUNS=1 \
WARM_RUNS_PER_COLD=0 \
TRANSACTION_EXPECTATION=parent-failure \
MATRIX_OUTPUT=parent-differential.actual.json \
  scripts/run-browser-matrix.sh
node scripts/assert-parent-differential.mjs

echo "parent differential reproduced BEGIN IMMEDIATE/EXCLUSIVE WASM failures"
