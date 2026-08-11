#!/usr/bin/env bash
set -euo pipefail

SPIKE_PATH=apps/web/spikes/graphql-cache-turso-core-fix-verify/
ROOT="$(jj root)"
builtin cd "$ROOT"

usage() {
  echo "usage: scripts/source-boundary.sh [--revision <revision>]" >&2
  exit 2
}

REVISION=@
while (($#)); do
  case "$1" in
    --revision)
      (($# >= 2)) || usage
      REVISION=$2
      shift 2
      ;;
    *) usage ;;
  esac
done

revision_id="$(jj log -r "$REVISION" --no-graph -T 'commit_id')"
[[ -n "$revision_id" ]] || {
  echo "source-boundary revision did not resolve: $REVISION" >&2
  exit 1
}
changed="$(jj diff -r "$REVISION" --name-only)"
[[ -n "$changed" ]] || {
  echo "source-boundary revision has no changes to inspect: $REVISION ($revision_id)" >&2
  exit 1
}
unexpected="$(grep -v "^$SPIKE_PATH" <<<"$changed" || true)"
if [[ -n "$unexpected" ]]; then
  echo "revision $revision_id changed paths outside $SPIKE_PATH" >&2
  printf '%s\n' "$unexpected" >&2
  exit 1
fi
if rg -n 'path\s*=\s*"/' "$SPIKE_PATH" --glob 'Cargo.toml' --glob 'Cargo.lock'; then
  echo "absolute committed Cargo path dependency found" >&2
  exit 1
fi
printf 'source boundary revision %s contains %s changed paths under %s and no absolute Cargo path dependency\n' \
  "$revision_id" "$(wc -l <<<"$changed")" "$SPIKE_PATH"
