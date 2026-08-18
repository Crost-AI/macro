use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{Result, SyncError};

/// Per-project link between a Macro project and a GitHub repository.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectLink {
    pub project_id: String,
    pub gh_owner: String,
    pub gh_repo: String,
    /// Macro status id/name for open GitHub issues.
    pub open_status: String,
    /// Macro status for closed GitHub issues.
    pub closed_status: String,
    /// Optional label applied to mirrored tasks.
    #[serde(default)]
    pub label: Option<String>,
}

/// Runtime configuration for the issue sync service.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Config {
    /// SQLite path for sync-state persistence.
    pub state_db_path: String,
    /// Macro storage base URL (e.g. http://localhost:31015).
    pub macro_base_url: String,
    /// Service token for Macro `/api/v1/*` (W2.4 contract).
    pub macro_service_token: String,
    /// GitHub personal access token or app installation token.
    pub github_token: String,
    /// GitHub REST API base (default https://api.github.com).
    #[serde(default = "default_github_api")]
    pub github_api_base_url: String,
    /// Optional secret for verifying GitHub webhooks (X-Hub-Signature-256).
    #[serde(default)]
    pub github_webhook_secret: Option<String>,
    /// Optional secret for verifying Macro outgoing webhooks (W2.7).
    #[serde(default)]
    pub macro_webhook_secret: Option<String>,
    /// HTTP listen address for webhook ingress.
    #[serde(default = "default_listen")]
    pub listen_addr: String,
    /// Project ↔ repo links to sync.
    pub projects: Vec<ProjectLink>,
}

fn default_listen() -> String {
    "127.0.0.1:8789".to_string()
}

fn default_github_api() -> String {
    "https://api.github.com".to_string()
}

impl Config {
    pub fn from_json_file(path: impl AsRef<Path>) -> Result<Self> {
        let raw = fs::read_to_string(path.as_ref())
            .map_err(|e| SyncError::Config(format!("read {}: {e}", path.as_ref().display())))?;
        Self::from_json_str(&raw)
    }

    pub fn from_json_str(raw: &str) -> Result<Self> {
        let cfg: Self = serde_json::from_str(raw)?;
        if cfg.projects.is_empty() {
            return Err(SyncError::Config("projects must not be empty".into()));
        }
        Ok(cfg)
    }

    pub fn from_env() -> Result<Self> {
        let path = std::env::var("CROST_ISSUE_SYNC_CONFIG").map_err(|_| {
            SyncError::Config("CROST_ISSUE_SYNC_CONFIG must point to a JSON config file".into())
        })?;
        Self::from_json_file(path)
    }

    pub fn project_link(&self, project_id: &str) -> Option<&ProjectLink> {
        self.projects.iter().find(|p| p.project_id == project_id)
    }

    pub fn project_for_repo(&self, owner: &str, repo: &str) -> Option<&ProjectLink> {
        self.projects
            .iter()
            .find(|p| p.gh_owner == owner && p.gh_repo == repo)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_config() {
        let raw = r#"{
            "state_db_path": "/tmp/sync.db",
            "macro_base_url": "http://localhost:31015",
            "macro_service_token": "tok",
            "github_token": "ghp_test",
            "projects": [{
                "project_id": "proj-1",
                "gh_owner": "Crost-AI",
                "gh_repo": "macro",
                "open_status": "todo",
                "closed_status": "done"
            }]
        }"#;
        let cfg = Config::from_json_str(raw).unwrap();
        assert_eq!(cfg.projects[0].gh_repo, "macro");
    }
}
