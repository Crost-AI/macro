#!/usr/bin/env bash
set -euo pipefail

SPIKE_ROOT="$(builtin cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GETRANDOM_BACKEND_CFG='getrandom_backend="wasm_js"'
REMAP_DESTINATION='spike-src'

# Environment rustflags override Cargo config, so repeat the pinned backend and
# add location-independent source paths for every scripted WASM compilation.
flags=(
  --cfg "$GETRANDOM_BACKEND_CFG"
  "--remap-path-prefix=$SPIKE_ROOT=$REMAP_DESTINATION"
  "--remap-path-prefix=${CARGO_HOME:-$HOME/.cargo}=cargo-home"
  "--remap-path-prefix=$HOME=host-home"
  "--remap-path-prefix=/nix/store=nix-store"
  "--remap-path-prefix=/rustc=rustc"
)
if [[ "${1:-}" == "--print-config" ]]; then
  printf '%s\n' "${flags[@]}"
  exit 0
fi

printf -v CARGO_ENCODED_RUSTFLAGS '%s\x1f' "${flags[@]}"
export CARGO_ENCODED_RUSTFLAGS="${CARGO_ENCODED_RUSTFLAGS%$'\x1f'}"
unset RUSTFLAGS

exec cargo "$@"
