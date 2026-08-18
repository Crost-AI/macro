//! End-to-end sync demo: both sides create/edit/close/comment with no echo loops.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    routing::{get, patch, post},
};
use chrono::Utc;
use crost_issue_sync::{
    Config, GhIssue, GhIssueState, MacroTask, ProjectLink, StateStore, SyncEngine,
    backfill_project,
    models::{GitHubIssuesWebhook, GitHubIssuePayload, GitHubOwnerPayload, GitHubRepoPayload},
};
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

#[derive(Clone, Default)]
struct FakeStore {
    tasks: Arc<Mutex<HashMap<String, MacroTask>>>,
    issues: Arc<Mutex<HashMap<u64, GhIssue>>>,
    comments: Arc<Mutex<Vec<(String, String)>>>,
    next_issue: Arc<Mutex<u64>>,
    next_task: Arc<Mutex<u64>>,
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
        macro_webhook_secret: None,
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
    let engine = SyncEngine::new(cfg, store).unwrap();

    // 1) GitHub issue created → Macro task
    let gh_issue = GhIssue {
        number: 42,
        title: "Bug".into(),
        body: "details".into(),
        state: GhIssueState::Open,
        labels: vec!["bug".into()],
        updated_at: Utc::now(),
    };
    fake.issues.lock().unwrap().insert(42, gh_issue.clone());
    engine
        .sync_from_github(&engine.cfg.projects[0], &gh_issue)
        .await
        .unwrap();
    assert_eq!(fake.tasks.lock().unwrap().len(), 1);

    // 2) Macro task edit → GitHub issue
    {
        let mut tasks = fake.tasks.lock().unwrap();
        let task = tasks.values_mut().next().unwrap();
        task.title = "Bug (fixed title)".into();
        task.updated_at = Utc::now();
        let updated = task.clone();
        engine.sync_from_macro(&engine.cfg.projects[0], &updated).await.unwrap();
    }
    assert_eq!(
        fake.issues.lock().unwrap().get(&42).unwrap().title,
        "Bug (fixed title)"
    );

    // 3) Close on GitHub → Macro status
    let mut closed = fake.issues.lock().unwrap().get(&42).unwrap().clone();
    closed.state = GhIssueState::Closed;
    closed.updated_at = Utc::now();
    fake.issues.lock().unwrap().insert(42, closed.clone());
    engine
        .sync_from_github(&engine.cfg.projects[0], &closed)
        .await
        .unwrap();
    let task = fake.tasks.lock().unwrap().values().next().unwrap().clone();
    assert_eq!(task.status, "done");

    // 4) Reopen on Macro → GitHub open
    let mut reopened = task.clone();
    reopened.status = "todo".into();
    reopened.updated_at = Utc::now();
    engine
        .sync_from_macro(&engine.cfg.projects[0], &reopened)
        .await
        .unwrap();
    assert_eq!(
        fake.issues.lock().unwrap().get(&42).unwrap().state,
        GhIssueState::Open
    );

    // 5) Echo loop protection on webhook carrying our marker
    let echo_payload = GitHubIssuesWebhook {
        action: "edited".into(),
        issue: GitHubIssuePayload {
            number: 42,
            title: "Bug (fixed title)".into(),
            body: Some("<!--macro-sync:sync-echo-->".into()),
            state: "open".into(),
            labels: vec![],
            updated_at: Utc::now(),
        },
        repository: GitHubRepoPayload {
            name: "macro".into(),
            owner: GitHubOwnerPayload {
                login: "Crost-AI".into(),
            },
        },
        comment: None,
    };
    let err = engine.handle_github_webhook(echo_payload).await.unwrap_err();
    assert!(matches!(err, crost_issue_sync::SyncError::Echo { .. }));

    // 6) Backfill imports a new open issue without duplicating linked #42
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
    assert_eq!(fake.tasks.lock().unwrap().len(), 2);

    macro_listener.shutdown().await;
    gh_listener.shutdown().await;
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

async fn spawn_macro_fake(store: FakeStore) -> TestServer {
    let state = FakeState { store };
    let app = Router::new()
        .route("/api/v1/tasks", post(create_task))
        .route("/api/v1/tasks/{id}", get(get_task).patch(patch_task))
        .route("/api/v1/tasks/{id}/status", post(set_status))
        .route("/api/v1/tasks/{id}/comment", post(add_comment))
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

#[derive(Deserialize)]
struct CreateTaskBody {
    title: String,
    body: String,
    project_id: String,
    status: String,
    labels: Vec<String>,
    metadata: HashMap<String, String>,
}

async fn create_task(
    State(state): State<FakeState>,
    Json(body): Json<CreateTaskBody>,
) -> Json<MacroTask> {
    let mut next = state.store.next_task.lock().unwrap();
    *next += 1;
    let id = format!("task-{next}");
    let task = MacroTask {
        id: id.clone(),
        title: body.title,
        body: body.body,
        status: body.status,
        labels: body.labels,
        metadata: body.metadata.into_iter().collect::<BTreeMap<_, _>>(),
        updated_at: Utc::now(),
    };
    state.store.tasks.lock().unwrap().insert(id, task.clone());
    Json(task)
}

async fn get_task(
    State(state): State<FakeState>,
    Path(id): Path<String>,
) -> Json<MacroTask> {
    Json(state.store.tasks.lock().unwrap().get(&id).unwrap().clone())
}

#[derive(Deserialize)]
struct PatchTaskBody {
    title: Option<String>,
    body: Option<String>,
    status: Option<String>,
    labels: Option<Vec<String>>,
    metadata: HashMap<String, String>,
}

async fn patch_task(
    State(state): State<FakeState>,
    Path(id): Path<String>,
    Json(body): Json<PatchTaskBody>,
) -> Json<MacroTask> {
    let mut tasks = state.store.tasks.lock().unwrap();
    let task = tasks.get_mut(&id).unwrap();
    if let Some(title) = body.title {
        task.title = title;
    }
    if let Some(body) = body.body {
        task.body = body;
    }
    if let Some(status) = body.status {
        task.status = status;
    }
    if let Some(labels) = body.labels {
        task.labels = labels;
    }
    task.metadata.extend(body.metadata);
    task.updated_at = Utc::now();
    Json(task.clone())
}

#[derive(Deserialize)]
struct StatusBody {
    status: String,
    metadata: HashMap<String, String>,
}

async fn set_status(
    State(state): State<FakeState>,
    Path(id): Path<String>,
    Json(body): Json<StatusBody>,
) -> Json<serde_json::Value> {
    let mut tasks = state.store.tasks.lock().unwrap();
    let task = tasks.get_mut(&id).unwrap();
    task.status = body.status;
    task.metadata.extend(body.metadata);
    task.updated_at = Utc::now();
    Json(json!({}))
}

#[derive(Deserialize)]
struct CommentBody {
    text: String,
    metadata: HashMap<String, String>,
}

async fn add_comment(
    State(state): State<FakeState>,
    Path(id): Path<String>,
    Json(body): Json<CommentBody>,
) -> Json<serde_json::Value> {
    state
        .store
        .comments
        .lock()
        .unwrap()
        .push((id, body.text));
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
    State(_state): State<FakeState>,
    Json(body): Json<GhCommentBody>,
) -> Json<serde_json::Value> {
    Json(json!({
        "id": 1001u64,
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
