#!/usr/bin/env bash
set -euo pipefail

SPIKE_ROOT="$(builtin cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
builtin cd "$SPIKE_ROOT"

scripts/check-toolchain.sh
cargo fmt --all --check
for script in harness/*.js scripts/*.mjs scripts/*.cjs; do
  node --check "$script"
done
cargo test --locked
cargo test --locked -p inspect-opfs-wasm
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo check --locked --target wasm32-unknown-unknown \
  --no-default-features --features wasm-minimum,failing-runtime-probes
cargo clippy --locked --target wasm32-unknown-unknown \
  --no-default-features --features wasm-minimum,failing-runtime-probes -- -D warnings
scripts/build.sh
scripts/inspect.sh
node scripts/reproduce-blockers.mjs
scripts/run-browser-matrix.sh
node scripts/assert-browser-results.mjs
scripts/verify-measurements.sh
