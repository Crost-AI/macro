#!/usr/bin/env bash
# Enter the Macro Nix dev shell for Buildkite (parity with .githooks/lib.sh and GHA setup-nix-dev-shell).
set -euo pipefail

ensure_nix_dev_env() {
  if [[ -n "${MACRO_BUILDKITE_NIX_ENV:-}" && -f "${MACRO_BUILDKITE_NIX_ENV}" ]]; then
    # shellcheck source=/dev/null
    source "${MACRO_BUILDKITE_NIX_ENV}"
    return 0
  fi

  if command -v nix >/dev/null 2>&1; then
    dev_env="${TMPDIR:-/tmp}/macro-buildkite-nix-dev-env-$$"
    if ! nix print-dev-env --accept-flake-config >"$dev_env"; then
      echo "buildkite: failed to enter Nix dev shell (nix print-dev-env)" >&2
      echo "Install Nix: https://nix.dev/install-nix then retry." >&2
      rm -f "$dev_env"
      return 1
    fi

    # shellcheck source=/dev/null
    source "$dev_env"
    export MACRO_BUILDKITE_NIX_ENV="$dev_env"

    if ! command -v cargo >/dev/null 2>&1 || ! command -v just >/dev/null 2>&1; then
      echo "buildkite: Nix dev shell did not provide cargo and just" >&2
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
    echo "buildkite: missing tools: ${missing[*]}" >&2
    echo "Install Nix (https://nix.dev/install-nix) or provide cargo, just, bun, wasm-pack on PATH." >&2
    return 1
  fi

  if command -v rustup >/dev/null 2>&1; then
    rustup_bin="$(dirname "$(rustup which rustc 2>/dev/null || true)")"
    if [[ -n "$rustup_bin" && -d "$rustup_bin" ]]; then
      export PATH="${rustup_bin}:${PATH}"
      if ! rustup target list --installed | grep -qx wasm32-unknown-unknown; then
        rustup target add wasm32-unknown-unknown
      fi
    fi
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
    echo "buildkite: bun not found (required for just build-dev)" >&2
    return 1
  fi

  bun install --frozen-lockfile
}

ensure_nix_dev_env
