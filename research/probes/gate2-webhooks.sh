#!/usr/bin/env bash
# Gate 2: outgoing webhooks — register webhook, post channel message, capture delivery.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
# shellcheck source=common.sh
source "$SCRIPT_DIR/common.sh"

: "${WEBHOOK_LISTEN_PORT:=31024}"
: "${WEBHOOK_PATH:=/macro-events}"
: "${INSTANCE:=crost-trial}"
LOG=$(mktemp)
LISTENER_PID=""
TUNNEL_PID=""

cleanup() {
  [[ -n "$LISTENER_PID" ]] && kill "$LISTENER_PID" 2>/dev/null || true
  [[ -n "$TUNNEL_PID" ]] && kill "$TUNNEL_PID" 2>/dev/null || true
  rm -f "$LOG"
}
trap cleanup EXIT

start_listener() {
  LOG_FILE="$LOG" WEBHOOK_LISTEN_PORT="$WEBHOOK_LISTEN_PORT" python3 - <<'PY' &
import http.server
import json
import os

PORT = int(os.environ["WEBHOOK_LISTEN_PORT"])
LOG = os.environ["LOG_FILE"]

class H(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        length = int(self.headers.get("Content-Length", "0"))
        body = self.rfile.read(length)
        record = {
            "path": self.path,
            "headers": {k: v for k, v in self.headers.items()},
            "body": body.decode("utf-8", errors="replace"),
        }
        with open(LOG, "a", encoding="utf-8") as f:
            f.write(json.dumps(record) + "\n")
        self.send_response(200)
        self.end_headers()
        self.wfile.write(b"ok")

    def log_message(self, *_args):
        return

http.server.HTTPServer(("127.0.0.1", PORT), H).serve_forever()
PY
  LISTENER_PID=$!
  for _ in $(seq 1 20); do
    if lsof -iTCP:"$WEBHOOK_LISTEN_PORT" -sTCP:LISTEN >/dev/null 2>&1; then return 0; fi
    sleep 0.2
  done
  echo "FAIL: listener did not bind port $WEBHOOK_LISTEN_PORT"
  exit 1
}

restart_tunnel() {
  local key_dir pid_file ssh_port
  key_dir="$REPO_ROOT/infra/local/generated/${INSTANCE}/sdk-webhook"
  pid_file="$key_dir/tunnel.pid"
  ssh_port=$((PORT_BASE + 23))
  if [[ -f "$pid_file" ]]; then
    kill "$(cat "$pid_file")" 2>/dev/null || true
    rm -f "$pid_file"
  fi
  sleep 0.5
  ssh -N -T \
    -o ExitOnForwardFailure=yes \
    -o StrictHostKeyChecking=no \
    -o UserKnownHostsFile=/dev/null \
    -o ConnectTimeout=2 \
    -o ServerAliveInterval=15 \
    -o ServerAliveCountMax=3 \
    -i "$key_dir/id_ed25519" \
    -p "$ssh_port" \
    -R "0.0.0.0:8787:127.0.0.1:${WEBHOOK_LISTEN_PORT}" \
    sdk-webhook@127.0.0.1 &
  TUNNEL_PID=$!
  echo "$TUNNEL_PID" >"$pid_file"
  sleep 0.5
}

start_listener
restart_tunnel

ACCESS=$(macro_access_token)
NS="crost-w01-$(date +%s)"
WEBHOOK_URL="http://sdk-webhook-relay:8787/macro-events"

echo "=== gate2: POST /webhook/webhooks ==="
WH_BODY=$(python3 - <<PY
import json
print(json.dumps({
  "endpoint_url": "$WEBHOOK_URL",
  "namespace": "$NS",
  "name": "crost-w01-probe",
  "filters": [{"events": ["channel.message_posted"]}],
  "scope": "user",
}))
PY
)
CREATE_WH=$(curl -sS -w '\nHTTP_STATUS:%{http_code}\n' -X POST "$STORAGE/webhook/webhooks" \
  -H "Authorization: Bearer $ACCESS" \
  -H 'Content-Type: application/json' \
  -d "$WH_BODY")
echo "$CREATE_WH"
WH_ID=$(echo "$CREATE_WH" | sed '/HTTP_STATUS:/d' | python3 -c "import sys,json; print(json.load(sys.stdin)['id'])")

echo "=== gate2: POST /webhook/webhooks/{id}/validate ==="
curl -sS -w '\nHTTP_STATUS:%{http_code}\n' -X POST \
  -H "Authorization: Bearer $ACCESS" \
  "$STORAGE/webhook/webhooks/$WH_ID/validate"

CHANNEL_BODY='{"channel_type":"public","name":"crost-webhook-probe","participants":["macro|bob@seed.macro.local"]}'
CHANNEL=$(curl -sS -X POST "$STORAGE/channels" \
  -H "Authorization: Bearer $ACCESS" \
  -H 'Content-Type: application/json' \
  -d "$CHANNEL_BODY")
CHANNEL_ID=$(echo "$CHANNEL" | python3 -c "import sys,json; d=json.load(sys.stdin); print(d.get('channel_id') or d.get('id'))")
echo "channel_id=$CHANNEL_ID"

MSG_BODY='{"content":"crost w0.1 webhook probe","mentions":[],"attachments":[]}'
echo "=== gate2: POST /channels/{id}/message (trigger webhook) ==="
curl -sS -w '\nHTTP_STATUS:%{http_code}\n' -X POST \
  "$STORAGE/channels/$CHANNEL_ID/message" \
  -H "Authorization: Bearer $ACCESS" \
  -H 'Content-Type: application/json' \
  -d "$MSG_BODY"

echo "=== gate2: waiting for delivery (25s) ==="
for _ in $(seq 1 25); do
  if [[ -s "$LOG" ]]; then break; fi
  sleep 1
done

if [[ -s "$LOG" ]]; then
  echo "PASS: captured webhook delivery:"
  cat "$LOG"
else
  echo "FAIL: no webhook delivery captured on $WEBHOOK_URL"
  exit 1
fi
