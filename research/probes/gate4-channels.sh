#!/usr/bin/env bash
# Gate 4: channel create + archive (DELETE) via storage REST API.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=common.sh
source "$SCRIPT_DIR/common.sh"

ACCESS=$(macro_access_token)
CHANNEL_NAME="crost-w01-probe-$(date +%s)"
BODY=$(python3 - <<PY
import json
print(json.dumps({
  "channel_type": "public",
  "name": "$CHANNEL_NAME",
  "participants": ["macro|bob@seed.macro.local"],
}))
PY
)

echo "=== gate4: POST /channels ==="
CREATE=$(curl -sS -w '\nHTTP_STATUS:%{http_code}\n' -X POST "$STORAGE/channels" \
  -H "Authorization: Bearer $ACCESS" \
  -H 'Content-Type: application/json' \
  -d "$BODY")
echo "$CREATE"

CHANNEL_ID=$(echo "$CREATE" | sed '/HTTP_STATUS:/d' | python3 -c "import sys,json; d=json.load(sys.stdin); print(d.get('channel_id') or d.get('id',''))" 2>/dev/null || true)
if [[ -z "$CHANNEL_ID" ]]; then
  echo "FAIL: could not parse channel_id"
  exit 1
fi

echo
echo "=== gate4: GET /channels/{id} ==="
curl -sS -w '\nHTTP_STATUS:%{http_code}\n' \
  -H "Authorization: Bearer $ACCESS" \
  "$STORAGE/channels/$CHANNEL_ID"

echo
echo "=== gate4: DELETE /channels/{id} (archive) ==="
curl -sS -w '\nHTTP_STATUS:%{http_code}\n' -X DELETE \
  -H "Authorization: Bearer $ACCESS" \
  "$STORAGE/channels/$CHANNEL_ID"

echo
echo "=== gate4: GET after delete (expect 404) ==="
curl -sS -w '\nHTTP_STATUS:%{http_code}\n' \
  -H "Authorization: Bearer $ACCESS" \
  "$STORAGE/channels/$CHANNEL_ID"
