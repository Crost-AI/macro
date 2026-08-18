//! Done-when demo via `/webhooks/github` and `/webhooks/macro`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{StatusCode},
    routing::{get, patch, post},
};
use chrono::Utc;
use crost_issue_sync::{
    Config, GhIssue, GhIssueState, MacroTask, ProjectLink, StateStore, SyncEngine,
    backfill_project,
    webhook::{WebhookState, router},
};
use hmac::{Hmac, Mac};
use reqwest::Client;
use serde::Deserialize;
use serde_json::json;
use sha2::Sha256;
use uuid::Uuid;

const MACRO_SECRET: &str = "macro-webhook-secret";

#[derive(Clone, Default)]
struct FakeStore {
    tasks: Arc<Mutex<HashMap<String, MacroTask>>>,
    issues: Arc<Mutex<HashMap<u64, GhIssue>>>,
    comments: Arc<Mutex<Vec<(String, String)>>>,
    gh_comments: Arc<Mutex<Vec<(u64, String)>>>,
    next_issue: Arc<Mutex<u64>>,
    next_task: Arc<Mutex<u64>>,
    next_gh_comment: Arc<Mutex<u64>>,
}

#[derive(Clone)]
struct FakeState {
    store: FakeStore,
}

#[tokio::test]
async fn demo_bidirectional_sync_converges_without_echo_loops() {
    let fake = FakeStore::default();
    let macro_listener = spawn_macro_fake(fake.clone()).await;
    let gh_listener = spawn_github_fake(fake.clone()).await;

    let cfg = Config {
        state_db_path: "/tmp/unused".into(),
        macro_base_url: macro_listener.url(),
        macro_service_token: "test-token".into(),
        github_token: "gh-test".into(),
        github_api_base_url: gh_listener.url(),
        github_webhook_secret: None,
        macro_webhook_secret: Some(MACRO_SECRET.into()),
        listen_addr: "127.0.0.1:0".into(),
        projects: vec![ProjectLink {
            project_id: "proj-1".into(),
            gh_owner: "Crost-AI".into(),
            gh_repo: "macro".into(),
            open_status: "todo".into(),
            closed_status: "done".into(),
            label: None,
        }],
    };

    let store = StateStore::in_memory().unwrap();
    let engine = Arc::new(SyncEngine::new(cfg, store).unwrap());
    let sync_listener = spawn_sync_router(engine.clone()).await;
    let client = Client::new();
    let sync_url = sync_listener.url();

    // GitHub issue #42 already exists on GH before the webhook fires.
    fake.issues.lock().unwrap().insert(
        42,
        GhIssue {
            number: 42,
            title: "Bug".into(),
            body: "details".into(),
            state: GhIssueState::Open,
            labels: vec![],
            updated_at: Utc::now(),
        },
    );

    // 1) GitHub issue opened → Macro task (via webhook)
    post_github(
        &client,
        &sync_url,
        github_payload("opened", 42, "Bug", "details", "open", None),
    )
    .await;
    assert_eq!(fake.tasks.lock().unwrap().len(), 1);
    let task_ref = fake.tasks.lock().unwrap().keys().next().unwrap().clone();

    // 2) Macro task title edit → GitHub issue (via macro webhook)
  fake.tasks.lock().unwrap().get_mut(&task_ref).unwrap().title =
        "Bug (fixed title)".into();
    fake.tasks.lock().unwrap().get_mut(&task_ref).unwrap().updated_at = Utc::now();
    post_macro(
        &client,
        &sync_url,
        macro_event(
            "task.updated",
            json!({"task_id": task_ref, "project_id": "proj-1"}),
        ),
    )
    .await;
    assert_eq!(
        fake.issues.lock().unwrap().get(&42).unwrap().title,
        "Bug (fixed title)"
    );

    // 3) GitHub close → Macro status
    post_github(
        &client,
        &sync_url,
        github_payload("closed", 42, "Bug (fixed title)", "details", "closed", None),
    )
    .await;
    assert_eq!(
        fake.tasks.lock().unwrap().get(&task_ref).unwrap().status,
        "done"
    );

    // 4) Macro reopen → GitHub open
    fake.tasks.lock().unwrap().get_mut(&task_ref).unwrap().status = "todo".into();
    fake.tasks.lock().unwrap().get_mut(&task_ref).unwrap().updated_at = Utc::now();
    post_macro(
        &client,
        &sync_url,
        macro_event(
            "task.updated",
            json!({"task_id": task_ref, "project_id": "proj-1"}),
        ),
    )
    .await;
    assert_eq!(
        fake.issues.lock().unwrap().get(&42).unwrap().state,
        GhIssueState::Open
    );

    // 5) Comment both ways
    post_github_comment(
        &client,
        &sync_url,
        github_comment_payload(42, 9001, "from github"),
    )
    .await;
    assert!(fake
        .comments
        .lock()
        .unwrap()
        .iter()
        .any(|(_, text)| text == "from github"));

    post_macro(
        &client,
        &sync_url,
        macro_event(
            "task.comment",
            json!({
                "task_id": task_ref,
                "comment_id": "cmt-macro-1",
                "author": "alice",
                "text": "from macro"
            }),
        ),
    )
    .await;
    assert!(fake
        .gh_comments
        .lock()
        .unwrap()
        .iter()
        .any(|(_, text)| text.contains("from macro")));

    // 6) Echo: re-deliver macro update with unchanged hashes → no duplicate GH issue
    let issue_count = fake.issues.lock().unwrap().len();
    post_macro(
        &client,
        &sync_url,
        macro_event(
            "task.updated",
            json!({"task_id": task_ref, "project_id": "proj-1"}),
        ),
    )
    .await;
    assert_eq!(fake.issues.lock().unwrap().len(), issue_count);

    // 7) Macro-side create → GitHub issue
    let task_ref2 = "task-macro-new".to_string();
    fake.tasks.lock().unwrap().insert(
        task_ref2.clone(),
        MacroTask {
            r#ref: task_ref2.clone(),
            title: "From Macro".into(),
            body: "created on macro".into(),
            status: "todo".into(),
            labels: vec![],
            updated_at: Utc::now(),
        },
    );
    post_macro(
        &client,
        &sync_url,
        macro_event(
            "task.created",
            json!({"task_id": task_ref2, "project_id": "proj-1"}),
        ),
    )
    .await;
    assert_eq!(fake.issues.lock().unwrap().len(), 2);

    // 8) Backfill imports open issue #99 without duplicating #42
    fake.issues.lock().unwrap().insert(
        99,
        GhIssue {
            number: 99,
            title: "Backfill me".into(),
            body: "from repo".into(),
            state: GhIssueState::Open,
            labels: vec![],
            updated_at: Utc::now(),
        },
    );
    let imported = backfill_project(&engine, "proj-1").await.unwrap();
    assert_eq!(imported, 1);
    assert_eq!(fake.tasks.lock().unwrap().len(), 3);

    macro_listener.shutdown().await;
    gh_listener.shutdown().await;
    sync_listener.shutdown().await;
}

struct TestServer {
    url: String,
    shutdown: tokio::sync::oneshot::Sender<()>,
    handle: tokio::task::JoinHandle<()>,
}

impl TestServer {
    fn url(&self) -> String {
        self.url.clone()
    }

    async fn shutdown(self) {
        let _ = self.shutdown.send(());
        let _ = self.handle.await;
    }
}

async fn spawn_sync_router(engine: Arc<SyncEngine>) -> TestServer {
    let state = WebhookState {
        engine,
        github_secret: None,
        macro_secret: Some(MACRO_SECRET.into()),
    };
    bind_test_server(router(state)).await
}

async fn spawn_macro_fake(store: FakeStore) -> TestServer {
    let state = FakeState { store };
    let app = Router::new()
        .route("/api/v1/tasks", post(create_task))
        .route("/api/v1/tasks/{ref}", get(get_task))
        .route("/api/v1/tasks/{ref}/status", post(set_status))
        .route("/api/v1/tasks/{ref}/comment", post(add_comment))
        .with_state(state);
    bind_test_server(app).await
}

async fn spawn_github_fake(store: FakeStore) -> TestServer {
    let state = FakeState { store };
    let app = Router::new()
        .route("/repos/{owner}/{repo}/issues", post(create_issue).get(list_issues))
        .route("/repos/{owner}/{repo}/issues/{number}", patch(update_issue))
        .route(
            "/repos/{owner}/{repo}/issues/{number}/comments",
            post(add_issue_comment),
        )
        .with_state(state);
    bind_test_server(app).await
}

async fn bind_test_server(app: Router) -> TestServer {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = tokio::sync::oneshot::channel();
    let handle = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = rx.await;
            })
            .await
            .unwrap();
    });
    TestServer {
        url: format!("http://{addr}"),
        shutdown: tx,
        handle,
    }
}

async fn post_github(client: &Client, base: &str, payload: serde_json::Value) {
    let resp = client
        .post(format!("{base}/webhooks/github"))
        .header("x-github-event", "issues")
        .json(&payload)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

async fn post_github_comment(client: &Client, base: &str, payload: serde_json::Value) {
    let resp = client
        .post(format!("{base}/webhooks/github"))
        .header("x-github-event", "issue_comment")
        .json(&payload)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

fn github_payload(
    action: &str,
    number: u64,
    title: &str,
    body: &str,
    state: &str,
    comment: Option<serde_json::Value>,
) -> serde_json::Value {
    let mut v = json!({
        "action": action,
        "issue": {
            "number": number,
            "title": title,
            "body": body,
            "state": state,
            "labels": [],
            "updated_at": Utc::now(),
        },
        "repository": {
            "name": "macro",
            "owner": {"login": "Crost-AI"}
        }
    });
    if let Some(c) = comment {
        v["comment"] = c;
    }
    v
}

fn github_comment_payload(number: u64, comment_id: u64, body: &str) -> serde_json::Value {
    json!({
        "action": "created",
        "issue": {
            "number": number,
            "title": "Bug (fixed title)",
            "body": "details\n\n<!--macro-sync:sync-abc-->",
            "state": "open",
            "labels": [],
            "updated_at": Utc::now(),
        },
        "comment": {
            "id": comment_id,
            "body": body,
            "updated_at": Utc::now(),
        },
        "repository": {
            "name": "macro",
            "owner": {"login": "Crost-AI"}
        }
    })
}

async fn post_macro(client: &Client, base: &str, body: serde_json::Value) {
    let bytes = serde_json::to_vec(&body).unwrap();
    let timestamp = "1700000000";
    let payload = format!("{timestamp}.{}", String::from_utf8_lossy(&bytes));
    let mut mac = Hmac::<Sha256>::new_from_slice(MACRO_SECRET.as_bytes()).unwrap();
    mac.update(payload.as_bytes());
    let sig = format!("v1={}", hex::encode(mac.finalize().into_bytes()));

    let resp = client
        .post(format!("{base}/webhooks/macro"))
        .header("x-macro-timestamp", timestamp)
        .header("x-macro-signature", sig)
        .body(bytes)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

fn macro_event(event_type: &str, metadata: serde_json::Value) -> serde_json::Value {
    json!({
        "event_id": Uuid::new_v4().to_string(),
        "event_type": event_type,
        "metadata": metadata,
    })
}

#[derive(Deserialize)]
struct CreateTaskBody {
    title: String,
    body: String,
    #[allow(dead_code)]
    project_id: String,
    status: String,
    labels: Vec<String>,
}

async fn create_task(
    State(state): State<FakeState>,
    Json(body): Json<CreateTaskBody>,
) -> Json<serde_json::Value> {
    let mut next = state.store.next_task.lock().unwrap();
    *next += 1;
    let task_ref = format!("task-{next}");
    let task = MacroTask {
        r#ref: task_ref.clone(),
        title: body.title,
        body: body.body,
        status: body.status,
        labels: body.labels,
        updated_at: Utc::now(),
    };
    state
        .store
        .tasks
        .lock()
        .unwrap()
        .insert(task_ref.clone(), task);
    Json(json!({"ref": task_ref}))
}

async fn get_task(
    State(state): State<FakeState>,
    Path(task_ref): Path<String>,
) -> Json<MacroTask> {
    Json(
        state
            .store
            .tasks
            .lock()
            .unwrap()
            .get(&task_ref)
            .unwrap()
            .clone(),
    )
}

#[derive(Deserialize)]
struct StatusBody {
    status: String,
}

async fn set_status(
    State(state): State<FakeState>,
    Path(task_ref): Path<String>,
    Json(body): Json<StatusBody>,
) -> StatusCode {
    state
        .store
        .tasks
        .lock()
        .unwrap()
        .get_mut(&task_ref)
        .unwrap()
        .status = body.status;
    StatusCode::OK
}

#[derive(Deserialize)]
struct CommentBody {
    text: String,
}

async fn add_comment(
    State(state): State<FakeState>,
    Path(task_ref): Path<String>,
    Json(body): Json<CommentBody>,
) -> Json<serde_json::Value> {
    state
        .store
        .comments
        .lock()
        .unwrap()
        .push((task_ref, body.text));
    Json(json!({"comment_id": Uuid::new_v4().to_string(), "updated_at": Utc::now()}))
}

#[derive(Deserialize)]
struct CreateIssueBody {
    title: String,
    body: String,
    labels: Vec<String>,
}

async fn create_issue(
    State(state): State<FakeState>,
    Json(body): Json<CreateIssueBody>,
) -> Json<serde_json::Value> {
    let mut next = state.store.next_issue.lock().unwrap();
    *next += 1;
    let issue = GhIssue {
        number: *next,
        title: body.title,
        body: body.body,
        state: GhIssueState::Open,
        labels: body.labels,
        updated_at: Utc::now(),
    };
    state.store.issues.lock().unwrap().insert(*next, issue.clone());
    Json(issue_json(&issue))
}

#[derive(Deserialize)]
struct IssueQuery {
    state: Option<String>,
}

async fn list_issues(
    State(state): State<FakeState>,
    Query(q): Query<IssueQuery>,
) -> Json<Vec<serde_json::Value>> {
    let issues = state.store.issues.lock().unwrap();
    let out: Vec<_> = issues
        .values()
        .filter(|i| q.state.as_deref() != Some("open") || i.state == GhIssueState::Open)
        .map(issue_json)
        .collect();
    Json(out)
}

#[derive(Deserialize)]
struct UpdateIssueBody {
    title: Option<String>,
    body: Option<String>,
    state: Option<String>,
    labels: Option<Vec<String>>,
}

async fn update_issue(
    State(state): State<FakeState>,
    Path((_owner, _repo, number)): Path<(String, String, u64)>,
    Json(body): Json<UpdateIssueBody>,
) -> Json<serde_json::Value> {
    let mut issues = state.store.issues.lock().unwrap();
    let issue = issues.get_mut(&number).unwrap();
    if let Some(title) = body.title {
        issue.title = title;
    }
    if let Some(body) = body.body {
        issue.body = body;
    }
    if let Some(state_str) = body.state {
        issue.state = if state_str == "closed" {
            GhIssueState::Closed
        } else {
            GhIssueState::Open
        };
    }
    if let Some(labels) = body.labels {
        issue.labels = labels;
    }
    issue.updated_at = Utc::now();
    Json(issue_json(issue))
}

#[derive(Deserialize)]
struct GhCommentBody {
    body: String,
}

async fn add_issue_comment(
    State(state): State<FakeState>,
    Json(body): Json<GhCommentBody>,
) -> Json<serde_json::Value> {
    let mut next = state.store.next_gh_comment.lock().unwrap();
    *next += 1;
    state
        .store
        .gh_comments
        .lock()
        .unwrap()
        .push((*next, body.body.clone()));
    Json(json!({
        "id": *next,
        "body": body.body,
        "updated_at": Utc::now(),
    }))
}

fn issue_json(issue: &GhIssue) -> serde_json::Value {
    json!({
        "number": issue.number,
        "title": issue.title,
        "body": issue.body,
        "state": match issue.state { GhIssueState::Open => "open", GhIssueState::Closed => "closed" },
        "labels": issue.labels.iter().map(|n| json!({"name": n})).collect::<Vec<_>>(),
        "updated_at": issue.updated_at,
    })
}

#[tokio::test]
async fn github_comment_on_marked_issue_still_syncs() {
    let fake = FakeStore::default();
    let macro_listener = spawn_macro_fake(fake.clone()).await;
    let gh_listener = spawn_github_fake(fake.clone()).await;
    let cfg = Config {
        state_db_path: "/tmp/unused".into(),
        macro_base_url: macro_listener.url(),
        macro_service_token: "test-token".into(),
        github_token: "gh-test".into(),
        github_api_base_url: gh_listener.url(),
        github_webhook_secret: None,
        macro_webhook_secret: Some(MACRO_SECRET.into()),
        listen_addr: "127.0.0.1:0".into(),
        projects: vec![ProjectLink {
            project_id: "proj-1".into(),
            gh_owner: "Crost-AI".into(),
            gh_repo: "macro".into(),
            open_status: "todo".into(),
            closed_status: "done".into(),
            label: None,
        }],
    };
    let store = StateStore::in_memory().unwrap();
    let engine = Arc::new(SyncEngine::new(cfg, store).unwrap());
    let sync_listener = spawn_sync_router(engine).await;
    let client = Client::new();

    post_github(
        &client,
        &sync_listener.url(),
        github_payload("opened", 42, "Bug", "details", "open", None),
    )
    .await;
    fake.issues.lock().unwrap().insert(
        42,
        GhIssue {
            number: 42,
            title: "Bug".into(),
            body: "details".into(),
            state: GhIssueState::Open,
            labels: vec![],
            updated_at: Utc::now(),
        },
    );
    post_github_comment(
        &client,
        &sync_listener.url(),
        github_comment_payload(42, 9001, "from github"),
    )
    .await;
    assert!(fake
        .comments
        .lock()
        .unwrap()
        .iter()
        .any(|(_, t)| t == "from github"));

    macro_listener.shutdown().await;
    gh_listener.shutdown().await;
    sync_listener.shutdown().await;
}
