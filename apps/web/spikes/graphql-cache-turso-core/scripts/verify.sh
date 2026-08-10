#!/usr/bin/env bash
set -euo pipefail

SPIKE_ROOT="$(builtin cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
builtin cd "$SPIKE_ROOT"

cargo fmt --all --check
cargo test --locked -p turso-core-wasm-spike
cargo test --locked -p inspect-turso-wasm
cargo run --locked -q -p turso-core-wasm-spike --example native
cargo check --locked --target wasm32-unknown-unknown -p turso-core-wasm-spike --lib \
  --no-default-features --features wasm-minimum

TARGET_DIR="$SPIKE_ROOT/target/release-spike"
CARGO_TARGET_DIR="$TARGET_DIR" scripts/build-release.sh
RAW_WASM="$TARGET_DIR/wasm32-unknown-unknown/release/turso_core_wasm_spike.wasm"
NODE_DIR="$TARGET_DIR/bindgen-node"
OPTIMIZED_NODE_DIR="$TARGET_DIR/bindgen-node-optimized"
WEB_DIR="$TARGET_DIR/bindgen-web"
OPTIMIZED_WEB_DIR="$TARGET_DIR/bindgen-web-optimized"
ALLOWLIST="tools/inspect-wasm/expected-web-imports.tsv"

inspect() {
  local output="$1"
  shift
  cargo run --locked -q -p inspect-turso-wasm -- "$@" > "$output"
}

inspect "$TARGET_DIR/raw-inspection.json" \
  --assert-browser-contract "$RAW_WASM"
inspect "$TARGET_DIR/node-inspection.json" \
  --assert-browser-contract "$NODE_DIR/turso_core_wasm_spike_bg.wasm"
inspect "$TARGET_DIR/optimized-node-inspection.json" \
  --assert-browser-contract "$OPTIMIZED_NODE_DIR/turso_core_wasm_spike_bg.wasm"
inspect "$TARGET_DIR/web-inspection.json" \
  --assert-browser-contract --expected-imports "$ALLOWLIST" \
  "$WEB_DIR/turso_core_wasm_spike_bg.wasm"
inspect "$TARGET_DIR/optimized-web-inspection.json" \
  --assert-browser-contract --expected-imports "$ALLOWLIST" \
  "$OPTIMIZED_WEB_DIR/turso_core_wasm_spike_bg.wasm"

node scripts/inspect-web-glue.cjs "$WEB_DIR/turso_core_wasm_spike.js" \
  > "$TARGET_DIR/web-glue-inspection.json"
node scripts/run-node.cjs "$OPTIMIZED_NODE_DIR" optimized-node
node scripts/run-web.mjs \
  "$OPTIMIZED_WEB_DIR/turso_core_wasm_spike.js" \
  "$OPTIMIZED_WEB_DIR/turso_core_wasm_spike_bg.wasm"
