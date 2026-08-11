#!/usr/bin/env bash
set -euo pipefail

SPIKE_ROOT="$(builtin cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
builtin cd "$SPIKE_ROOT"

usage() {
  echo "usage: scripts/verify.sh [--revision <revision>]" >&2
  exit 2
}

boundary_revision=${SOURCE_BOUNDARY_REVISION:-}
while (($#)); do
  case "$1" in
    --revision)
      (($# >= 2)) || usage
      boundary_revision=$2
      shift 2
      ;;
    *) usage ;;
  esac
done
if [[ -z "$boundary_revision" ]]; then
  boundary_revision='latest(ancestors(@) & ~empty())'
fi

scripts/check-toolchain.sh
scripts/materialize-turso.sh
cargo fmt --all --check
for script in harness/*.js scripts/*.mjs scripts/*.cjs; do
  node --check "$script"
done
cargo test --locked
cargo test --locked -p inspect-opfs-wasm
cargo clippy --locked --workspace --all-targets -- -D warnings
scripts/cargo-wasm.sh check --locked --target wasm32-unknown-unknown \
  --no-default-features --features wasm-minimum
scripts/cargo-wasm.sh clippy --locked --target wasm32-unknown-unknown \
  --no-default-features --features wasm-minimum -- -D warnings
scripts/build.sh
scripts/inspect.sh
node scripts/assert-runtime-routes.mjs
scripts/run-browser-matrix.sh
node scripts/assert-browser-results.mjs
scripts/run-parent-differential.sh
scripts/verify-standalone-copy.sh
scripts/record-provenance.sh > /dev/null
scripts/verify-measurements.sh
scripts/source-boundary.sh --revision "$boundary_revision"
