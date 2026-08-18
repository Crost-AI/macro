use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Canonical GitHub issue representation used by the sync engine.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GhIssue {
    pub number: u64,
    pub title: String,
    pub body: String,
    pub state: GhIssueState,
    pub labels: Vec<String>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum GhIssueState {
    Open,
    Closed,
}

/// Canonical Macro task representation (W2.4 contract subset).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MacroTask {
    pub id: String,
    pub title: String,
    pub body: String,
    pub status: String,
    pub labels: Vec<String>,
    #[serde(default)]
    pub metadata: std::collections::BTreeMap<String, String>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GhComment {
    pub id: u64,
    pub body: String,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MacroComment {
    pub id: String,
    pub body: String,
    pub updated_at: DateTime<Utc>,
}

/// GitHub `issues` webhook payload (subset).
#[derive(Debug, Clone, Deserialize)]
pub struct GitHubIssuesWebhook {
    pub action: String,
    pub issue: GitHubIssuePayload,
    pub repository: GitHubRepoPayload,
    #[serde(default)]
    pub comment: Option<GitHubCommentPayload>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GitHubIssuePayload {
    pub number: u64,
    pub title: String,
    #[serde(default)]
    pub body: Option<String>,
    pub state: String,
    #[serde(default)]
    pub labels: Vec<GitHubLabelPayload>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GitHubLabelPayload {
    pub name: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GitHubCommentPayload {
    pub id: u64,
    #[serde(default)]
    pub body: Option<String>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GitHubRepoPayload {
    pub name: String,
    pub owner: GitHubOwnerPayload,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GitHubOwnerPayload {
    pub login: String,
}

/// Macro outgoing webhook envelope (W2.7 contract subset).
#[derive(Debug, Clone, Deserialize)]
pub struct MacroWebhookEvent {
    pub event_id: String,
    pub event_type: String,
    pub metadata: serde_json::Value,
}

/// Stored sync link row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncLink {
    pub project_id: String,
    pub gh_owner: String,
    pub gh_repo: String,
    pub gh_issue_number: u64,
    pub macro_task_id: String,
    pub title_hash: String,
    pub body_hash: String,
    pub state_hash: String,
    pub labels_hash: String,
    pub gh_updated_at: Option<String>,
    pub macro_updated_at: Option<String>,
}
