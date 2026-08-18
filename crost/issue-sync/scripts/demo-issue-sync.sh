#!/usr/bin/env bash
# Demo: GitHub Issues ↔ Macro task bidirectional sync (W2.9)
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

echo "==> Running issue-sync integration demo (in-process fakes)"
SQLX_OFFLINE=true cargo test -p crost-issue-sync demo_bidirectional_sync_converges_without_echo_loops -- --nocapture

echo "==> PASS: create/edit/close/comment converge; backfill imports one new issue; echo loops skipped"
