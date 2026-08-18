//! Background worker draining `crost_webhook_outbox`.

use std::time::Duration;

use tokio_util::sync::CancellationToken;

use crate::{
    config::Config,
    delivery::{DeliveryClient, DeliveryResult},
    events::WebhookEnvelope,
    outbox::PgOutbox,
};

const POLL_INTERVAL: Duration = Duration::from_secs(1);
const BATCH_SIZE: i64 = 16;

/// Polls the outbox and delivers signed webhook payloads.
pub struct Worker {
    outbox: PgOutbox,
    client: DeliveryClient,
}

impl Worker {
    /// Build a worker over the shared database pool and emitter config.
    pub fn new(pool: sqlx::PgPool, config: Config) -> anyhow::Result<Self> {
        Ok(Self {
            outbox: PgOutbox::new(pool),
            client: DeliveryClient::new(config)?,
        })
    }

    /// Run until `cancellation_token` is cancelled.
    pub async fn run(&self, cancellation_token: CancellationToken) {
        loop {
            tokio::select! {
                biased;
                () = cancellation_token.cancelled() => return,
                () = self.tick() => {}
            }
        }
    }

    async fn tick(&self) {
        match self.outbox.claim_due(BATCH_SIZE).await {
            Ok(rows) if rows.is_empty() => {
                tokio::time::sleep(POLL_INTERVAL).await;
            }
            Ok(rows) => {
                for row in rows {
                    self.deliver_row(row).await;
                }
            }
            Err(error) => {
                tracing::error!(error = ?error, "failed to claim crost webhook outbox rows");
                tokio::time::sleep(POLL_INTERVAL).await;
            }
        }
    }

    async fn deliver_row(&self, row: crate::outbox::OutboxRow) {
        let envelope: WebhookEnvelope = match serde_json::from_value(row.payload.clone()) {
            Ok(envelope) => envelope,
            Err(error) => {
                tracing::error!(
                    event_id = %row.event_id,
                    error = ?error,
                    "discarding crost webhook row with invalid payload"
                );
                let _ = self
                    .outbox
                    .record_failure(row.event_id, row.attempt_count, "invalid payload json")
                    .await;
                return;
            }
        };

        match self.client.deliver(&envelope).await {
            DeliveryResult::Success => {
                if let Err(error) = self.outbox.mark_delivered(row.event_id).await {
                    tracing::error!(
                        event_id = %row.event_id,
                        error = ?error,
                        "failed to mark crost webhook delivery delivered"
                    );
                }
            }
            DeliveryResult::RetryableFailure(message) | DeliveryResult::PermanentFailure(message) => {
                if let Err(error) = self
                    .outbox
                    .record_failure(row.event_id, row.attempt_count, &message)
                    .await
                {
                    tracing::error!(
                        event_id = %row.event_id,
                        error = ?error,
                        "failed to record crost webhook delivery failure"
                    );
                }
            }
        }
    }
}
