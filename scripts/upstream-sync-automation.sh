#!/usr/bin/env bash
# Detect a new upstream release tag, rebase via sync-upstream.sh, then open a PR
# (range-diff in body) or an upstream-conflict issue on rebase failure.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

UPSTREAM_REMOTE="${UPSTREAM_REMOTE:-upstream}"
UPSTREAM_URL="${UPSTREAM_URL:-https://github.com/macro-inc/macro.git}"
BASE_TAG_FILE="${BASE_TAG_FILE:-.upstream-base}"
GH="${GH:-gh}"
MAIN_BRANCH="${MAIN_BRANCH:-main}"
UPSTREAM_TAG="${UPSTREAM_TAG:-}"
DRY_RUN="${UPSTREAM_SYNC_DRY_RUN:-0}"

ensure_upstream_remote() {
  if ! git remote get-url "$UPSTREAM_REMOTE" &>/dev/null; then
    git remote add "$UPSTREAM_REMOTE" "$UPSTREAM_URL"
  fi
}

read_base_marker() {
  if [[ ! -f "$BASE_TAG_FILE" ]]; then
    echo "error: missing ${BASE_TAG_FILE}" >&2
    exit 1
  fi
  tr -d '[:space:]' < "$BASE_TAG_FILE"
}

resolve_to_commit() {
  local ref="$1"
  if git rev-parse -q --verify "${ref}^{commit}" >/dev/null; then
    git rev-parse "${ref}^{commit}"
    return 0
  fi
  if git rev-parse -q --verify "refs/tags/${ref}^{commit}" >/dev/null; then
    git rev-parse "refs/tags/${ref}^{commit}"
    return 0
  fi
  if git rev-parse -q --verify "${UPSTREAM_REMOTE}/${ref}^{commit}" >/dev/null; then
    git rev-parse "${UPSTREAM_REMOTE}/${ref}^{commit}"
    return 0
  fi
  return 1
}

list_upstream_tags_newest_first() {
  git ls-remote --tags "$UPSTREAM_REMOTE" 'v*' 2>/dev/null \
    | awk '{print $2}' \
    | sed 's|refs/tags/||' \
    | grep -v '\^{}$' \
    | sort -Vr
}

pick_newest_upstream_tag() {
  local current base_commit tag tag_commit candidate=""

  current="$(read_base_marker)"
  base_commit="$(resolve_to_commit "$current")" || {
    echo "error: cannot resolve current base ${current}" >&2
    exit 1
  }

  while IFS= read -r tag; do
    [[ -z "$tag" ]] && continue
    git fetch "$UPSTREAM_REMOTE" "refs/tags/${tag}:refs/tags/${tag}" --force >/dev/null 2>&1 || continue
    tag_commit="$(resolve_to_commit "$tag")" || continue

    if [[ "$tag_commit" == "$base_commit" ]]; then
      continue
    fi
    if git merge-base --is-ancestor "$tag_commit" "$base_commit" \
      && [[ "$tag_commit" != "$base_commit" ]]; then
      continue
    fi
    candidate="$tag"
    break
  done < <(list_upstream_tags_newest_first)

  if [[ -z "$candidate" ]]; then
    return 1
  fi
  echo "$candidate"
}

extract_range_diff() {
  local output_file="$1"
  awk '
    /^=== git range-diff / { capture=1; next }
    capture && /^Sync complete\./ { exit }
    capture { print }
  ' "$output_file"
}

classify_sync_failure() {
  local sync_log="$1"
  if grep -q 'error: rebase conflict' "$sync_log"; then
    echo "rebase-conflict"
  elif grep -q 'error: tag not found:' "$sync_log" \
    || grep -q "couldn't find remote ref refs/tags/" "$sync_log"; then
    echo "tag-not-found"
  elif grep -q 'Refusing to sync backwards' "$sync_log"; then
    echo "backwards-tag"
  elif grep -q 'disallowed workflow files present after sync' "$sync_log"; then
    echo "workflow-slimming"
  elif grep -q 'cannot determine old upstream base' "$sync_log"; then
    echo "missing-base"
  else
    echo "sync-failed"
  fi
}

ensure_conflict_label() {
  if [[ "$DRY_RUN" == "1" ]]; then
    return 0
  fi
  "$GH" label list --limit 200 --json name -q '.[].name' \
    | grep -qx 'upstream-conflict' \
    || "$GH" label create upstream-conflict \
      --color d73a4a \
      --description 'Upstream sync rebase conflict — resolve manually'
}

open_sync_failure_issue() {
  local tag="$1"
  local failure_kind="$2"
  local sync_log="$3"
  local title body_file gh_args=()

  case "$failure_kind" in
    rebase-conflict)
      title="Upstream sync conflict: ${tag}"
      gh_args=(--label upstream-conflict)
      ;;
    tag-not-found)
      title="Upstream sync failed (tag not found): ${tag}"
      ;;
    backwards-tag)
      title="Upstream sync failed (backwards tag): ${tag}"
      ;;
    workflow-slimming)
      title="Upstream sync failed (workflow slimming): ${tag}"
      ;;
    missing-base)
      title="Upstream sync failed (missing base marker): ${tag}"
      ;;
    *)
      title="Upstream sync failed: ${tag}"
      ;;
  esac

  body_file="$(mktemp)"
  {
    echo "Automated upstream sync to \`${tag}\` failed."
    echo ""
    echo "**Failure kind:** \`${failure_kind}\`"
    echo ""
    if [[ "$failure_kind" == "rebase-conflict" ]]; then
      cat <<'EOF'
## Next steps

1. Check out `main` and create a branch.
2. Run `./scripts/sync-upstream.sh <tag>` locally and resolve conflicts.
3. Complete the rebase, verify `git range-diff`, and open a PR manually.
4. Close this issue when the sync lands.

See `UPSTREAM.md` for the standing delta checklist.
EOF
    else
      echo "## sync-upstream.sh output"
      echo ""
      echo '```'
      cat "$sync_log"
      echo '```'
    fi
  } > "$body_file"

  if [[ "$DRY_RUN" == "1" ]]; then
    echo "dry-run: would open issue (${failure_kind}): ${title}"
    rm -f "$body_file"
    return 0
  fi

  if [[ "$failure_kind" == "rebase-conflict" ]]; then
    ensure_conflict_label
  fi
  "$GH" issue create \
    --title "$title" \
    "${gh_args[@]}" \
    --body-file "$body_file"
  rm -f "$body_file"
}

commit_upstream_base_marker() {
  local tag="$1"
  local head_marker worktree_marker

  worktree_marker="$(tr -d '[:space:]' < "$BASE_TAG_FILE")"
  if [[ "$worktree_marker" != "$tag" ]]; then
    echo "error: ${BASE_TAG_FILE} is ${worktree_marker:-<empty>}, expected ${tag}" >&2
    return 1
  fi

  head_marker="$(git show "HEAD:${BASE_TAG_FILE}" 2>/dev/null | tr -d '[:space:]' || true)"
  if [[ "$head_marker" == "$tag" ]] && [[ -z "$(git status --porcelain -- "$BASE_TAG_FILE")" ]]; then
    return 0
  fi

  git add "$BASE_TAG_FILE"
  git commit -m "chore: record upstream base ${tag}"
}

open_sync_pr() {
  local tag="$1"
  local range_diff="$2"
  local branch="$3"
  local body_file
  body_file="$(mktemp)"
  {
    echo "Automated upstream sync to \`${tag}\`."
    echo ""
    echo "## git range-diff"
    echo ""
    echo '```'
    if [[ -n "$range_diff" ]]; then
      printf '%s\n' "$range_diff"
    else
      echo "(no range-diff output captured)"
    fi
    echo '```'
    echo ""
    echo "## Review checklist"
    echo ""
    echo "- [ ] range-diff reviewed — no accidental upstream drift"
    echo "- [ ] Crost CI green"
    echo "- [ ] \`deploy/README.md\` smoke path still valid"
  } > "$body_file"

  if [[ "$DRY_RUN" == "1" ]] || ! git remote get-url origin &>/dev/null; then
    echo "dry-run: would open PR ${branch} -> ${MAIN_BRANCH}"
    cat "$body_file"
    rm -f "$body_file"
    return 0
  fi

  git push -u origin "$branch"
  "$GH" pr create \
    --base "$MAIN_BRANCH" \
    --head "$branch" \
    --title "Upstream sync: ${tag}" \
    --body-file "$body_file"
  rm -f "$body_file"
}

run_sync() {
  local tag="$1"
  local branch sync_log sync_status range_diff failure_kind

  branch="crost/upstream-sync-${tag//\//-}"

  git checkout "$MAIN_BRANCH"
  if [[ "$DRY_RUN" != "1" ]] && git remote get-url origin &>/dev/null; then
    git pull --ff-only origin "$MAIN_BRANCH"
  fi

  git checkout -B "$branch"

  sync_log="$(mktemp)"
  set +e
  ./scripts/sync-upstream.sh "$tag" >"$sync_log" 2>&1
  sync_status=$?
  set -e

  if [[ $sync_status -ne 0 ]]; then
    cat "$sync_log" >&2
    failure_kind="$(classify_sync_failure "$sync_log")"
    git checkout "$MAIN_BRANCH"
    git branch -D "$branch" >/dev/null 2>&1 || true
    open_sync_failure_issue "$tag" "$failure_kind" "$sync_log"
    rm -f "$sync_log"
    return 0
  fi

  if grep -Eq 'Already on upstream base|nothing to rebase' "$sync_log"; then
    cat "$sync_log"
    git checkout "$MAIN_BRANCH"
    git branch -D "$branch" >/dev/null 2>&1 || true
    rm -f "$sync_log"
    echo "No sync changes required."
    return 0
  fi

  range_diff="$(extract_range_diff "$sync_log")"
  cat "$sync_log"

  commit_upstream_base_marker "$tag"

  rm -f "$sync_log"

  if [[ -z "$(git log "${MAIN_BRANCH}..HEAD" --oneline)" ]]; then
    git checkout "$MAIN_BRANCH"
    git branch -D "$branch" >/dev/null 2>&1 || true
    echo "Sync produced no branch delta; skipping PR."
    return 0
  fi

  open_sync_pr "$tag" "$range_diff" "$branch"
}

main() {
  local tag="$UPSTREAM_TAG"

  ensure_upstream_remote

  if [[ -z "$tag" ]]; then
    if ! tag="$(pick_newest_upstream_tag)"; then
      echo "No new upstream release tag found; nothing to do."
      exit 0
    fi
  fi

  echo "Target upstream tag: ${tag}"
  run_sync "$tag"
}

main "$@"
