#!/usr/bin/env bash
set -euo pipefail

SPIKE_ROOT="$(builtin cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
builtin cd "$SPIKE_ROOT"
scripts/check-toolchain.sh

# Cargo includes canonical manifest/package paths in crate disambiguators even
# when rustc source paths are remapped. Compile both variants from this fixed,
# clean staged layout so checkout location and repository-parent Cargo config
# cannot affect the resulting WASM.
BUILD_ROOT=/tmp/macro-turso-core-fix-wasm-build-v1
STAGED_SPIKE=$BUILD_ROOT/apps/web/spikes/graphql-cache-turso-core-fix-verify
BUILD_LOCK=${BUILD_ROOT}.lock
if ! mkdir "$BUILD_LOCK" 2>/dev/null; then
  echo "deterministic WASM build root is already locked: $BUILD_LOCK" >&2
  exit 1
fi
cleanup() {
  rm -rf "$BUILD_ROOT" "$BUILD_LOCK"
}
trap cleanup EXIT
rm -rf "$BUILD_ROOT"
mkdir -p "$STAGED_SPIKE"
tar -C "$SPIKE_ROOT" \
  --exclude='variants/parent/target' \
  --exclude='variants/head/target' \
  -cf - .cargo harness variants rust-toolchain.toml scripts/cargo-wasm.sh target/sources \
  | tar -xf - -C "$STAGED_SPIKE"
find "$BUILD_ROOT" -exec touch -h -d @315532800 {} +

for variant in parent head; do
  manifest="$STAGED_SPIKE/variants/$variant/Cargo.toml"
  staged_target="$STAGED_SPIKE/target/cargo-$variant"
  target="$SPIKE_ROOT/target/cargo-$variant"
  if [[ "$variant" == parent ]]; then
    package=turso-temp-fix-parent
    crate=turso_temp_fix_parent
    revision_feature=parent-revision
  else
    package=turso-temp-fix-head
    crate=turso_temp_fix_head
    revision_feature=head-revision
  fi
  rm -rf "$staged_target" "$target"
  CARGO_TARGET_DIR="$staged_target" "$STAGED_SPIKE/scripts/cargo-wasm.sh" build \
    --manifest-path "$manifest" --locked --release \
    --target wasm32-unknown-unknown --package "$package" --lib \
    --no-default-features \
    --features "wasm-minimum,failing-runtime-probes,$revision_feature"
  staged_raw="$staged_target/wasm32-unknown-unknown/release/$crate.wasm"
  raw="$target/wasm32-unknown-unknown/release/$crate.wasm"
  mkdir -p "$(dirname "$raw")"
  cp "$staged_raw" "$raw"

  node_dir="$SPIKE_ROOT/target/wasm/$variant/node"
  web_dir="$SPIKE_ROOT/target/wasm/$variant/web"
  rm -rf "$node_dir" "$web_dir"
  mkdir -p "$node_dir" "$web_dir"
  wasm-bindgen --target nodejs --out-name temp_fix --out-dir "$node_dir" "$raw"
  wasm-bindgen --target web --out-name temp_fix --out-dir "$web_dir" "$raw"
  cp "$node_dir/temp_fix.js" "$node_dir/temp_fix.cjs"
  printf '%s raw=%s node=%s web=%s\n' \
    "$variant" "$raw" "$node_dir/temp_fix_bg.wasm" "$web_dir/temp_fix_bg.wasm"
done
