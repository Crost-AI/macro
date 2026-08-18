use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue};
use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::error::{Result, SyncError};
use crate::marker::MACRO_METADATA_KEY;
use crate::models::MacroTask;

#[derive(Clone)]
pub struct MacroClient {
    base_url: String,
    token: String,
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
            token: cfg.macro_service_token.clone(),
            http,
        })
    }

    pub fn from_parts(base_url: String, token: String, http: reqwest::Client) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            token,
            http,
        }
    }

    pub async fn create_task(
        &self,
        project_id: &str,
        title: &str,
        body: &str,
        status: &str,
        labels: &[String],
        origin_id: &str,
    ) -> Result<MacroTask> {
        let mut metadata = BTreeMap::new();
        metadata.insert(MACRO_METADATA_KEY.to_string(), origin_id.to_string());
        let req = CreateTaskRequest {
            title: title.to_string(),
            body: body.to_string(),
            project_id: project_id.to_string(),
            status: status.to_string(),
            labels: labels.to_vec(),
            metadata,
        };
        let resp = self
            .http
            .post(format!("{}/api/v1/tasks", self.base_url))
            .json(&req)
            .send()
            .await?;
        parse_macro_response(resp).await
    }

    pub async fn get_task(&self, task_id: &str) -> Result<MacroTask> {
        let resp = self
            .http
            .get(format!("{}/api/v1/tasks/{task_id}", self.base_url))
            .send()
            .await?;
        parse_macro_response(resp).await
    }

    pub async fn update_task(
        &self,
        task_id: &str,
        title: Option<&str>,
        body: Option<&str>,
        status: Option<&str>,
        labels: Option<&[String]>,
        origin_id: &str,
    ) -> Result<MacroTask> {
        let req = UpdateTaskRequest {
            title: title.map(str::to_string),
            body: body.map(str::to_string),
            status: status.map(str::to_string),
            labels: labels.map(|v| v.to_vec()),
            metadata: BTreeMap::from([(MACRO_METADATA_KEY.to_string(), origin_id.to_string())]),
        };
        let resp = self
            .http
            .patch(format!("{}/api/v1/tasks/{task_id}", self.base_url))
            .json(&req)
            .send()
            .await?;
        parse_macro_response(resp).await
    }

    pub async fn update_status(&self, task_id: &str, status: &str, origin_id: &str) -> Result<()> {
        let req = StatusRequest {
            status: status.to_string(),
            metadata: BTreeMap::from([(MACRO_METADATA_KEY.to_string(), origin_id.to_string())]),
        };
        let resp = self
            .http
            .post(format!("{}/api/v1/tasks/{task_id}/status", self.base_url))
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

    pub async fn add_comment(
        &self,
        task_id: &str,
        text: &str,
        origin_id: &str,
    ) -> Result<CommentResponse> {
        let req = CommentRequest {
            text: text.to_string(),
            metadata: BTreeMap::from([(MACRO_METADATA_KEY.to_string(), origin_id.to_string())]),
        };
        let resp = self
            .http
            .post(format!("{}/api/v1/tasks/{task_id}/comment", self.base_url))
            .json(&req)
            .send()
            .await?;
        parse_macro_response(resp).await
    }

    pub async fn list_open_tasks(&self, project_id: &str) -> Result<Vec<MacroTask>> {
        let resp = self
            .http
            .get(format!(
                "{}/api/v1/tasks?project_id={project_id}&status=open",
                self.base_url
            ))
            .send()
            .await?;
        parse_macro_response(resp).await
    }
}

#[derive(Debug, Serialize)]
struct CreateTaskRequest {
    title: String,
    body: String,
    project_id: String,
    status: String,
    labels: Vec<String>,
    metadata: BTreeMap<String, String>,
}

#[derive(Debug, Serialize)]
struct UpdateTaskRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    body: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    labels: Option<Vec<String>>,
    metadata: BTreeMap<String, String>,
}

#[derive(Debug, Serialize)]
struct StatusRequest {
    status: String,
    metadata: BTreeMap<String, String>,
}

#[derive(Debug, Serialize)]
struct CommentRequest {
    text: String,
    metadata: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
pub struct CommentResponse {
    pub comment_id: String,
    pub updated_at: DateTime<Utc>,
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
