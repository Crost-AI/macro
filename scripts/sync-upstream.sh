#!/usr/bin/env bash
# Rebase the Crost patch series onto an upstream release tag.
# Prints git range-diff on success; aborts cleanly on conflict.
set -euo pipefail

TAG="${1:-}"
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

UPSTREAM_REMOTE="${UPSTREAM_REMOTE:-upstream}"
UPSTREAM_URL="${UPSTREAM_URL:-https://github.com/macro-inc/macro.git}"
BASE_TAG_FILE="${BASE_TAG_FILE:-.upstream-base}"

usage() {
  echo "Usage: $0 <upstream-tag>" >&2
  echo "Example: $0 v2026.8.14.0" >&2
  echo "" >&2
  echo "Recent upstream tags:" >&2
  if git remote get-url "$UPSTREAM_REMOTE" &>/dev/null; then
    git ls-remote --tags "$UPSTREAM_REMOTE" 'v*' 2>/dev/null | awk '{print $2}' | sed 's|refs/tags/||' | sort -V | tail -5 >&2 || true
  else
    git ls-remote --tags "$UPSTREAM_URL" 'v*' 2>/dev/null | awk '{print $2}' | sed 's|refs/tags/||' | sort -V | tail -5 >&2 || true
  fi
  exit 1
}

[[ -n "$TAG" ]] || usage

if ! git remote get-url "$UPSTREAM_REMOTE" &>/dev/null; then
  git remote add "$UPSTREAM_REMOTE" "$UPSTREAM_URL"
fi

# Fetch only the requested tag (avoid pulling every upstream branch).
git fetch "$UPSTREAM_REMOTE" "refs/tags/${TAG}:refs/tags/${TAG}" --force

resolve_ref() {
  local ref="$1"
  if git rev-parse -q --verify "${ref}^{commit}" >/dev/null; then
    echo "$ref"
    return 0
  fi
  if git rev-parse -q --verify "${UPSTREAM_REMOTE}/${ref}^{commit}" >/dev/null; then
    echo "${UPSTREAM_REMOTE}/${ref}"
    return 0
  fi
  if git rev-parse -q --verify "refs/tags/${ref}^{commit}" >/dev/null; then
    echo "refs/tags/${ref}"
    return 0
  fi
  return 1
}

NEW_BASE_REF="$(resolve_ref "$TAG")" || {
  echo "error: tag not found: $TAG" >&2
  exit 1
}
NEW_BASE_COMMIT="$(git rev-parse "${NEW_BASE_REF}^{commit}")"

OLD_BASE_REF=""
if [[ -f "$BASE_TAG_FILE" ]]; then
  OLD_BASE_REF="$(tr -d '[:space:]' < "$BASE_TAG_FILE")"
fi
if [[ -z "$OLD_BASE_REF" ]]; then
  if git merge-base --is-ancestor "$NEW_BASE_COMMIT" HEAD 2>/dev/null; then
    OLD_BASE_REF="$NEW_BASE_COMMIT"
  elif git rev-parse -q --verify "${UPSTREAM_REMOTE}/main^{commit}" >/dev/null; then
    OLD_BASE_REF="$(git merge-base HEAD "${UPSTREAM_REMOTE}/main")"
  else
    echo "error: cannot determine old upstream base; create $BASE_TAG_FILE" >&2
    exit 1
  fi
fi
OLD_BASE_COMMIT="$(git rev-parse "${OLD_BASE_REF}^{commit}")"

if git merge-base --is-ancestor "$NEW_BASE_COMMIT" "$OLD_BASE_COMMIT" \
  && [[ "$NEW_BASE_COMMIT" != "$OLD_BASE_COMMIT" ]]; then
  echo "error: $TAG ($NEW_BASE_COMMIT) is older than current base $OLD_BASE_REF ($OLD_BASE_COMMIT)" >&2
  echo "Refusing to sync backwards; pick a newer upstream tag." >&2
  exit 1
fi

if [[ "$OLD_BASE_COMMIT" == "$NEW_BASE_COMMIT" ]]; then
  echo "Already on upstream base $TAG ($NEW_BASE_COMMIT); nothing to rebase."
  exit 0
fi

PATCH_COUNT="$(git rev-list --count "${OLD_BASE_COMMIT}..HEAD" || true)"
if [[ "${PATCH_COUNT:-0}" -eq 0 ]]; then
  echo "No patch commits above $OLD_BASE_REF; fast-forwarding base marker to $TAG."
  echo "$TAG" > "$BASE_TAG_FILE"
  exit 0
fi

OLD_HEAD="$(git rev-parse HEAD)"

echo "Rebasing $PATCH_COUNT patch commit(s) onto $TAG"
echo "  old base: $OLD_BASE_REF ($OLD_BASE_COMMIT)"
echo "  new base: $NEW_BASE_REF ($NEW_BASE_COMMIT)"
echo "  old HEAD: $OLD_HEAD"

if ! git rebase --onto "$NEW_BASE_COMMIT" "$OLD_BASE_COMMIT"; then
  echo "" >&2
  echo "error: rebase conflict — aborting and restoring pre-sync state" >&2
  git rebase --abort
  exit 1
fi

echo ""
echo "=== git range-diff ${OLD_BASE_COMMIT}..${OLD_HEAD} ${NEW_BASE_COMMIT}..HEAD ==="
git range-diff "${OLD_BASE_COMMIT}..${OLD_HEAD}" "${NEW_BASE_COMMIT}..HEAD" || true

echo "$TAG" > "$BASE_TAG_FILE"
echo ""
echo "Sync complete. Updated ${BASE_TAG_FILE} -> ${TAG}"
