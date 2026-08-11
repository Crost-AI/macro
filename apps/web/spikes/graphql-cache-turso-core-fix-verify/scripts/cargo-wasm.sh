#!/usr/bin/env bash
set -euo pipefail

SPIKE_ROOT="$(builtin cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
REPOSITORY_ROOT="$(builtin cd "$SPIKE_ROOT/../../../.." && pwd -P)"
GETRANDOM_BACKEND_CFG='getrandom_backend="wasm_js"'

# CARGO_ENCODED_RUSTFLAGS overrides merged parent Cargo rustflags. Repeat the
# spike-local backend cfg and remap every host/build root that can reach Rust
# metadata so the same source produces the same WASM at a different location.
flags=(
  --cfg "$GETRANDOM_BACKEND_CFG"
  # rustc uses the last matching remap, so order broad roots before their
  # nested Cargo, repository, and spike roots.
  "--remap-path-prefix=/nix/store=nix-store"
  "--remap-path-prefix=/rustc=rustc"
  "--remap-path-prefix=$HOME=host-home"
  "--remap-path-prefix=${CARGO_HOME:-$HOME/.cargo}=cargo-home"
  "--remap-path-prefix=$REPOSITORY_ROOT=repository-root"
  "--remap-path-prefix=$SPIKE_ROOT=spike-src"
)
if [[ "${1:-}" == "--print-config" ]]; then
  printf '%s\n' "${flags[@]}"
  exit 0
fi

printf -v CARGO_ENCODED_RUSTFLAGS '%s\x1f' "${flags[@]}"
export CARGO_ENCODED_RUSTFLAGS="${CARGO_ENCODED_RUSTFLAGS%$'\x1f'}"
export CARGO_INCREMENTAL=0
export SOURCE_DATE_EPOCH=315532800
# The repository direnv contributes a workspace-specific rpath and helper-bin
# entry. Neither is needed for this pure-Rust WASM target; remove them so an
# external copy does not inherit the original workspace as a hidden input.
if [[ -n "${NIX_LDFLAGS:-}" ]]; then
  NIX_LDFLAGS="$(sed -E 's#(^|[[:space:]])-rpath[[:space:]]+[^[:space:]]*/outputs/out/lib([[:space:]]|$)# #g' <<<"$NIX_LDFLAGS")"
  export NIX_LDFLAGS
fi
clean_path=
IFS=: read -r -a path_entries <<<"$PATH"
for entry in "${path_entries[@]}"; do
  [[ "$entry" == */.direnv/bin ]] && continue
  clean_path+="${clean_path:+:}$entry"
done
export PATH=$clean_path
unset RUSTFLAGS RUSTC_WRAPPER RUSTC_WORKSPACE_WRAPPER CARGO_BUILD_RUSTC_WRAPPER
unset DIRENV_DIR DIRENV_FILE out

exec cargo "$@"
