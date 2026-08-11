#!/usr/bin/env bash
set -euo pipefail

SPIKE_ROOT="$(builtin cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
builtin cd "$SPIKE_ROOT"
EXPECTED=measurements/expected-toolchain.json

check_exact() {
  local label=$1 expected=$2 actual=$3
  if [[ "$actual" != "$expected" ]]; then
    printf '%s mismatch\nexpected: %s\nactual:   %s\n' "$label" "$expected" "$actual" >&2
    exit 1
  fi
}

resolved_executable() {
  readlink -f "$(command -v "$1")"
}

check_tool() {
  local key=$1 command=$2 version=$3
  check_exact "$key version" "$(jq -r "$key.version" "$EXPECTED")" "$version"
  check_exact "$key executable" "$(jq -r "$key.executable" "$EXPECTED")" \
    "$(resolved_executable "$command")"
}

check_tool .rustc rustc "$(rustc --version)"
check_tool .cargo cargo "$(cargo --version)"
check_tool .wasmBindgen wasm-bindgen "$(wasm-bindgen --version)"
check_tool .node node "$(node --version)"
check_tool .jq jq "$(jq --version)"
check_tool .verificationTools.git git "$(git --version)"
check_tool .verificationTools.jj jj "$(jj --version)"
check_tool .verificationTools.tar tar "$(tar --version | head -1)"
check_tool .verificationTools.ripgrep rg "$(rg --version | head -1)"
check_tool .verificationTools.sha256sum sha256sum "$(sha256sum --version | head -1)"

[[ -d "$(rustc --print target-libdir --target wasm32-unknown-unknown)" ]] || {
  echo "wasm32-unknown-unknown target is unavailable" >&2
  exit 1
}
expected_cargo_config=$'# Keep direct standalone WASM builds independent of repository-parent Cargo config.\n[target.wasm32-unknown-unknown]\nrustflags = ["--cfg", '\''getrandom_backend="wasm_js"'\'']'
check_exact spike-cargo-config "$expected_cargo_config" "$(<.cargo/config.toml)"

scripted_flags="$(scripts/cargo-wasm.sh --print-config)"
[[ "$(head -n 2 <<<"$scripted_flags" | tail -n 1)" == 'getrandom_backend="wasm_js"' ]] || {
  echo "scripted getrandom backend cfg is not pinned" >&2
  exit 1
}
while IFS= read -r destination; do
  grep -E -- "--remap-path-prefix=.*=${destination}$" <<<"$scripted_flags" > /dev/null || {
    echo "scripted remap destination is not pinned: $destination" >&2
    exit 1
  }
done < <(jq -r '.wasmBuild.scriptedRemapPathPrefixDestinations[]' "$EXPECTED")
jq -e '
  .wasmBuild.getrandomBackendCfg == "getrandom_backend=\"wasm_js\"" and
  .wasmBuild.spikeCargoConfigRustflags == ["--cfg", "getrandom_backend=\"wasm_js\""] and
  .wasmBuild.cargoIncremental == "0" and
  .wasmBuild.sourceDateEpoch == "315532800" and
  .wasmBuild.rustcWrappersDisabled == true and
  .wasmBuild.hostBuildEnvironmentSanitized == true and
  .wasmBuild.cleanWasmTargetBeforeBuild == true and
  .wasmBuild.fixedStagedBuildRoot == "/tmp/macro-turso-core-fix-wasm-build-v1" and
  .wasmBuild.repositoryParentCargoConfigExcludedFromWasmBuild == true and
  .wasmBuild.artifactAbsolutePathScan == true and
  .wasmBuild.standaloneCopyVerification == true' "$EXPECTED" > /dev/null
grep -Fx 'BUILD_ROOT=/tmp/macro-turso-core-fix-wasm-build-v1' scripts/build-wasm.sh > /dev/null || {
  echo "fixed staged WASM build root is not pinned" >&2
  exit 1
}

printf 'exact tools, wasm target, spike-local cfg, and deterministic WASM wrapper are present\n'
