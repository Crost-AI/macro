#!/usr/bin/env bash
set -euo pipefail

SPIKE_ROOT="$(builtin cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
builtin cd "$SPIKE_ROOT"
source scripts/wasm-tools.sh
resolve_wasm_tools

TARGET_DIR="target/measure-release"
MEASUREMENTS="measurements/generated"
rm -rf "$TARGET_DIR" "$MEASUREMENTS"
mkdir -p "$MEASUREMENTS"

elapsed_command() {
  local output="$1"
  shift
  local start end
  start="$(date +%s%N)"
  "$@"
  end="$(date +%s%N)"
  awk -v start="$start" -v end="$end" 'BEGIN { printf "%.3f\n", (end-start)/1000000000 }' > "$output"
}

elapsed_command "$MEASUREMENTS/release-clean-seconds.txt" \
  env CARGO_TARGET_DIR="$TARGET_DIR" cargo build --locked --release \
    --target wasm32-unknown-unknown -p turso-core-wasm-spike --lib \
    --no-default-features --features wasm-minimum
elapsed_command "$MEASUREMENTS/release-noop-seconds.txt" \
  env CARGO_TARGET_DIR="$TARGET_DIR" cargo build --locked --release \
    --target wasm32-unknown-unknown -p turso-core-wasm-spike --lib \
    --no-default-features --features wasm-minimum

CARGO_TARGET_DIR="$TARGET_DIR" scripts/build-release.sh > "$MEASUREMENTS/build-artifacts.txt"
RAW_WASM="$TARGET_DIR/wasm32-unknown-unknown/release/turso_core_wasm_spike.wasm"
NODE_WASM="$TARGET_DIR/bindgen-node/turso_core_wasm_spike_bg.wasm"
OPTIMIZED_NODE_WASM="$TARGET_DIR/bindgen-node-optimized/turso_core_wasm_spike_bg.wasm"
WEB_WASM="$TARGET_DIR/bindgen-web/turso_core_wasm_spike_bg.wasm"
OPTIMIZED_WEB_WASM="$TARGET_DIR/bindgen-web-optimized/turso_core_wasm_spike_bg.wasm"
ALLOWLIST="tools/inspect-wasm/expected-web-imports.tsv"

artifacts=(
  "cargo-release|$RAW_WASM"
  "wasm-bindgen-node|$NODE_WASM"
  "wasm-bindgen-node-wasm-opt-117-Oz|$OPTIMIZED_NODE_WASM"
  "wasm-bindgen-web|$WEB_WASM"
  "wasm-bindgen-web-wasm-opt-117-Oz|$OPTIMIZED_WEB_WASM"
)
printf 'artifact\traw_bytes\tgzip_9_bytes\tbrotli_11_bytes\n' > "$MEASUREMENTS/sizes.tsv"
printf 'artifact\tsha256\tbytes\n' > "$MEASUREMENTS/hashes.tsv"
for entry in "${artifacts[@]}"; do
  label="${entry%%|*}"
  artifact="${entry#*|}"
  printf '%s\t%s\t%s\t%s\n' \
    "$label" \
    "$(wc -c < "$artifact")" \
    "$(gzip -9 -c "$artifact" | wc -c)" \
    "$(brotli -q 11 -c "$artifact" | wc -c)" \
    >> "$MEASUREMENTS/sizes.tsv"
  printf '%s\t%s\t%s\n' \
    "$label" \
    "$(sha256sum "$artifact" | cut -d' ' -f1)" \
    "$(wc -c < "$artifact")" \
    >> "$MEASUREMENTS/hashes.tsv"
done

inspect() {
  local output="$1"
  shift
  cargo run --locked -q -p inspect-turso-wasm -- "$@" > "$MEASUREMENTS/$output"
}
inspect raw-inspection.json --assert-browser-contract "$RAW_WASM"
inspect node-inspection.json --assert-browser-contract "$NODE_WASM"
inspect optimized-node-inspection.json --assert-browser-contract "$OPTIMIZED_NODE_WASM"
inspect web-inspection.json --assert-browser-contract --expected-imports "$ALLOWLIST" "$WEB_WASM"
inspect optimized-web-inspection.json --assert-browser-contract --expected-imports "$ALLOWLIST" "$OPTIMIZED_WEB_WASM"
node scripts/inspect-web-glue.cjs "$TARGET_DIR/bindgen-web/turso_core_wasm_spike.js" \
  > "$MEASUREMENTS/web-glue-inspection.json"

for sample in 1 2 3 4 5; do
  node scripts/run-node.cjs "$TARGET_DIR/bindgen-node-optimized" optimized-node \
    > "$MEASUREMENTS/node-optimized-$sample.json"
done
node scripts/run-web.mjs \
  "$TARGET_DIR/bindgen-web-optimized/turso_core_wasm_spike.js" \
  "$OPTIMIZED_WEB_WASM" > "$MEASUREMENTS/web-optimized-run.json"

cargo tree --locked --target wasm32-unknown-unknown -p turso-core-wasm-spike -e features \
  --no-default-features --features wasm-minimum \
  | awk -v root="$SPIKE_ROOT" '{gsub(root, "$SPIKE_ROOT"); print}' \
  > "$MEASUREMENTS/cargo-tree-wasm.txt"

mapfile -t source_files < <(
  find . -type f ! -path './target/*' ! -path './measurements/*' ! -name README.md -print \
    | sed 's|^./||' \
    | sort
)
printf 'path\tsha256\tbytes\n' > "$MEASUREMENTS/source-hashes.tsv"
for source_file in "${source_files[@]}"; do
  printf '%s\t%s\t%s\n' \
    "$source_file" \
    "$(sha256sum "$source_file" | cut -d' ' -f1)" \
    "$(wc -c < "$source_file")" \
    >> "$MEASUREMENTS/source-hashes.tsv"
done

rustc_wrapper="none"
if [[ -n "${RUSTC_WRAPPER:-}" ]]; then
  rustc_wrapper="$(basename "$RUSTC_WRAPPER")"
fi
jq -n \
  --arg generated_at_utc "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
  --arg rustc "$(rustc --version)" \
  --arg rustc_verbose "$(rustc --version --verbose)" \
  --arg cargo "$(cargo --version)" \
  --arg wasm_bindgen "$($WASM_BINDGEN_BIN --version)" \
  --arg wasm_opt "$($WASM_OPT_BIN --version)" \
  --arg wasm_opt_flags "-Oz" \
  --arg node "$(node --version)" \
  --arg gzip "$(gzip --version | head -1)" \
  --arg brotli "$(brotli --version)" \
  --arg jq "$(jq --version)" \
  --arg sccache "$(sccache --version 2>/dev/null || printf unavailable)" \
  --arg rustc_wrapper "$rustc_wrapper" \
  --arg architecture "$(uname -m)" \
  --arg turso_revision "ed15b13f8e5f77d7ae24af321a63d7cd0fa53365" \
  --arg wasmparser "0.240.0" \
  --arg measurement_script_sha256 "$(sha256sum scripts/measure.sh | cut -d' ' -f1)" \
  '{schema_version: 1, $generated_at_utc, $rustc, $rustc_verbose, $cargo, $wasm_bindgen, $wasm_opt, $wasm_opt_flags, $node, $gzip, $brotli, $jq, $sccache, $rustc_wrapper, $architecture, $turso_revision, $wasmparser, $measurement_script_sha256}' \
  > "$MEASUREMENTS/tool-versions.json"

node scripts/summarize-measurements.cjs "$MEASUREMENTS" > "$MEASUREMENTS/summary.json"
cat "$MEASUREMENTS/summary.json"
