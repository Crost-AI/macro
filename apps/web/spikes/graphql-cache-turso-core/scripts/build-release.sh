#!/usr/bin/env bash
set -euo pipefail

SPIKE_ROOT="$(builtin cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
builtin cd "$SPIKE_ROOT"
source scripts/wasm-tools.sh
resolve_wasm_tools

TARGET_DIR="${CARGO_TARGET_DIR:-$SPIKE_ROOT/target/release-spike}"
RAW_WASM="$TARGET_DIR/wasm32-unknown-unknown/release/turso_core_wasm_spike.wasm"
NODE_DIR="$TARGET_DIR/bindgen-node"
WEB_DIR="$TARGET_DIR/bindgen-web"
OPTIMIZED_NODE_DIR="$TARGET_DIR/bindgen-node-optimized"
OPTIMIZED_WEB_DIR="$TARGET_DIR/bindgen-web-optimized"

cargo build --locked --release --target wasm32-unknown-unknown \
  -p turso-core-wasm-spike --lib --no-default-features --features wasm-minimum
rm -rf "$NODE_DIR" "$WEB_DIR" "$OPTIMIZED_NODE_DIR" "$OPTIMIZED_WEB_DIR"
"$WASM_BINDGEN_BIN" --target nodejs --out-dir "$NODE_DIR" "$RAW_WASM"
"$WASM_BINDGEN_BIN" --target web --out-dir "$WEB_DIR" "$RAW_WASM"

# apps/web/package.json marks descendant .js files as ESM, while wasm-bindgen's
# nodejs target emits CommonJS. A .cjs copy lets Node load it without editing
# any package manifest.
cp "$NODE_DIR/turso_core_wasm_spike.js" "$NODE_DIR/turso_core_wasm_spike.cjs"

mkdir -p "$OPTIMIZED_NODE_DIR" "$OPTIMIZED_WEB_DIR"
cp "$NODE_DIR"/*.js "$NODE_DIR"/*.d.ts "$OPTIMIZED_NODE_DIR/"
cp "$NODE_DIR/turso_core_wasm_spike.js" "$OPTIMIZED_NODE_DIR/turso_core_wasm_spike.cjs"
"$WASM_OPT_BIN" -Oz "$NODE_DIR/turso_core_wasm_spike_bg.wasm" \
  -o "$OPTIMIZED_NODE_DIR/turso_core_wasm_spike_bg.wasm"

cp "$WEB_DIR"/*.js "$WEB_DIR"/*.d.ts "$OPTIMIZED_WEB_DIR/"
"$WASM_OPT_BIN" -Oz "$WEB_DIR/turso_core_wasm_spike_bg.wasm" \
  -o "$OPTIMIZED_WEB_DIR/turso_core_wasm_spike_bg.wasm"

printf 'raw_wasm=%s\n' "$RAW_WASM"
printf 'node_wasm=%s\n' "$NODE_DIR/turso_core_wasm_spike_bg.wasm"
printf 'optimized_node_wasm=%s\n' "$OPTIMIZED_NODE_DIR/turso_core_wasm_spike_bg.wasm"
printf 'web_wasm=%s\n' "$WEB_DIR/turso_core_wasm_spike_bg.wasm"
printf 'optimized_web_wasm=%s\n' "$OPTIMIZED_WEB_DIR/turso_core_wasm_spike_bg.wasm"
