use macro_db_migrator::MACRO_DB_MIGRATIONS;
use sqlx::PgPool;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::{
    config::Config,
    events::{WebhookEnvelope, TASK_UPDATED},
    outbox::PgOutbox,
    worker::Worker,
};

const SECRET: &str = "outbox-retry-secret";

#[tokio::test]
async fn outbox_retry_preserves_event_id_through_worker() {
    let database_url = match std::env::var("DATABASE_URL") {
        Ok(url) => url,
        Err(_) => {
            eprintln!("skipping outbox_retry_preserves_event_id_through_worker: DATABASE_URL not set");
            return;
        }
    };

    let pool = PgPool::connect(&database_url).await.expect("connect");
    MACRO_DB_MIGRATIONS.run(&pool).await.expect("migrate");

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

    let outbox = PgOutbox::new(pool.clone());
    let envelope = WebhookEnvelope::new(TASK_UPDATED, serde_json::json!({ "task_id": "t-retry" }));
    let event_id = envelope.event_id;
    outbox.enqueue(&envelope).await.expect("enqueue");

    let worker = Worker::new(
        pool,
        Config {
            webhook_url: server.uri(),
            webhook_secret: SECRET.into(),
        },
    )
    .expect("worker");

    worker.tick().await;
    worker.tick().await;

    let requests = server.received_requests().await.expect("requests");
    assert_eq!(requests.len(), 2);
    for request in requests {
        let body = request.body.clone();
        let parsed: WebhookEnvelope = serde_json::from_slice(&body).expect("envelope");
        assert_eq!(parsed.event_id, event_id);
    }
}
