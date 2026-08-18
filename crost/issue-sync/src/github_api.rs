use chrono::{DateTime, Utc};
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue, USER_AGENT};
use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::error::{Result, SyncError};
use crate::marker::embed_github_marker;
use crate::models::{GhComment, GhIssue, GhIssueState};

#[derive(Clone)]
pub struct GitHubClient {
    base_url: String,
    http: reqwest::Client,
}

impl GitHubClient {
    pub fn new(cfg: &Config) -> Result<Self> {
        let mut headers = HeaderMap::new();
        let value = format!("Bearer {}", cfg.github_token);
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&value)
                .map_err(|e| SyncError::Config(format!("invalid github token: {e}")))?,
        );
        headers.insert(
            USER_AGENT,
            HeaderValue::from_static("crost-issue-sync/0.1"),
        );
        let http = reqwest::Client::builder()
            .default_headers(headers)
            .build()?;
        Ok(Self {
            base_url: cfg.github_api_base_url.trim_end_matches('/').to_string(),
            http,
        })
    }

    pub fn from_parts(_token: String, base_url: String, http: reqwest::Client) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            http,
        }
    }

    fn issue_url(&self, owner: &str, repo: &str, number: u64) -> String {
        format!("{}/repos/{owner}/{repo}/issues/{number}", self.base_url)
    }

    fn issues_url(&self, owner: &str, repo: &str) -> String {
        format!("{}/repos/{owner}/{repo}/issues", self.base_url)
    }

    pub async fn get_issue(&self, owner: &str, repo: &str, number: u64) -> Result<GhIssue> {
        let url = self.issue_url(owner, repo, number);
        let resp = self.http.get(url).send().await?;
        parse_github_issue(resp).await
    }

    pub async fn create_issue(
        &self,
        owner: &str,
        repo: &str,
        title: &str,
        body: &str,
        labels: &[String],
        origin_id: &str,
    ) -> Result<GhIssue> {
        let req = CreateIssueRequest {
            title: title.to_string(),
            body: embed_github_marker(body, origin_id),
            labels: labels.to_vec(),
        };
        let url = self.issues_url(owner, repo);
        let resp = self.http.post(url).json(&req).send().await?;
        parse_github_issue(resp).await
    }

    pub async fn update_issue(
        &self,
        owner: &str,
        repo: &str,
        number: u64,
        title: Option<&str>,
        body: Option<&str>,
        state: Option<GhIssueState>,
        labels: Option<&[String]>,
        origin_id: &str,
    ) -> Result<GhIssue> {
        let req = UpdateIssueRequest {
            title: title.map(str::to_string),
            body: body.map(|b| embed_github_marker(b, origin_id)),
            state: state.map(gh_state_str),
            labels: labels.map(|v| v.to_vec()),
        };
        let url = self.issue_url(owner, repo, number);
        let resp = self.http.patch(url).json(&req).send().await?;
        parse_github_issue(resp).await
    }

    pub async fn add_comment(
        &self,
        owner: &str,
        repo: &str,
        number: u64,
        body: &str,
        origin_id: &str,
    ) -> Result<GhComment> {
        let req = CommentRequest {
            body: embed_github_marker(body, origin_id),
        };
        let url = format!(
            "{}/repos/{owner}/{repo}/issues/{number}/comments",
            self.base_url
        );
        let resp = self.http.post(url).json(&req).send().await?;
        if resp.status().is_success() {
            let payload: GitHubCommentResponse = resp.json().await?;
            Ok(GhComment {
                id: payload.id,
                body: payload.body.unwrap_or_default(),
                updated_at: payload.updated_at,
            })
        } else {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            Err(SyncError::GitHub { status, body })
        }
    }

    pub async fn list_open_issues(&self, owner: &str, repo: &str) -> Result<Vec<GhIssue>> {
        let url = format!(
            "{}/repos/{owner}/{repo}/issues?state=open&per_page=100",
            self.base_url
        );
        let resp = self.http.get(url).send().await?;
        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            return Err(SyncError::GitHub { status, body });
        }
        let items: Vec<GitHubIssueResponse> = resp.json().await?;
        Ok(items
            .into_iter()
            .filter(|i| i.pull_request.is_none())
            .filter_map(|i| i.into_issue().ok())
            .collect())
    }
}

#[derive(Debug, Serialize)]
struct CreateIssueRequest {
    title: String,
    body: String,
    labels: Vec<String>,
}

#[derive(Debug, Serialize)]
struct UpdateIssueRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    body: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    state: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    labels: Option<Vec<String>>,
}

#[derive(Debug, Serialize)]
struct CommentRequest {
    body: String,
}

#[derive(Debug, Deserialize)]
struct GitHubIssueResponse {
    number: u64,
    title: String,
    body: Option<String>,
    state: String,
    labels: Vec<GitHubLabelResponse>,
    updated_at: DateTime<Utc>,
    pull_request: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct GitHubLabelResponse {
    name: String,
}

#[derive(Debug, Deserialize)]
struct GitHubCommentResponse {
    id: u64,
    body: Option<String>,
    updated_at: DateTime<Utc>,
}

impl GitHubIssueResponse {
    fn into_issue(self) -> Result<GhIssue> {
        Ok(GhIssue {
            number: self.number,
            title: self.title,
            body: self.body.unwrap_or_default(),
            state: parse_gh_state(&self.state)?,
            labels: self.labels.into_iter().map(|l| l.name).collect(),
            updated_at: self.updated_at,
        })
    }
}

async fn parse_github_issue(resp: reqwest::Response) -> Result<GhIssue> {
    if resp.status().is_success() {
        let payload: GitHubIssueResponse = resp.json().await?;
        payload.into_issue()
    } else {
        let status = resp.status().as_u16();
        let body = resp.text().await.unwrap_or_default();
        Err(SyncError::GitHub { status, body })
    }
}

fn parse_gh_state(state: &str) -> Result<GhIssueState> {
    match state {
        "open" => Ok(GhIssueState::Open),
        "closed" => Ok(GhIssueState::Closed),
        other => Err(SyncError::Other(format!("unknown github state: {other}"))),
    }
}

fn gh_state_str(state: GhIssueState) -> &'static str {
    match state {
        GhIssueState::Open => "open",
        GhIssueState::Closed => "closed",
    }
}
