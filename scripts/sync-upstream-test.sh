#!/usr/bin/env bash
# Minimal regression test for scripts/sync-upstream.sh (rebase, range-diff, abort).
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

git init -q "$TMP/upstream"
git init -q "$TMP/fork"
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

cd "$TMP/fork"
git config user.email "test@example.com"
git config user.name "test"
git remote add upstream "$TMP/upstream"
git fetch upstream --tags
git checkout -q v1.0.0
echo crost-patch > crost.txt
git add crost.txt
git commit -q -m "crost patch"
echo v1.0.0 > .upstream-base

mkdir -p scripts
cp "$REPO_ROOT/scripts/sync-upstream.sh" scripts/
chmod +x scripts/sync-upstream.sh

# Happy path: rebase patch onto v2.0.0
./scripts/sync-upstream.sh v2.0.0
grep -q v2.0.0 .upstream-base
grep -q crost-patch crost.txt
grep -q upstream-v2 file.txt

# Conflict path: upstream and patch touch the same line
git checkout -q v1.0.0
echo crost-conflict > file.txt
git commit -q -am "crost conflicting patch"
echo v1.0.0 > .upstream-base
if ./scripts/sync-upstream.sh v2.0.0; then
  echo "expected sync to fail on conflict" >&2
  exit 1
fi
test ! -d .git/rebase-merge

echo "sync-upstream-test: ok"
