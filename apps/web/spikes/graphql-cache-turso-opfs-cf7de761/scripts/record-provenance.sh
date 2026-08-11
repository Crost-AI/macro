#!/usr/bin/env bash
set -euo pipefail
export GIT_OPTIONAL_LOCKS=0

SPIKE_ROOT="$(builtin cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
builtin cd "$SPIKE_ROOT"
EXPECTED_HEAD="cf7de76172d61057007097e2dee7c47002cdc559"
EXPECTED_PARENT="79163249538197d01dec5ea7f65519454ed792e2"
OUTPUT="measurements/generated/provenance.actual.json"

scripts/materialize-turso.sh "$EXPECTED_HEAD" >/dev/null
head="$(git -C "$TURSO_FORK" rev-parse HEAD)"
parent="$(git -C "$TURSO_FORK" rev-parse HEAD^)"
tree="$(git -C "$TURSO_FORK" rev-parse HEAD^{tree})"
branch="$(git -C "$TURSO_FORK" branch --show-current)"
remote="$(git -C "$TURSO_FORK" remote get-url origin)"
status="$(git -C "$TURSO_FORK" status --porcelain=v1 --untracked-files=all)"
materialized_revision="$(<.turso-source/.spike-materialized-revision)"
materialized_tree="$(<.turso-source/.spike-materialized-tree)"

if [[ "$head" != "$EXPECTED_HEAD" || "$parent" != "$EXPECTED_PARENT" ]]; then
  echo "fork provenance changed during verification" >&2
  exit 1
fi
if [[ -n "$status" ]]; then
  echo "fork became dirty during verification" >&2
  exit 1
fi
if rg -n '(path|git)\s*=\s*"/' Cargo.toml tools/inspect-wasm/Cargo.toml; then
  echo "committed Cargo manifest contains an absolute dependency path" >&2
  exit 1
fi

jq -n \
  --arg revision "$head" \
  --arg parent "$parent" \
  --arg tree "$tree" \
  --arg branch "$branch" \
  --arg remote "$remote" \
  --arg materializedRevision "$materialized_revision" \
  --arg materializedTree "$materialized_tree" \
  '{
    tursoFork: {
      revision: $revision,
      parent: $parent,
      tree: $tree,
      branch: $branch,
      remote: $remote,
      worktreeCleanBeforeAndAfter: true
    },
    dependency: {
      kind: "materialized relative path",
      manifestPath: ".turso-source/core",
      committedAbsolutePath: false,
      materializedRevision: $materializedRevision,
      materializedTree: $materializedTree
    }
  }' > "$OUTPUT"

cat "$OUTPUT"
