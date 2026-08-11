#!/usr/bin/env bash
set -euo pipefail

SPIKE_ROOT="$(builtin cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
builtin cd "$SPIKE_ROOT"

usage() {
  echo "usage: scripts/verify.sh --source-repository <read-only-turso-worktree> [--revision <revision>]" >&2
  exit 2
}

source_repository=
boundary_revision=${SOURCE_BOUNDARY_REVISION:-}
while (($#)); do
  case "$1" in
    --source-repository)
      (($# >= 2)) || usage
      source_repository=$2
      shift 2
      ;;
    --revision)
      (($# >= 2)) || usage
      boundary_revision=$2
      shift 2
      ;;
    *) usage ;;
  esac
done
[[ -n "$source_repository" ]] || usage
if [[ -z "$boundary_revision" ]]; then
  boundary_revision='latest(ancestors(@) & ~empty())'
fi
SOURCE_ARGS=(--source-repository "$source_repository")

scripts/check-toolchain.sh
for script in scripts/*.sh; do
  bash -n "$script"
done
for script in scripts/*.mjs; do
  node --check "$script"
done
scripts/prepare-sources.sh "${SOURCE_ARGS[@]}"
cargo fmt --manifest-path Cargo.toml --all --check
cargo fmt --manifest-path variants/parent/Cargo.toml --all --check
cargo fmt --manifest-path variants/head/Cargo.toml --all --check
cargo test --locked -p inspect-turso-temp-fix-wasm
cargo test --manifest-path variants/parent/Cargo.toml --locked
cargo test --manifest-path variants/head/Cargo.toml --locked

scripts/generate-evidence.sh "${SOURCE_ARGS[@]}"
scripts/collect-evidence.sh target/verified-evidence
diff -ru measurements/generated target/verified-evidence
scripts/source-boundary.sh --revision "$boundary_revision"
if [[ "${CORE_FIX_VERIFY_STANDALONE_CHILD:-0}" != 1 ]]; then
  scripts/verify-standalone-copy.sh "${SOURCE_ARGS[@]}"
  diff -u measurements/standalone-copy.json target/evidence/standalone-copy.json
fi

[[ -n "$source_repository" ]]
[[ "$(git -C "$source_repository" rev-parse HEAD)" == "cf7de76172d61057007097e2dee7c47002cdc559" ]]
[[ -z "$(git -C "$source_repository" status --porcelain=v1 --untracked-files=all)" ]]

printf 'verified exact parent/fixed native and WASM evidence; source fork remains unchanged\n'
