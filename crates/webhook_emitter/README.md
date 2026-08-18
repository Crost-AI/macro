# webhook_emitter

Crost fork delta (W2.7): singleton outgoing webhook emitter for the broker.

When `WEBHOOK_URL` and `WEBHOOK_SECRET` are set, Macro enqueues signed HTTP
deliveries for:

- `task.created`
- `task.updated` (including status changes)
- `task.comment`
- `message.posted` (channel id, author, text, thread id, mentions)
- `doc.updated`

Deliveries use `X-Macro-Signature: v1=<hex>` over
`{X-Macro-Timestamp}.{body}` (HMAC-SHA256). Each payload carries a stable
`event_id` (UUID v7) for broker idempotency; retries reuse the same id.

At-least-once semantics: rows live in `crost_webhook_outbox` until delivered or
dead-lettered after six failed attempts (30s between retries). Claimed rows are
leased for 120s so concurrent workers cannot double-deliver.

## Deferred (follow-up)

- `task.comment` on comment edit/delete paths (create-only hook today).
- Narrow `document_is_task` lookup if a cheaper task-id signal becomes available.

## Configuration

| Variable | Purpose |
| --- | --- |
| `WEBHOOK_URL` | Broker ingest URL (e.g. `http://broker:8080/webhooks/macro`) |
| `WEBHOOK_SECRET` | Shared HMAC secret (`X-Macro-Signature`) |

When either variable is unset, the emitter is disabled (no worker, no bridge).

## Local verification

```sh
# Terminal 1 — listener
python3 - <<'PY'
import http.server, json, os
class H(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        n = int(self.headers.get("Content-Length", "0"))
        body = self.rfile.read(n)
        print(json.dumps({
            "signature": self.headers.get("X-Macro-Signature"),
            "timestamp": self.headers.get("X-Macro-Timestamp"),
            "body": body.decode(),
        }, indent=2))
        self.send_response(200); self.end_headers()
    def log_message(self, *a): pass
http.server.HTTPServer(("127.0.0.1", 9191), H).serve_forever()
PY

# Terminal 2 — stack with emitter pointed at listener
export WEBHOOK_URL=http://host.docker.internal:9191/webhooks/macro
export WEBHOOK_SECRET=local-dev-secret
```

Then trigger task/channel/document activity; the listener should print signed
payloads. Re-run a failed delivery and confirm the `event_id` is unchanged.
