#!/usr/bin/env bash
set -euo pipefail

SPIKE_ROOT="$(builtin cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
builtin cd "$SPIKE_ROOT"
TARGET_DIR="$SPIKE_ROOT/target/blocker-probes"
LOG_DIR="$TARGET_DIR/logs"
mkdir -p "$LOG_DIR"

set +e
CARGO_TARGET_DIR="$TARGET_DIR/no-uuid" cargo check --locked \
  --target wasm32-unknown-unknown -p turso-core-wasm-spike \
  --no-default-features --features probe-no-uuid \
  > "$LOG_DIR/no-uuid.log" 2>&1
NO_UUID_STATUS=$?
set -e
if [[ $NO_UUID_STATUS -eq 0 ]] || ! grep -q 'unresolved module or unlinked crate `uuid`' "$LOG_DIR/no-uuid.log"; then
  echo "no-uuid probe did not reproduce the expected compile failure" >&2
  exit 1
fi

CARGO_TARGET_DIR="$TARGET_DIR/runtime" cargo build --locked \
  --target wasm32-unknown-unknown -p turso-core-wasm-spike --lib \
  --no-default-features --features wasm-minimum,failing-runtime-probes
WASM="$TARGET_DIR/runtime/wasm32-unknown-unknown/debug/turso_core_wasm_spike.wasm"
BINDGEN_DIR="$TARGET_DIR/runtime/bindgen-node"
rm -rf "$BINDGEN_DIR"
wasm-bindgen --target nodejs --out-dir "$BINDGEN_DIR" "$WASM"
cp "$BINDGEN_DIR/turso_core_wasm_spike.js" "$BINDGEN_DIR/turso_core_wasm_spike.cjs"

run_expected_panic() {
  local probe="$1"
  local expected_stack="$2"
  set +e
  node scripts/run-failure-probe.cjs "$BINDGEN_DIR" "$probe" \
    > "$LOG_DIR/$probe.log" 2>&1
  local status=$?
  set -e
  if [[ $status -eq 0 ]] || ! grep -q 'std::time::Instant::now' "$LOG_DIR/$probe.log" \
    || ! grep -q "$expected_stack" "$LOG_DIR/$probe.log"; then
    echo "$probe did not reproduce the expected WASM clock panic" >&2
    exit 1
  fi
}

run_expected_panic run_builtin_memory_io_probe 'MemoryIO as turso_core::io::clock::Clock'
run_expected_panic run_begin_immediate_probe 'Connection::create_temp_database'

echo "reproduced no-uuid compile failure and both WASM clock panics"
