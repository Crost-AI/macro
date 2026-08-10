#!/usr/bin/env bash

EXPECTED_WASM_BINDGEN_VERSION="0.2.121"
EXPECTED_WASM_OPT_VERSION="117"

resolve_wasm_tools() {
  WASM_BINDGEN_BIN="${WASM_BINDGEN_BIN:-$(command -v wasm-bindgen || true)}"
  if [[ -z "$WASM_BINDGEN_BIN" ]]; then
    echo "wasm-bindgen $EXPECTED_WASM_BINDGEN_VERSION is required" >&2
    return 1
  fi
  local bindgen_version
  bindgen_version="$($WASM_BINDGEN_BIN --version)"
  if [[ "$bindgen_version" != "wasm-bindgen $EXPECTED_WASM_BINDGEN_VERSION" ]]; then
    echo "expected wasm-bindgen $EXPECTED_WASM_BINDGEN_VERSION, got: $bindgen_version" >&2
    return 1
  fi

  local candidates=()
  if [[ -n "${WASM_OPT_BIN:-}" ]]; then
    candidates+=("$WASM_OPT_BIN")
  else
    local path_wasm_opt
    path_wasm_opt="$(command -v wasm-opt || true)"
    [[ -n "$path_wasm_opt" ]] && candidates+=("$path_wasm_opt")
    while IFS= read -r candidate; do
      candidates+=("$candidate")
    done < <(find "$HOME/.cache/.wasm-pack" -type f -name wasm-opt -perm -u+x 2>/dev/null | sort)
  fi

  WASM_OPT_BIN=""
  local candidate version
  for candidate in "${candidates[@]}"; do
    [[ -x "$candidate" ]] || continue
    version="$($candidate --version 2>&1 || true)"
    if [[ "$version" =~ ^wasm-opt\ version\ ${EXPECTED_WASM_OPT_VERSION}([[:space:]]|$) ]]; then
      WASM_OPT_BIN="$candidate"
      break
    fi
  done
  if [[ -z "$WASM_OPT_BIN" ]]; then
    echo "wasm-opt version $EXPECTED_WASM_OPT_VERSION is required; set WASM_OPT_BIN to that executable" >&2
    return 1
  fi

  export WASM_BINDGEN_BIN WASM_OPT_BIN
}
