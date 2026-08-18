use wiremock::matchers::{header_exists, method};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::{
    config::Config,
    delivery::{DeliveryClient, verify},
    events::{WebhookEnvelope, TASK_CREATED},
};

#[tokio::test]
async fn delivers_signed_payload() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(header_exists("x-macro-signature"))
        .and(header_exists("x-macro-timestamp"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    let config = Config {
        webhook_url: server.uri(),
        webhook_secret: "test-secret".into(),
    };
    let client = DeliveryClient::new(config).expect("client");
    let envelope = WebhookEnvelope::new(
        TASK_CREATED,
        serde_json::json!({ "task_id": "task-1" }),
    );

    let result = client.deliver(&envelope).await;
    assert_eq!(result, super::DeliveryResult::Success);
}

#[test]
fn signature_round_trip() {
    let body = br#"{"event_id":"01998a30-1a2b-7c3d-9e4f-5a6b7c8d9e0f","event_type":"task.created","metadata":{}}"#;
    let timestamp = "1700000000";
    let signature = super::sign("secret", timestamp, body).expect("sign");
    assert!(verify("secret", timestamp, body, &signature));
    assert!(!verify("wrong", timestamp, body, &signature));
}
