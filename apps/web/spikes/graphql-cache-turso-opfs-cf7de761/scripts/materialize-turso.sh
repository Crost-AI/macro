#!/usr/bin/env bash
set -euo pipefail
export GIT_OPTIONAL_LOCKS=0

SPIKE_ROOT="$(builtin cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
EXPECTED_HEAD="cf7de76172d61057007097e2dee7c47002cdc559"
EXPECTED_PARENT="79163249538197d01dec5ea7f65519454ed792e2"
REVISION="${1:-$EXPECTED_HEAD}"
SOURCE_DIR="$SPIKE_ROOT/.turso-source"

if [[ -z "${TURSO_FORK:-}" ]]; then
  echo "TURSO_FORK must name the read-only Turso checkout at exact HEAD $EXPECTED_HEAD" >&2
  exit 1
fi
if [[ "$REVISION" != "$EXPECTED_HEAD" && "$REVISION" != "$EXPECTED_PARENT" ]]; then
  echo "refusing unreviewed Turso revision: $REVISION" >&2
  exit 1
fi

actual_head="$(git -C "$TURSO_FORK" rev-parse HEAD)"
if [[ "$actual_head" != "$EXPECTED_HEAD" ]]; then
  printf 'Turso fork HEAD mismatch\nexpected: %s\nactual:   %s\n' "$EXPECTED_HEAD" "$actual_head" >&2
  exit 1
fi
if [[ -n "$(git -C "$TURSO_FORK" status --porcelain=v1 --untracked-files=all)" ]]; then
  echo "Turso fork is dirty; refusing to materialize an ambiguous source tree" >&2
  exit 1
fi
actual_parent="$(git -C "$TURSO_FORK" rev-parse "$EXPECTED_HEAD^")"
if [[ "$actual_parent" != "$EXPECTED_PARENT" ]]; then
  printf 'Turso fork parent mismatch\nexpected: %s\nactual:   %s\n' "$EXPECTED_PARENT" "$actual_parent" >&2
  exit 1
fi

temporary="$SPIKE_ROOT/.turso-source.tmp.$$"
rm -rf "$temporary"
mkdir -p "$temporary"
trap 'rm -rf "$temporary"' EXIT
git -C "$TURSO_FORK" archive --format=tar "$REVISION" | tar -xf - -C "$temporary"
printf '%s\n' "$REVISION" > "$temporary/.spike-materialized-revision"
printf '%s\n' "$(git -C "$TURSO_FORK" rev-parse "$REVISION^{tree}")" > "$temporary/.spike-materialized-tree"
rm -rf "$SOURCE_DIR"
mv "$temporary" "$SOURCE_DIR"
trap - EXIT

printf 'materialized Turso %s (%s) into ignored relative source path %s\n' \
  "$REVISION" "$(<"$SOURCE_DIR/.spike-materialized-tree")" ".turso-source"
