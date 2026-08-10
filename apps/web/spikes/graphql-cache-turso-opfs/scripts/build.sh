#!/usr/bin/env bash
set -euo pipefail

SPIKE_ROOT="$(builtin cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
builtin cd "$SPIKE_ROOT"

scripts/check-toolchain.sh
cargo build --locked --release --target wasm32-unknown-unknown \
  --no-default-features --features wasm-minimum,failing-runtime-probes
rm -rf pkg
wasm-bindgen --target web --out-dir pkg \
  target/wasm32-unknown-unknown/release/turso_opfs_spike.wasm
