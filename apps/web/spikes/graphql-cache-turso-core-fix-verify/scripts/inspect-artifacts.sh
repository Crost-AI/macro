#!/usr/bin/env bash
set -euo pipefail

SPIKE_ROOT="$(builtin cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
builtin cd "$SPIKE_ROOT"
mkdir -p target/evidence
ALLOWLIST=tools/inspect-wasm/expected-web-imports.tsv

inspect() {
  local output=$1
  shift
  cargo run --locked -q -p inspect-turso-temp-fix-wasm -- "$@" > "target/evidence/$output"
}

for variant in parent head; do
  if [[ "$variant" == parent ]]; then
    crate=turso_temp_fix_parent
    package=turso-temp-fix-parent
    revision_feature=parent-revision
  else
    crate=turso_temp_fix_head
    package=turso-temp-fix-head
    revision_feature=head-revision
  fi
  inspect "wasm-$variant-raw.json" --assert-browser-contract \
    "target/cargo-$variant/wasm32-unknown-unknown/release/$crate.wasm"
  inspect "wasm-$variant-node.json" --assert-browser-contract \
    "target/wasm/$variant/node/temp_fix_bg.wasm"
  inspect "wasm-$variant-web.json" --assert-browser-contract \
    --expected-imports "$ALLOWLIST" "target/wasm/$variant/web/temp_fix_bg.wasm"
  node scripts/inspect-glue.mjs web "target/wasm/$variant/web/temp_fix.js" \
    > "target/evidence/glue-$variant-web.json"
  node scripts/inspect-glue.mjs node "target/wasm/$variant/node/temp_fix.cjs" \
    > "target/evidence/glue-$variant-node.json"
  cargo tree --manifest-path "variants/$variant/Cargo.toml" --locked \
    --target wasm32-unknown-unknown --package "$package" \
    --no-default-features --features "wasm-minimum,failing-runtime-probes,$revision_feature" \
    | sed -e "s#$SPIKE_ROOT#<SPIKE_ROOT>#g" \
    > "target/evidence/cargo-tree-$variant-wasm.txt"
done

{
  printf 'artifact\tbytes\tsha256\n'
  for artifact in \
    target/cargo-parent/wasm32-unknown-unknown/release/turso_temp_fix_parent.wasm \
    target/wasm/parent/node/temp_fix_bg.wasm \
    target/wasm/parent/web/temp_fix_bg.wasm \
    target/wasm/parent/web/temp_fix.js \
    target/cargo-head/wasm32-unknown-unknown/release/turso_temp_fix_head.wasm \
    target/wasm/head/node/temp_fix_bg.wasm \
    target/wasm/head/web/temp_fix_bg.wasm \
    target/wasm/head/web/temp_fix.js; do
    printf '%s\t%s\t%s\n' \
      "$artifact" "$(wc -c < "$artifact")" "$(sha256sum "$artifact" | cut -d' ' -f1)"
  done
} > target/evidence/artifact-hashes.tsv

node scripts/inspect-artifact-paths.mjs \
  > target/evidence/artifact-path-inspection.json
scripts/check-toolchain.sh
cp measurements/expected-toolchain.json target/evidence/toolchain.json

jq -n \
  --slurpfile parent_raw target/evidence/wasm-parent-raw.json \
  --slurpfile head_raw target/evidence/wasm-head-raw.json \
  --slurpfile parent_node target/evidence/wasm-parent-node.json \
  --slurpfile head_node target/evidence/wasm-head-node.json \
  --slurpfile parent_web target/evidence/wasm-parent-web.json \
  --slurpfile head_web target/evidence/wasm-head-web.json \
  --slurpfile parent_web_glue target/evidence/glue-parent-web.json \
  --slurpfile head_web_glue target/evidence/glue-head-web.json \
  --slurpfile parent_node_glue target/evidence/glue-parent-node.json \
  --slurpfile head_node_glue target/evidence/glue-head-node.json \
  'def wasm_summary:
     {path, bytes, memories, atomic_operator_count, memory_grow_operator_count,
      clock_time_imports, random_crypto_imports, filesystem_imports, wasi_imports,
      thread_related_imports, worker_related_imports, unexpected_imports,
      missing_imports, duplicate_imports, imports_allowed, contract_violations,
      browser_contract_compliant};
   {parent: {raw: ($parent_raw[0] | wasm_summary), node: ($parent_node[0] | wasm_summary), web: ($parent_web[0] | wasm_summary), web_glue: $parent_web_glue[0], node_glue: $parent_node_glue[0]},
    head: {raw: ($head_raw[0] | wasm_summary), node: ($head_node[0] | wasm_summary), web: ($head_web[0] | wasm_summary), web_glue: $head_web_glue[0], node_glue: $head_node_glue[0]}}' \
  > target/evidence/structural-summary.json

printf 'inspected raw, node, web, glue, dependencies, exact imports, host paths, and tools for both revisions\n'
