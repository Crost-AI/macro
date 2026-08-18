//! Public enqueue API.

use serde_json::json;
use sqlx::PgPool;

use crate::{
    events::{self, WebhookEnvelope},
    outbox::PgOutbox,
};

/// Enqueue any Crost webhook envelope.
pub async fn enqueue(pool: &PgPool, envelope: WebhookEnvelope) -> anyhow::Result<()> {
    PgOutbox::new(pool.clone()).enqueue(&envelope).await
}

/// Enqueue a `task.comment` event.
pub async fn emit_task_comment(
    pool: &PgPool,
    task_id: &str,
    comment_id: &str,
    author: &str,
    text: &str,
) -> anyhow::Result<()> {
    let envelope = WebhookEnvelope::new(
        events::TASK_COMMENT,
        json!({
            "task_id": task_id,
            "comment_id": comment_id,
            "author": author,
            "text": text,
        }),
    );
    enqueue(pool, envelope).await
}

/// Enqueue a `task.comment` event when the emitter is configured.
pub async fn emit_task_comment_if_configured(
    pool: &PgPool,
    task_id: &str,
    comment_id: &str,
    author: &str,
    text: &str,
) {
    if crate::config::Config::from_env().is_none() {
        return;
    }
    if let Err(error) = emit_task_comment(pool, task_id, comment_id, author, text).await {
        tracing::error!(
            error = ?error,
            task_id,
            comment_id,
            "failed to enqueue crost task.comment webhook"
        );
    }
}
