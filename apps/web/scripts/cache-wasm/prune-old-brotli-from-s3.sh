#!/usr/bin/env bash
# Prune only prior cache WASM objects after every publication step succeeds.
set -euo pipefail

if [ "$#" -ne 2 ]; then
  echo "usage: prune-old-brotli-from-s3.sh <dist-root> <s3-prefix>" >&2
  exit 2
fi

dist_root=$1
s3_prefix=${2%/}
raw_files=()
while IFS= read -r -d '' path; do
  raw_files+=("$path")
done < <(find "$dist_root" -type f -name 'cache_wasm_bg*.wasm' -print0)
if [ "${#raw_files[@]}" -ne 1 ]; then
  echo "expected one raw cache WASM; found ${#raw_files[@]}" >&2
  exit 1
fi
raw=${raw_files[0]}
relative_key=${raw#"${dist_root%/}"/}
if [ "$relative_key" = "$raw" ]; then
  echo "cache WASM is not contained by dist root" >&2
  exit 1
fi

# Cache WASM is excluded from generic sync/delete. Keep the current immutable
# key and remove only older hashed cache WASM objects.
aws s3 rm "$s3_prefix" --recursive \
  --exclude '*' \
  --include '*cache_wasm_bg*.wasm' \
  --exclude "$relative_key"
