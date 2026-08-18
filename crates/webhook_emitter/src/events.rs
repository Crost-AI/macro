//! Crost webhook event types and wire envelope.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// `task.created`
pub const TASK_CREATED: &str = "task.created";
/// `task.updated` (includes status transitions)
pub const TASK_UPDATED: &str = "task.updated";
/// `task.comment`
pub const TASK_COMMENT: &str = "task.comment";
/// `message.posted`
pub const MESSAGE_POSTED: &str = "message.posted";
/// `doc.updated`
pub const DOC_UPDATED: &str = "doc.updated";

/// Signed JSON body posted to `WEBHOOK_URL`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebhookEnvelope {
    /// Stable idempotency key for the broker.
    pub event_id: Uuid,
    /// Crost event name (`task.created`, `message.posted`, …).
    pub event_type: String,
    /// Event-specific payload.
    pub metadata: serde_json::Value,
}

impl WebhookEnvelope {
    /// Build a new envelope with a fresh UUID v7 `event_id`.
    pub fn new(event_type: impl Into<String>, metadata: serde_json::Value) -> Self {
        Self {
            event_id: Uuid::now_v7(),
            event_type: event_type.into(),
            metadata,
        }
    }
}
