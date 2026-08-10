#!/usr/bin/env bash
set -euo pipefail

SPIKE='apps/web/spikes/graphql-cache-turso-opfs/'
changed="$(jj diff --name-only)"
if [[ -n "$changed" ]] && grep -v "^${SPIKE}" <<<"$changed"; then
  echo "WP-02 changed a path outside its self-contained spike" >&2
  exit 1
fi
printf '%s\n' "$changed"
