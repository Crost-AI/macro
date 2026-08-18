#!/usr/bin/env bash
# Fixture test for upstream-sync automation (happy PR path + conflict issue path).
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

GH_LOG="$TMP/gh.log"
: >"$GH_LOG"

MOCK_BIN="$TMP/bin"
mkdir -p "$MOCK_BIN"
cat >"$MOCK_BIN/gh" <<'EOF'
#!/usr/bin/env bash
echo "$*" >> "${GH_LOG:?}"
exit 0
EOF
chmod +x "$MOCK_BIN/gh"
export PATH="$MOCK_BIN:$PATH"
export GH_LOG

setup_upstream_repo() {
  git init -q "$TMP/upstream"
  cd "$TMP/upstream"
  git config user.email "test@example.com"
  git config user.name "test"
  echo upstream-base > file.txt
  git add file.txt
  git commit -q -m "upstream base"
  git tag v1.0.0
  echo upstream-v2 >> file.txt
  git commit -q -am "upstream v2"
  git tag v2.0.0
}

setup_fork_repo() {
  local fork_dir="$1"
  git init -q "$fork_dir"
  cd "$fork_dir"
  git config user.email "test@example.com"
  git config user.name "test"
  git remote add upstream "$TMP/upstream"
  git fetch upstream --tags
  git checkout -q -b main v1.0.0
  echo crost-patch > crost.txt
  git add crost.txt
  git commit -q -m "crost patch"
  echo v1.0.0 > .upstream-base
  git add .upstream-base
  git commit -q -m "record upstream base"
  mkdir -p scripts
  cp "$REPO_ROOT/scripts/sync-upstream.sh" scripts/
  cp "$REPO_ROOT/scripts/upstream-sync-automation.sh" scripts/
  chmod +x scripts/sync-upstream.sh scripts/upstream-sync-automation.sh
}

setup_upstream_repo

echo "=== fixture: happy path (sync PR) ==="
setup_fork_repo "$TMP/fork-happy"
cd "$TMP/fork-happy"
happy_log="$TMP/happy.log"
UPSTREAM_SYNC_DRY_RUN=1 UPSTREAM_TAG=v2.0.0 ./scripts/upstream-sync-automation.sh | tee "$happy_log"
grep -q 'git range-diff' "$happy_log"
grep -q 'dry-run: would open PR' "$happy_log"
grep -q v2.0.0 .upstream-base

echo "=== fixture: conflict path (upstream-conflict issue) ==="
git init -q "$TMP/fork-conflict"
cd "$TMP/fork-conflict"
git config user.email "test@example.com"
git config user.name "test"
git remote add upstream "$TMP/upstream"
git fetch upstream --tags
git checkout -q -b main v1.0.0
echo crost-conflict > file.txt
git add file.txt
git commit -q -m "crost conflicting patch"
echo v1.0.0 > .upstream-base
git add .upstream-base
git commit -q -m "record upstream base"
mkdir -p scripts
cp "$REPO_ROOT/scripts/sync-upstream.sh" scripts/
cp "$REPO_ROOT/scripts/upstream-sync-automation.sh" scripts/
chmod +x scripts/sync-upstream.sh scripts/upstream-sync-automation.sh
: >"$GH_LOG"
UPSTREAM_SYNC_DRY_RUN=0 UPSTREAM_TAG=v2.0.0 ./scripts/upstream-sync-automation.sh
grep -q 'issue create' "$GH_LOG"
grep -q 'upstream-conflict' "$GH_LOG"

echo "upstream-sync-fixture-test: ok"
