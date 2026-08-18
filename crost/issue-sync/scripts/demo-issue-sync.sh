#!/usr/bin/env bash
# Demo: GitHub Issues ↔ Macro task bidirectional sync (W2.9)
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
cd "$ROOT"

echo "==> Running issue-sync integration demo (webhook handlers)"
cargo test -p crost-issue-sync --test integration_test -- --nocapture

echo "==> PASS: webhook-driven create/edit/close/reopen/comment, echo skip, macro create, backfill"
