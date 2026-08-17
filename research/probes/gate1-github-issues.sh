#!/usr/bin/env bash
# Gate 1: 2-way GitHub Issues sync probe (upstream capabilities + local stubs).
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=common.sh
source "$SCRIPT_DIR/common.sh"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

echo "=== gate1: handled GitHub webhook event types (source scan) ==="
rg -n 'GithubWebhookEventType::|"issues"|issue_opened' \
  "$REPO_ROOT/crates/github/src/domain/models/sync.rs" \
  "$REPO_ROOT/crates/github/src/domain/service/sync" 2>/dev/null | head -40 || true

echo
echo "=== gate1: GitHub integration env stubs (--no-doppler) ==="
grep -E 'GITHUB_' "$REPO_ROOT/infra/local/generated/crost-trial/local.generated.env" | head -10 || true

echo
echo "=== gate1: github link status for seeded user (expects stub/disabled without real OAuth) ==="
ACCESS=$(macro_access_token)
curl -sS -w '\nHTTP_STATUS:%{http_code}\n' \
  -H "Authorization: Bearer $ACCESS" \
  "$PROXY/auth/link/github/status" || true

echo
echo "=== gate1: simulate issues webhook (expect unsupported/ignored — upstream handles PR events only) ==="
PAYLOAD='{"action":"opened","issue":{"number":1,"title":"crost probe","body":"from w0.1"}}'
SIG=$(printf '%s' "$PAYLOAD" | openssl dgst -sha256 -hmac 'local-github-webhook-secret' | sed 's/^.*= //')
curl -sS -w '\nHTTP_STATUS:%{http_code}\n' -X POST \
  -H 'X-GitHub-Event: issues' \
  -H "X-Hub-Signature-256: sha256=$SIG" \
  -H 'Content-Type: application/json' \
  -d "$PAYLOAD" \
  "$STORAGE/github/webhook" || true
