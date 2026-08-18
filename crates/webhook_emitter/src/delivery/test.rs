use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::{
    config::Config,
    delivery::{DeliveryClient, DeliveryResult, verify},
    events::{
        WebhookEnvelope, DOC_UPDATED, MESSAGE_POSTED, TASK_COMMENT, TASK_CREATED, TASK_UPDATED,
    },
};

const SECRET: &str = "test-webhook-secret";

async fn mount_ok(server: &MockServer) {
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200))
        .mount(server)
        .await;
}

async fn assert_signed_delivery(server: &MockServer, expected_event_type: &str) -> WebhookEnvelope {
    let requests = server.received_requests().await.expect("requests");
    let request = requests.last().expect("one request");
    let body = request.body.clone();
    let timestamp = request
        .headers
        .get("x-macro-timestamp")
        .expect("timestamp")
        .to_str()
        .expect("timestamp utf8");
    let signature = request
        .headers
        .get("x-macro-signature")
        .expect("signature")
        .to_str()
        .expect("signature utf8");

    assert!(
        verify(SECRET, timestamp, &body, signature),
        "signature must match timestamp.body"
    );

    let envelope: WebhookEnvelope = serde_json::from_slice(&body).expect("envelope json");
    assert_eq!(envelope.event_type, expected_event_type);
    envelope
}

fn client(server: &MockServer) -> DeliveryClient {
    DeliveryClient::new(Config {
        webhook_url: server.uri(),
        webhook_secret: SECRET.into(),
    })
    .expect("client")
}

#[tokio::test]
async fn listener_receives_signed_task_created() {
    let server = MockServer::start().await;
    mount_ok(&server).await;
    let envelope = WebhookEnvelope::new(TASK_CREATED, serde_json::json!({ "task_id": "t1" }));
    assert_eq!(client(&server).deliver(&envelope).await, DeliveryResult::Success);
    assert_signed_delivery(&server, TASK_CREATED).await;
}

#[tokio::test]
async fn listener_receives_signed_task_updated() {
    let server = MockServer::start().await;
    mount_ok(&server).await;
    let envelope = WebhookEnvelope::new(
        TASK_UPDATED,
        serde_json::json!({ "task_id": "t1", "status": "in_progress" }),
    );
    assert_eq!(client(&server).deliver(&envelope).await, DeliveryResult::Success);
    assert_signed_delivery(&server, TASK_UPDATED).await;
}

#[tokio::test]
async fn listener_receives_signed_task_comment() {
    let server = MockServer::start().await;
    mount_ok(&server).await;
    let envelope = WebhookEnvelope::new(
        TASK_COMMENT,
        serde_json::json!({ "task_id": "t1", "comment_id": "c1", "text": "hi" }),
    );
    assert_eq!(client(&server).deliver(&envelope).await, DeliveryResult::Success);
    assert_signed_delivery(&server, TASK_COMMENT).await;
}

#[tokio::test]
async fn listener_receives_signed_message_posted() {
    let server = MockServer::start().await;
    mount_ok(&server).await;
    let envelope = WebhookEnvelope::new(
        MESSAGE_POSTED,
        serde_json::json!({
            "channel_id": "ch-1",
            "author": "macro|user@example.com",
            "text": "hello",
            "mentions": [],
        }),
    );
    assert_eq!(client(&server).deliver(&envelope).await, DeliveryResult::Success);
    assert_signed_delivery(&server, MESSAGE_POSTED).await;
}

#[tokio::test]
async fn listener_receives_signed_doc_updated() {
    let server = MockServer::start().await;
    mount_ok(&server).await;
    let envelope = WebhookEnvelope::new(
        DOC_UPDATED,
        serde_json::json!({ "document_id": "doc-1" }),
    );
    assert_eq!(client(&server).deliver(&envelope).await, DeliveryResult::Success);
    assert_signed_delivery(&server, DOC_UPDATED).await;
}

#[tokio::test]
async fn retry_reuses_stable_event_id() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(500))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    let envelope = WebhookEnvelope::new(TASK_UPDATED, serde_json::json!({ "task_id": "t1" }));
    let event_id = envelope.event_id;
    let client = client(&server);

    assert!(matches!(
        client.deliver(&envelope).await,
        DeliveryResult::RetryableFailure(_)
    ));
    assert_eq!(client.deliver(&envelope).await, DeliveryResult::Success);

    let requests = server.received_requests().await.expect("requests");
    assert_eq!(requests.len(), 2);
    for request in requests {
        let body = request.body.clone();
        let parsed: WebhookEnvelope = serde_json::from_slice(&body).expect("envelope");
        assert_eq!(parsed.event_id, event_id);
    }
}
