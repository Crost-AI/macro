use uuid::Uuid;

use crate::config::{Config, ProjectLink};
use crate::error::{Result, SyncError};
use crate::github_api::GitHubClient;
use crate::hash::{hash_labels, hash_text};
use crate::macro_api::MacroClient;
use crate::marker::{MACRO_METADATA_KEY, parse_github_origin, strip_github_markers};
use crate::models::{
    GhIssue, GhIssueState, GitHubIssuesWebhook, MacroTask, MacroWebhookEvent, SyncLink,
};
use crate::state::StateStore;

pub struct SyncEngine {
    pub cfg: Config,
    pub store: StateStore,
    pub macro_client: MacroClient,
    pub github_client: GitHubClient,
}

impl SyncEngine {
    pub fn new(cfg: Config, store: StateStore) -> Result<Self> {
        let macro_client = MacroClient::new(&cfg)?;
        let github_client = GitHubClient::new(&cfg)?;
        Ok(Self {
            cfg,
            store,
            macro_client,
            github_client,
        })
    }

    pub fn origin_id(&self) -> String {
        format!("sync-{}", Uuid::new_v4())
    }

    pub async fn handle_github_webhook(&self, payload: GitHubIssuesWebhook) -> Result<()> {
        let link = self
            .cfg
            .project_for_repo(&payload.repository.owner.login, &payload.repository.name)
            .ok_or_else(|| {
                SyncError::Other(format!(
                    "no project link for {}/{}",
                    payload.repository.owner.login, payload.repository.name
                ))
            })?;

        if payload.action == "created"
            || payload.action == "opened"
            || payload.action == "edited"
            || payload.action == "closed"
            || payload.action == "reopened"
            || payload.action == "labeled"
            || payload.action == "unlabeled"
        {
            let issue = payload_to_issue(&payload)?;
            if let Some(origin) = parse_github_origin(&issue.body) {
                if origin.starts_with("sync-") {
                    return Err(SyncError::Echo { origin });
                }
            }
            self.sync_from_github(link, &issue).await?;
        }

        if payload.action == "created" || payload.action == "edited" {
            if let Some(comment) = payload.comment {
                let body = comment.body.unwrap_or_default();
                if let Some(origin) = parse_github_origin(&body) {
                    if origin.starts_with("sync-") {
                        return Err(SyncError::Echo { origin });
                    }
                }
                self.sync_comment_from_github(
                    link,
                    payload.issue.number,
                    comment.id,
                    &body,
                    comment.updated_at.to_rfc3339(),
                )
                .await?;
            }
        }

        Ok(())
    }

    pub async fn handle_macro_webhook(&self, event: MacroWebhookEvent) -> Result<()> {
        match event.event_type.as_str() {
            "task.created" | "task.updated" => {
                let project_id = event
                    .metadata
                    .get("project_id")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| SyncError::Other("missing project_id".into()))?;
                let link = self
                    .cfg
                    .project_link(project_id)
                    .ok_or_else(|| SyncError::Other(format!("unknown project {project_id}")))?;
                let task_id = event
                    .metadata
                    .get("task_id")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| SyncError::Other("missing task_id".into()))?;
                if let Some(origin) = event
                    .metadata
                    .get("metadata")
                    .and_then(|m| m.get(MACRO_METADATA_KEY))
                    .and_then(|v| v.as_str())
                {
                    if origin.starts_with("sync-") {
                        return Err(SyncError::Echo {
                            origin: origin.to_string(),
                        });
                    }
                }
                let task = self.macro_client.get_task(task_id).await?;
                self.sync_from_macro(link, &task).await?;
            }
            "task.comment" => {
                let project_id = event
                    .metadata
                    .get("project_id")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| SyncError::Other("missing project_id".into()))?;
                let link = self
                    .cfg
                    .project_link(project_id)
                    .ok_or_else(|| SyncError::Other(format!("unknown project {project_id}")))?;
                let task_id = event
                    .metadata
                    .get("task_id")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| SyncError::Other("missing task_id".into()))?;
                let comment_id = event
                    .metadata
                    .get("comment_id")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| SyncError::Other("missing comment_id".into()))?;
                let text = event
                    .metadata
                    .get("text")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if let Some(origin) = event
                    .metadata
                    .get("metadata")
                    .and_then(|m| m.get(MACRO_METADATA_KEY))
                    .and_then(|v| v.as_str())
                {
                    if origin.starts_with("sync-") {
                        return Err(SyncError::Echo {
                            origin: origin.to_string(),
                        });
                    }
                }
                self.sync_comment_from_macro(link, task_id, comment_id, text)
                    .await?;
            }
            _ => {}
        }
        Ok(())
    }

    pub async fn sync_from_github(&self, link: &ProjectLink, issue: &GhIssue) -> Result<()> {
        let owner = &link.gh_owner;
        let repo = &link.gh_repo;
        let clean_body = strip_github_markers(&issue.body);
        let title_hash = hash_text(&issue.title);
        let body_hash = hash_text(&clean_body);
        let state_hash = hash_text(&format!("{:?}", issue.state));
        let labels_hash = hash_labels(&issue.labels);
        let gh_updated = issue.updated_at.to_rfc3339();

        if let Some(existing) = self.store.link_by_issue(
            &link.project_id,
            owner,
            repo,
            issue.number,
        )? {
            if should_apply_github(&existing, &gh_updated) {
                let status = gh_status_to_macro(issue.state, link);
                let origin = self.origin_id();
                self.macro_client
                    .update_task(
                        &existing.macro_task_id,
                        Some(&issue.title),
                        Some(&clean_body),
                        Some(status),
                        Some(&issue.labels),
                        &origin,
                    )
                    .await?;
                self.store.upsert_link(&SyncLink {
                    project_id: link.project_id.clone(),
                    gh_owner: owner.clone(),
                    gh_repo: repo.clone(),
                    gh_issue_number: issue.number,
                    macro_task_id: existing.macro_task_id,
                    title_hash,
                    body_hash,
                    state_hash,
                    labels_hash,
                    gh_updated_at: Some(gh_updated),
                    macro_updated_at: existing.macro_updated_at,
                })?;
            }
            return Ok(());
        }

        let origin = self.origin_id();
        let status = gh_status_to_macro(issue.state, link);
        let task = self
            .macro_client
            .create_task(
                &link.project_id,
                &issue.title,
                &clean_body,
                status,
                &issue.labels,
                &origin,
            )
            .await?;
        self.store.upsert_link(&SyncLink {
            project_id: link.project_id.clone(),
            gh_owner: owner.clone(),
            gh_repo: repo.clone(),
            gh_issue_number: issue.number,
            macro_task_id: task.id.clone(),
            title_hash,
            body_hash,
            state_hash,
            labels_hash,
            gh_updated_at: Some(gh_updated),
            macro_updated_at: Some(task.updated_at.to_rfc3339()),
        })?;
        Ok(())
    }

    pub async fn sync_from_macro(&self, link: &ProjectLink, task: &MacroTask) -> Result<()> {
        let owner = &link.gh_owner;
        let repo = &link.gh_repo;
        let title_hash = hash_text(&task.title);
        let body_hash = hash_text(&task.body);
        let state_hash = hash_text(&task.status);
        let labels_hash = hash_labels(&task.labels);
        let macro_updated = task.updated_at.to_rfc3339();

        if let Some(existing) = self.store.link_by_task(&link.project_id, &task.id)? {
            if should_apply_macro(&existing, &macro_updated) {
                let origin = self.origin_id();
                let gh_state = macro_status_to_gh(&task.status, link);
                self.github_client
                    .update_issue(
                        owner,
                        repo,
                        existing.gh_issue_number,
                        Some(&task.title),
                        Some(&task.body),
                        Some(gh_state),
                        Some(&task.labels),
                        &origin,
                    )
                    .await?;
                self.store.upsert_link(&SyncLink {
                    project_id: link.project_id.clone(),
                    gh_owner: owner.clone(),
                    gh_repo: repo.clone(),
                    gh_issue_number: existing.gh_issue_number,
                    macro_task_id: task.id.clone(),
                    title_hash,
                    body_hash,
                    state_hash,
                    labels_hash,
                    gh_updated_at: existing.gh_updated_at,
                    macro_updated_at: Some(macro_updated),
                })?;
            }
            return Ok(());
        }

        let origin = self.origin_id();
        let issue = self
            .github_client
            .create_issue(
                owner,
                repo,
                &task.title,
                &task.body,
                &task.labels,
                &origin,
            )
            .await?;
        self.store.upsert_link(&SyncLink {
            project_id: link.project_id.clone(),
            gh_owner: owner.clone(),
            gh_repo: repo.clone(),
            gh_issue_number: issue.number,
            macro_task_id: task.id.clone(),
            title_hash,
            body_hash,
            state_hash,
            labels_hash,
            gh_updated_at: Some(issue.updated_at.to_rfc3339()),
            macro_updated_at: Some(macro_updated),
        })?;
        Ok(())
    }

    async fn sync_comment_from_github(
        &self,
        link: &ProjectLink,
        issue_number: u64,
        gh_comment_id: u64,
        body: &str,
        gh_updated: String,
    ) -> Result<()> {
        let clean = strip_github_markers(body);
        let body_hash = hash_text(&clean);
        if let Some((_, existing_hash)) = self.store.comment_by_gh(
            &link.project_id,
            &link.gh_owner,
            &link.gh_repo,
            gh_comment_id,
        )? {
            if existing_hash == body_hash {
                return Ok(());
            }
        }
        let issue_link = self
            .store
            .link_by_issue(&link.project_id, &link.gh_owner, &link.gh_repo, issue_number)?
            .ok_or_else(|| SyncError::Other("issue not linked yet".into()))?;
        let origin = self.origin_id();
        let comment = self
            .macro_client
            .add_comment(&issue_link.macro_task_id, &clean, &origin)
            .await?;
        self.store.upsert_comment(
            &link.project_id,
            &link.gh_owner,
            &link.gh_repo,
            gh_comment_id,
            &comment.comment_id,
            &body_hash,
            Some(&gh_updated),
            Some(&comment.updated_at.to_rfc3339()),
        )?;
        Ok(())
    }

    async fn sync_comment_from_macro(
        &self,
        link: &ProjectLink,
        task_id: &str,
        macro_comment_id: &str,
        text: &str,
    ) -> Result<()> {
        let body_hash = hash_text(text);
        let issue_link = self
            .store
            .link_by_task(&link.project_id, task_id)?
            .ok_or_else(|| SyncError::Other("task not linked yet".into()))?;
        let origin = self.origin_id();
        let comment = self
            .github_client
            .add_comment(
                &link.gh_owner,
                &link.gh_repo,
                issue_link.gh_issue_number,
                text,
                &origin,
            )
            .await?;
        self.store.upsert_comment(
            &link.project_id,
            &link.gh_owner,
            &link.gh_repo,
            comment.id,
            macro_comment_id,
            &body_hash,
            Some(&comment.updated_at.to_rfc3339()),
            None,
        )?;
        Ok(())
    }
}

fn payload_to_issue(payload: &GitHubIssuesWebhook) -> Result<GhIssue> {
    let state = match payload.issue.state.as_str() {
        "open" => GhIssueState::Open,
        "closed" => GhIssueState::Closed,
        other => return Err(SyncError::Other(format!("unknown state {other}"))),
    };
    Ok(GhIssue {
        number: payload.issue.number,
        title: payload.issue.title.clone(),
        body: payload.issue.body.clone().unwrap_or_default(),
        state,
        labels: payload
            .issue
            .labels
            .iter()
            .map(|l| l.name.clone())
            .collect(),
        updated_at: payload.issue.updated_at,
    })
}

fn gh_status_to_macro(state: GhIssueState, link: &ProjectLink) -> &str {
    match state {
        GhIssueState::Open => &link.open_status,
        GhIssueState::Closed => &link.closed_status,
    }
}

fn macro_status_to_gh(status: &str, link: &ProjectLink) -> GhIssueState {
    if status == link.closed_status {
        GhIssueState::Closed
    } else {
        GhIssueState::Open
    }
}

/// Last-writer-wins per side using stored timestamps.
fn should_apply_github(existing: &SyncLink, gh_updated: &str) -> bool {
    match (&existing.gh_updated_at, &existing.macro_updated_at) {
        (Some(prev), Some(macro_ts)) => gh_updated >= prev.as_str() && gh_updated >= macro_ts.as_str(),
        (Some(prev), None) => gh_updated >= prev.as_str(),
        _ => true,
    }
}

fn should_apply_macro(existing: &SyncLink, macro_updated: &str) -> bool {
    match (&existing.macro_updated_at, &existing.gh_updated_at) {
        (Some(prev), Some(gh_ts)) => {
            macro_updated >= prev.as_str() && macro_updated >= gh_ts.as_str()
        }
        (Some(prev), None) => macro_updated >= prev.as_str(),
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lww_prefers_newer_github_timestamp() {
        let link = SyncLink {
            project_id: "p".into(),
            gh_owner: "o".into(),
            gh_repo: "r".into(),
            gh_issue_number: 1,
            macro_task_id: "t".into(),
            title_hash: String::new(),
            body_hash: String::new(),
            state_hash: String::new(),
            labels_hash: String::new(),
            gh_updated_at: Some("2026-01-01T00:00:00Z".into()),
            macro_updated_at: Some("2026-01-02T00:00:00Z".into()),
        };
        assert!(!should_apply_github(&link, "2026-01-01T12:00:00Z"));
        assert!(should_apply_github(&link, "2026-01-03T00:00:00Z"));
    }
}
