#!/usr/bin/env bash
# Shared helpers for Crost macro git hooks (Nix dev shell parity with CI).

ensure_nix_dev_env() {
  if [[ -n "${MACRO_GITHOOKS_NIX_ENV:-}" && -f "${MACRO_GITHOOKS_NIX_ENV}" ]]; then
    # shellcheck source=/dev/null
    source "${MACRO_GITHOOKS_NIX_ENV}"
    return 0
  fi

  if command -v nix >/dev/null 2>&1; then
    dev_env="${TMPDIR:-/tmp}/macro-nix-dev-env-$$"
    if ! nix print-dev-env --accept-flake-config >"$dev_env"; then
      echo "${GITHOOK_NAME:-git hook}: failed to enter Nix dev shell (nix print-dev-env)" >&2
      echo "Try: nix develop" >&2
      rm -f "$dev_env"
      return 1
    fi

    # shellcheck source=/dev/null
    source "$dev_env"
    export MACRO_GITHOOKS_NIX_ENV="$dev_env"

    if ! command -v cargo >/dev/null 2>&1 || ! command -v just >/dev/null 2>&1; then
      echo "${GITHOOK_NAME:-git hook}: Nix dev shell did not provide cargo and just" >&2
      echo "Try: nix develop" >&2
      return 1
    fi

    return 0
  fi

  missing=()
  for cmd in cargo just bun wasm-pack; do
    if ! command -v "$cmd" >/dev/null 2>&1; then
      missing+=("$cmd")
    fi
  done

  if ((${#missing[@]} > 0)); then
    echo "${GITHOOK_NAME:-git hook}: missing tools: ${missing[*]}" >&2
    echo "Install Nix (https://nix.dev/install-nix) then run: nix develop" >&2
    return 1
  fi

  return 0
}

ensure_web_prereqs() {
  if [[ ! -f bunfig.toml ]]; then
    {
      echo "[run]"
      echo "bun = true"
    } >bunfig.toml
  fi

  if ! command -v bun >/dev/null 2>&1; then
    echo "${GITHOOK_NAME:-git hook}: bun not found (required for just build-dev)" >&2
    echo "Enter the Nix dev shell: nix develop" >&2
    return 1
  fi

  bun install --frozen-lockfile
}
