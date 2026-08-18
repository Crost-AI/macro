//! Outbox row persistence.

#[cfg(test)]
mod test;

use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

use crate::events::WebhookEnvelope;

/// Maximum delivery attempts before dead-lettering.
pub const MAX_ATTEMPTS: i32 = 6;
/// Fixed retry delay between attempts.
pub const RETRY_DELAY_SECS: i64 = 30;

/// One pending or terminal outbox row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboxRow {
    /// Stable delivery id.
    pub event_id: Uuid,
    /// Crost event type string.
    pub event_type: String,
    /// Serialized [`WebhookEnvelope`] body.
    pub payload: Value,
    /// Number of completed HTTP attempts.
    pub attempt_count: i32,
    /// When the worker may try again.
    pub next_attempt_at: DateTime<Utc>,
}

/// Postgres-backed outbox store.
#[derive(Clone)]
pub struct PgOutbox {
    pool: PgPool,
}

impl PgOutbox {
    /// Wrap an existing pool.
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Insert a new delivery. The envelope's `event_id` is preserved.
    pub async fn enqueue(&self, envelope: &WebhookEnvelope) -> anyhow::Result<()> {
        let payload = serde_json::to_value(envelope)?;
        sqlx::query(
            r#"
            INSERT INTO crost_webhook_outbox (event_id, event_type, payload)
            VALUES ($1, $2, $3)
            "#,
        )
        .bind(envelope.event_id)
        .bind(&envelope.event_type)
        .bind(payload)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Whether `document_id` is stored as a task document.
    pub async fn document_is_task(&self, document_id: &str) -> anyhow::Result<bool> {
        let exists = sqlx::query_scalar::<_, bool>(
            r#"
            SELECT EXISTS (
                SELECT 1
                FROM document_sub_type
                WHERE document_id = $1 AND sub_type = 'task'
            )
            "#,
        )
        .bind(document_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(exists)
    }

    /// Claim due pending rows for delivery.
    pub async fn claim_due(&self, limit: i64) -> anyhow::Result<Vec<OutboxRow>> {
        let rows = sqlx::query(
            r#"
            SELECT event_id, event_type, payload, attempt_count, next_attempt_at
            FROM crost_webhook_outbox
            WHERE delivered_at IS NULL
              AND dead_letter = FALSE
              AND next_attempt_at <= NOW()
            ORDER BY next_attempt_at
            LIMIT $1
            FOR UPDATE SKIP LOCKED
            "#,
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|row| {
                use sqlx::Row;
                OutboxRow {
                    event_id: row.get("event_id"),
                    event_type: row.get("event_type"),
                    payload: row.get("payload"),
                    attempt_count: row.get("attempt_count"),
                    next_attempt_at: row.get("next_attempt_at"),
                }
            })
            .collect())
    }

    /// Mark a delivery successful.
    pub async fn mark_delivered(&self, event_id: Uuid) -> anyhow::Result<()> {
        sqlx::query(
            r#"
            UPDATE crost_webhook_outbox
            SET delivered_at = NOW()
            WHERE event_id = $1
            "#,
        )
        .bind(event_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Record a failed attempt, scheduling retry or dead-lettering.
    pub async fn record_failure(
        &self,
        event_id: Uuid,
        attempt_count: i32,
        error: &str,
    ) -> anyhow::Result<()> {
        let next_attempt = attempt_count + 1;
        if next_attempt >= MAX_ATTEMPTS {
            sqlx::query(
                r#"
                UPDATE crost_webhook_outbox
                SET attempt_count = $2,
                    dead_letter = TRUE,
                    last_error = $3
                WHERE event_id = $1
                "#,
            )
            .bind(event_id)
            .bind(next_attempt)
            .bind(error)
            .execute(&self.pool)
            .await?;
            tracing::error!(
                event_id = %event_id,
                attempts = next_attempt,
                error,
                "crost webhook delivery dead-lettered after max attempts"
            );
            return Ok(());
        }

        sqlx::query(
            r#"
            UPDATE crost_webhook_outbox
            SET attempt_count = $2,
                next_attempt_at = NOW() + ($3::bigint * INTERVAL '1 second'),
                last_error = $4
            WHERE event_id = $1
            "#,
        )
        .bind(event_id)
        .bind(next_attempt)
        .bind(RETRY_DELAY_SECS)
        .bind(error)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}
