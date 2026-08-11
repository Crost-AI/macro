#!/usr/bin/env bash
set -euo pipefail

SPIKE_ROOT="$(builtin cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
builtin cd "$SPIKE_ROOT"

scripts/check-toolchain.sh
scripts/prepare-sources.sh "$@"
cargo run --quiet --manifest-path variants/parent/Cargo.toml --locked --example native \
  > target/evidence/native-parent.json
cargo run --quiet --manifest-path variants/head/Cargo.toml --locked --example native \
  > target/evidence/native-head.json
scripts/build-wasm.sh
node scripts/runtime-matrix.mjs
scripts/inspect-artifacts.sh
printf 'generated exact native, WASM runtime, structural, source, and toolchain evidence\n'
