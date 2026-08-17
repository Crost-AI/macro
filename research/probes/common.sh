#!/usr/bin/env bash
# Shared helpers for W0.1 gate probes against the crost-trial local stack.
set -euo pipefail

: "${PROXY:=http://localhost:31009}"
: "${STORAGE:=http://localhost:31015}"
: "${AUTH_EMAIL:=alice@seed.macro.local}"
: "${REDIRECT_URI:=http://localhost:31010/app/login}"
: "${INSTANCE:=crost-trial}"
: "${PORT_BASE:=31000}"

macro_access_token() {
  local code headers access
  code=$(curl -fsS -X POST "$PROXY/auth/login/passwordless" \
    -H 'Content-Type: application/json' \
    -d "{\"email\":\"$AUTH_EMAIL\",\"redirect_uri\":\"$REDIRECT_URI\"}" \
    | python3 -c "import sys,json; print(json.load(sys.stdin)['code'])")
  headers=$(mktemp)
  curl -fsS -D "$headers" -o /dev/null \
    "$PROXY/auth/oauth/passwordless/${code}?email=$(python3 -c "import urllib.parse; print(urllib.parse.quote('$AUTH_EMAIL'))")"
  access=$(grep -i 'set-cookie: local-macro-access-token=' "$headers" | sed 's/.*local-macro-access-token=\([^;]*\).*/\1/')
  rm -f "$headers"
  printf '%s' "$access"
}
