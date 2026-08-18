use serde_json::json;

use crate::{
    events::{MESSAGE_POSTED, TASK_CREATED, WebhookEnvelope},
    outbox::{MAX_ATTEMPTS, RETRY_DELAY_SECS},
};

#[test]
fn retry_policy_matches_blueprint() {
    assert_eq!(MAX_ATTEMPTS, 6);
    assert_eq!(RETRY_DELAY_SECS, 30);
}

#[test]
fn envelope_serializes_event_id() {
    let envelope = WebhookEnvelope::new(TASK_CREATED, json!({ "task_id": "abc" }));
    let value = serde_json::to_value(&envelope).expect("serialize");
    assert_eq!(value["event_type"], TASK_CREATED);
    assert!(value["event_id"].is_string());
}

#[test]
fn message_posted_metadata_shape() {
    let envelope = WebhookEnvelope::new(
        MESSAGE_POSTED,
        json!({
            "channel_id": "00000000-0000-0000-0000-000000000001",
            "author": "macro|user@example.com",
            "text": "hello",
            "thread_id": null,
            "mentions": [],
        }),
    );
    assert_eq!(envelope.event_type, MESSAGE_POSTED);
}
