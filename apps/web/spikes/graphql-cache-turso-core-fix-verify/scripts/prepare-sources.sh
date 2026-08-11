#!/usr/bin/env bash
set -euo pipefail

SPIKE_ROOT="$(builtin cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PARENT=79163249538197d01dec5ea7f65519454ed792e2
HEAD=cf7de76172d61057007097e2dee7c47002cdc559

usage() {
  echo "usage: scripts/prepare-sources.sh --source-repository <read-only-turso-worktree>" >&2
  exit 2
}

SOURCE_REPOSITORY=
while (($#)); do
  case "$1" in
    --source-repository)
      (($# >= 2)) || usage
      SOURCE_REPOSITORY=$2
      shift 2
      ;;
    *) usage ;;
  esac
done
[[ -n "$SOURCE_REPOSITORY" ]] || usage
SOURCE_REPOSITORY="$(builtin cd "$SOURCE_REPOSITORY" && pwd)"

[[ "$(git -C "$SOURCE_REPOSITORY" rev-parse HEAD)" == "$HEAD" ]] || {
  echo "source repository HEAD is not $HEAD" >&2
  exit 1
}
[[ "$(git -C "$SOURCE_REPOSITORY" rev-parse "$HEAD^")" == "$PARENT" ]] || {
  echo "$PARENT is not the first parent of $HEAD" >&2
  exit 1
}
[[ -z "$(git -C "$SOURCE_REPOSITORY" status --porcelain=v1 --untracked-files=all)" ]] || {
  echo "source repository must be clean; refusing to consume or modify it" >&2
  exit 1
}
git -C "$SOURCE_REPOSITORY" diff --check "$PARENT..$HEAD"

rm -rf "$SPIKE_ROOT/target/sources"
mkdir -p "$SPIKE_ROOT/target/sources/parent" "$SPIKE_ROOT/target/sources/head"
git -C "$SOURCE_REPOSITORY" archive --format=tar "$PARENT" | tar -xf - -C "$SPIKE_ROOT/target/sources/parent"
git -C "$SOURCE_REPOSITORY" archive --format=tar "$HEAD" | tar -xf - -C "$SPIKE_ROOT/target/sources/head"

[[ -z "$(git -C "$SOURCE_REPOSITORY" status --porcelain=v1 --untracked-files=all)" ]] || {
  echo "source repository changed while sources were prepared" >&2
  exit 1
}

mkdir -p "$SPIKE_ROOT/target/evidence"
changed_files="$(git -C "$SOURCE_REPOSITORY" diff --name-only "$PARENT..$HEAD")"
printf '%s\n' "$changed_files" > "$SPIKE_ROOT/target/evidence/changed-files.txt"
diff_sha256="$(git -C "$SOURCE_REPOSITORY" diff --full-index --binary --no-ext-diff "$PARENT..$HEAD" | sha256sum | cut -d' ' -f1)"
parent_tree="$(git -C "$SOURCE_REPOSITORY" rev-parse "$PARENT^{tree}")"
head_tree="$(git -C "$SOURCE_REPOSITORY" rev-parse "$HEAD^{tree}")"

jq -n \
  --arg parent "$PARENT" \
  --arg head "$HEAD" \
  --arg parent_tree "$parent_tree" \
  --arg head_tree "$head_tree" \
  --arg diff_sha256 "$diff_sha256" \
  --argjson changed_files "$(printf '%s\n' "$changed_files" | jq -R . | jq -s .)" \
  '{parent: $parent, head: $head, parent_tree: $parent_tree, head_tree: $head_tree, head_first_parent: $parent, diff_sha256: $diff_sha256, changed_files: $changed_files, source_repository_clean_before_and_after: true, extraction: "git archive of exact commit trees", fork_modified: false}' \
  > "$SPIKE_ROOT/target/evidence/source-provenance.json"

risk_output="$SPIKE_ROOT/target/evidence/source-risk.json"
printf '{\n' > "$risk_output"
first_category=true
for category_pattern in \
  'clock=Instant::now|SystemTime::now|DefaultClock' \
  'random=getrandom|rand::|random\(' \
  'filesystem=std::fs|File::open|OpenOptions' \
  'time=std::time|chrono::|SystemTime|Instant' \
  'threads=std::thread|thread_local!|Atomic' \
  'wasi=wasi' \
  'workers=Worker|worker_threads'; do
  category=${category_pattern%%=*}
  pattern=${category_pattern#*=}
  category_json="$SPIKE_ROOT/target/evidence/risk-$category.json"
  for variant_commit in "parent=$PARENT" "head=$HEAD"; do
    variant=${variant_commit%%=*}
    commit=${variant_commit#*=}
    matches="$SPIKE_ROOT/target/evidence/risk-$category-$variant.txt"
    git -C "$SOURCE_REPOSITORY" grep -n -I -E "$pattern" "$commit" -- core \
      | sed -E -e 's/^[0-9a-f]{40}://' -e 's#^([^:]+):[0-9]+:#\1:#' \
      > "$matches" || true
    count=$(wc -l < "$matches")
    hash=$(sha256sum "$matches" | cut -d' ' -f1)
    jq -n --argjson count "$count" --arg sha256 "$hash" '{count: $count, matching_lines_sha256: $sha256}' > "$SPIKE_ROOT/target/evidence/risk-$category-$variant.json"
  done
  jq -n \
    --slurpfile parent "$SPIKE_ROOT/target/evidence/risk-$category-parent.json" \
    --slurpfile head "$SPIKE_ROOT/target/evidence/risk-$category-head.json" \
    '{parent: $parent[0], head: $head[0]}' > "$category_json"
  $first_category || printf ',\n' >> "$risk_output"
  first_category=false
  printf '  %s: ' "$(jq -Rn --arg value "$category" '$value')" >> "$risk_output"
  jq -c . "$category_json" >> "$risk_output"
done
printf '}\n' >> "$risk_output"

printf 'prepared parent=%s head=%s without modifying source repository\n' "$PARENT" "$HEAD"
