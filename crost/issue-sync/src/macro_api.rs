use chrono::{DateTime, Utc};
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue};
use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::error::{Result, SyncError};
use crate::models::MacroTask;

#[derive(Clone)]
pub struct MacroClient {
    base_url: String,
    http: reqwest::Client,
}

impl MacroClient {
    pub fn new(cfg: &Config) -> Result<Self> {
        let mut headers = HeaderMap::new();
        let value = format!("Bearer {}", cfg.macro_service_token);
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&value)
                .map_err(|e| SyncError::Config(format!("invalid macro token: {e}")))?,
        );
        let http = reqwest::Client::builder()
            .default_headers(headers)
            .build()?;
        Ok(Self {
            base_url: cfg.macro_base_url.trim_end_matches('/').to_string(),
            http,
        })
    }

    pub fn from_parts(base_url: String, _token: String, http: reqwest::Client) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            http,
        }
    }

    /// `POST /api/v1/tasks` → `{ref}` (W2.4).
    pub async fn create_task(&self, req: &CreateTaskRequest) -> Result<CreateTaskResponse> {
        let resp = self
            .http
            .post(format!("{}/api/v1/tasks", self.base_url))
            .json(req)
            .send()
            .await?;
        parse_macro_response(resp).await
    }

    /// `GET /api/v1/tasks/{ref}` (W2.4).
    pub async fn get_task(&self, task_ref: &str) -> Result<MacroTask> {
        let resp = self
            .http
            .get(format!("{}/api/v1/tasks/{task_ref}", self.base_url))
            .send()
            .await?;
        parse_macro_response(resp).await
    }

    /// `POST /api/v1/tasks/{ref}/status {status}` (W2.4).
    pub async fn update_status(&self, task_ref: &str, status: &str) -> Result<()> {
        let req = StatusRequest {
            status: status.to_string(),
        };
        let resp = self
            .http
            .post(format!("{}/api/v1/tasks/{task_ref}/status", self.base_url))
            .json(&req)
            .send()
            .await?;
        if resp.status().is_success() {
            Ok(())
        } else {
            let status_code = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            Err(SyncError::Macro {
                status: status_code,
                body,
            })
        }
    }

    /// `POST /api/v1/tasks/{ref}/comment {text}` (W2.4).
    pub async fn add_comment(&self, task_ref: &str, text: &str) -> Result<CommentResponse> {
        let req = CommentRequest {
            text: text.to_string(),
        };
        let resp = self
            .http
            .post(format!("{}/api/v1/tasks/{task_ref}/comment", self.base_url))
            .json(&req)
            .send()
            .await?;
        parse_macro_response(resp).await
    }

    /// `GET /api/v1/tasks?label=` → `{tasks:[...]}` (W2.4).
    pub async fn list_tasks_by_label(&self, label: &str) -> Result<Vec<MacroTask>> {
        let resp = self
            .http
            .get(format!(
                "{}/api/v1/tasks?label={label}",
                self.base_url
            ))
            .send()
            .await?;
        let body: ListTasksResponse = parse_macro_response(resp).await?;
        Ok(body.tasks)
    }
}

#[derive(Debug, Serialize)]
pub struct CreateTaskRequest {
    pub title: String,
    pub body: String,
    pub project_id: String,
    pub status: String,
    pub labels: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct CreateTaskResponse {
    pub r#ref: String,
}

#[derive(Debug, Serialize)]
struct StatusRequest {
    status: String,
}

#[derive(Debug, Serialize)]
struct CommentRequest {
    text: String,
}

#[derive(Debug, Deserialize)]
pub struct CommentResponse {
    pub comment_id: String,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
struct ListTasksResponse {
    tasks: Vec<MacroTask>,
}

async fn parse_macro_response<T: for<'de> Deserialize<'de>>(resp: reqwest::Response) -> Result<T> {
    if resp.status().is_success() {
        Ok(resp.json().await?)
    } else {
        let status = resp.status().as_u16();
        let body = resp.text().await.unwrap_or_default();
        Err(SyncError::Macro { status, body })
    }
}
