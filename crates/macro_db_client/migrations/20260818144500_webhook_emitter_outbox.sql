-- Crost outgoing webhook outbox (W2.7): at-least-once delivery to WEBHOOK_URL.

CREATE TABLE IF NOT EXISTS crost_webhook_outbox (
    event_id UUID PRIMARY KEY,
    event_type TEXT NOT NULL,
    payload JSONB NOT NULL,
    attempt_count INTEGER NOT NULL DEFAULT 0,
    next_attempt_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    delivered_at TIMESTAMPTZ,
    dead_letter BOOLEAN NOT NULL DEFAULT FALSE,
    last_error TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS crost_webhook_outbox_pending_idx
    ON crost_webhook_outbox (next_attempt_at)
    WHERE delivered_at IS NULL AND dead_letter = FALSE;
