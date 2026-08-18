use uuid::Uuid;

use crate::config::{Config, ProjectLink};
use crate::error::{Result, SyncError};
use crate::github_api::GitHubClient;
use crate::hash::{hash_labels, hash_text};
use crate::macro_api::{CreateTaskRequest, MacroClient};
use crate::marker::{parse_github_origin, strip_github_markers};
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct FieldHashes {
    title: String,
    body: String,
    state: String,
    labels: String,
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

        let is_issue_action = matches!(
            payload.action.as_str(),
            "created" | "opened" | "edited" | "closed" | "reopened" | "labeled" | "unlabeled"
        );

        if is_issue_action {
            let issue = payload_to_issue(&payload)?;
            let hashes = issue_field_hashes(&issue);
            let existing = self.store.link_by_issue(
                &link.project_id,
                &link.gh_owner,
                &link.gh_repo,
                issue.number,
            )?;
            let skip_issue = existing
                .as_ref()
                .is_some_and(|row| github_issue_is_echo(row, &hashes));
            if !skip_issue {
                self.sync_from_github(link, &issue).await?;
            }
        }

        if matches!(payload.action.as_str(), "created" | "edited") {
            if let Some(comment) = payload.comment {
                let body = comment.body.unwrap_or_default();
                let clean = strip_github_markers(&body);
                let body_hash = hash_text(&clean);
                if let Some((_, stored_hash)) = self.store.comment_by_gh(
                    &link.project_id,
                    &link.gh_owner,
                    &link.gh_repo,
                    comment.id,
                )? {
                    if stored_hash == body_hash {
                        return Ok(());
                    }
                }
                // Origin marker alone is not an echo if content changed.
                let _ = parse_github_origin(&body);
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
                let task_ref = metadata_str(&event.metadata, "task_id")?;
                let task = self.macro_client.get_task(task_ref).await?;
                if let Some(link) = self.store.link_by_task_ref(task_ref)? {
                    let project_link = self
                        .cfg
                        .project_link(&link.project_id)
                        .ok_or_else(|| SyncError::Other("project link missing".into()))?;
                    if macro_task_is_echo(&link, &task) {
                        return Ok(());
                    }
                    self.sync_from_macro(project_link, &task).await?;
                } else {
                    let project_id = metadata_str(&event.metadata, "project_id")
                        .or_else(|_| single_project_id(&self.cfg))?;
                    let project_link = self
                        .cfg
                        .project_link(project_id)
                        .ok_or_else(|| SyncError::Other("project link missing".into()))?;
                    self.sync_from_macro(project_link, &task).await?;
                }
            }
            "task.comment" => {
                let task_ref = metadata_str(&event.metadata, "task_id")?;
                let comment_id = metadata_str(&event.metadata, "comment_id")?;
                let text = metadata_str(&event.metadata, "text")?;
                let link = self
                    .store
                    .link_by_task_ref(task_ref)?
                    .ok_or_else(|| SyncError::Other(format!("no link for task {task_ref}")))?;
                let project_link = self
                    .cfg
                    .project_link(&link.project_id)
                    .ok_or_else(|| SyncError::Other("project link missing".into()))?;
                let body_hash = hash_text(text);
                if let Some((stored_macro_id, stored_hash)) = self.store.comment_by_macro(
                    &link.project_id,
                    comment_id,
                )? {
                    if stored_hash == body_hash && !stored_macro_id.is_empty() {
                        return Ok(());
                    }
                }
                self.sync_comment_from_macro(project_link, task_ref, comment_id, text)
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
        let hashes = issue_field_hashes(issue);
        let gh_updated = issue.updated_at.to_rfc3339();

        if let Some(existing) = self.store.link_by_issue(
            &link.project_id,
            owner,
            repo,
            issue.number,
        )? {
            if !github_side_wins(&existing, &gh_updated) {
                return Ok(());
            }

            let mut next = existing.clone();
            next.gh_updated_at = Some(gh_updated.clone());

            if hashes.state != existing.state_hash {
                let status = gh_status_to_macro(issue.state, link);
                self.macro_client
                    .update_status(&existing.macro_task_id, status)
                    .await?;
                next.state_hash = hashes.state.clone();
            }

            // Title/body/labels: no W2.4 update route — tracked locally only (see DEFERRALS.md).
            if hashes.title != existing.title_hash {
                next.title_hash = hashes.title.clone();
            }
            if hashes.body != existing.body_hash {
                next.body_hash = hashes.body.clone();
            }
            if hashes.labels != existing.labels_hash {
                next.labels_hash = hashes.labels.clone();
            }

            self.store.upsert_link(&next)?;
            return Ok(());
        }

        let status = gh_status_to_macro(issue.state, link);
        let created = self
            .macro_client
            .create_task(&CreateTaskRequest {
                title: issue.title.clone(),
                body: clean_body,
                project_id: link.project_id.clone(),
                status: status.to_string(),
                labels: issue.labels.clone(),
            })
            .await?;
        let task = self.macro_client.get_task(&created.r#ref).await?;
        self.store.upsert_link(&SyncLink {
            project_id: link.project_id.clone(),
            gh_owner: owner.clone(),
            gh_repo: repo.clone(),
            gh_issue_number: issue.number,
            macro_task_id: created.r#ref,
            title_hash: hashes.title,
            body_hash: hashes.body,
            state_hash: hashes.state,
            labels_hash: hashes.labels,
            gh_updated_at: Some(gh_updated),
            macro_updated_at: Some(task.updated_at.to_rfc3339()),
        })?;
        Ok(())
    }

    pub async fn sync_from_macro(&self, link: &ProjectLink, task: &MacroTask) -> Result<()> {
        let owner = &link.gh_owner;
        let repo = &link.gh_repo;
        let hashes = task_field_hashes(task);
        let macro_updated = task.updated_at.to_rfc3339();

        if let Some(existing) = self.store.link_by_task(&link.project_id, &task.r#ref)? {
            if !macro_side_wins(&existing, &macro_updated) {
                return Ok(());
            }

            let origin = self.origin_id();
            let gh_state = macro_status_to_gh(&task.status, link);

            let title = if hashes.title != existing.title_hash {
                Some(task.title.as_str())
            } else {
                None
            };
            let body = if hashes.body != existing.body_hash {
                Some(task.body.as_str())
            } else {
                None
            };
            let state = if hashes.state != existing.state_hash {
                Some(gh_state)
            } else {
                None
            };
            let labels = if hashes.labels != existing.labels_hash {
                Some(task.labels.as_slice())
            } else {
                None
            };

            if title.is_some() || body.is_some() || state.is_some() || labels.is_some() {
                self.github_client
                    .update_issue(
                        owner,
                        repo,
                        existing.gh_issue_number,
                        title,
                        body,
                        state,
                        labels,
                        &origin,
                    )
                    .await?;
            }

            self.store.upsert_link(&SyncLink {
                project_id: link.project_id.clone(),
                gh_owner: owner.clone(),
                gh_repo: repo.clone(),
                gh_issue_number: existing.gh_issue_number,
                macro_task_id: task.r#ref.clone(),
                title_hash: hashes.title,
                body_hash: hashes.body,
                state_hash: hashes.state,
                labels_hash: hashes.labels,
                gh_updated_at: existing.gh_updated_at,
                macro_updated_at: Some(macro_updated),
            })?;
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
        let issue_hashes = issue_field_hashes(&issue);
        self.store.upsert_link(&SyncLink {
            project_id: link.project_id.clone(),
            gh_owner: owner.clone(),
            gh_repo: repo.clone(),
            gh_issue_number: issue.number,
            macro_task_id: task.r#ref.clone(),
            title_hash: issue_hashes.title,
            body_hash: issue_hashes.body,
            state_hash: issue_hashes.state,
            labels_hash: issue_hashes.labels,
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
        let issue_link = self
            .store
            .link_by_issue(&link.project_id, &link.gh_owner, &link.gh_repo, issue_number)?
            .ok_or_else(|| SyncError::Other("issue not linked yet".into()))?;
        let comment = self
            .macro_client
            .add_comment(&issue_link.macro_task_id, &clean)
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
        task_ref: &str,
        macro_comment_id: &str,
        text: &str,
    ) -> Result<()> {
        let body_hash = hash_text(text);
        let issue_link = self
            .store
            .link_by_task(&link.project_id, task_ref)?
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

fn metadata_str<'a>(metadata: &'a serde_json::Value, key: &str) -> Result<&'a str> {
    metadata
        .get(key)
        .and_then(|v| v.as_str())
        .ok_or_else(|| SyncError::Other(format!("missing {key}")))
}

fn issue_field_hashes(issue: &GhIssue) -> FieldHashes {
    FieldHashes {
        title: hash_text(&issue.title),
        body: hash_text(&strip_github_markers(&issue.body)),
        state: hash_text(&format!("{:?}", issue.state)),
        labels: hash_labels(&issue.labels),
    }
}

fn task_field_hashes(task: &MacroTask) -> FieldHashes {
    FieldHashes {
        title: hash_text(&task.title),
        body: hash_text(&task.body),
        state: hash_text(&task.status),
        labels: hash_labels(&task.labels),
    }
}

fn github_issue_is_echo(existing: &SyncLink, hashes: &FieldHashes) -> bool {
    existing.title_hash == hashes.title
        && existing.body_hash == hashes.body
        && existing.state_hash == hashes.state
        && existing.labels_hash == hashes.labels
}

fn macro_task_is_echo(existing: &SyncLink, task: &MacroTask) -> bool {
    let hashes = task_field_hashes(task);
    github_issue_is_echo(existing, &hashes)
}

fn github_side_wins(existing: &SyncLink, gh_updated: &str) -> bool {
    match (&existing.gh_updated_at, &existing.macro_updated_at) {
        (Some(prev), Some(macro_ts)) => gh_updated >= prev.as_str() && gh_updated >= macro_ts.as_str(),
        (Some(prev), None) => gh_updated >= prev.as_str(),
        _ => true,
    }
}

fn macro_side_wins(existing: &SyncLink, macro_updated: &str) -> bool {
    match (&existing.macro_updated_at, &existing.gh_updated_at) {
        (Some(prev), Some(gh_ts)) => {
            macro_updated >= prev.as_str() && macro_updated >= gh_ts.as_str()
        }
        (Some(prev), None) => macro_updated >= prev.as_str(),
        _ => true,
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

fn single_project_id(cfg: &Config) -> Result<&str> {
    if cfg.projects.len() == 1 {
        Ok(&cfg.projects[0].project_id)
    } else {
        Err(SyncError::Other(
            "project_id required when multiple projects are configured".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn echo_when_all_field_hashes_match() {
        let link = SyncLink {
            project_id: "p".into(),
            gh_owner: "o".into(),
            gh_repo: "r".into(),
            gh_issue_number: 1,
            macro_task_id: "t".into(),
            title_hash: "a".into(),
            body_hash: "b".into(),
            state_hash: "c".into(),
            labels_hash: "d".into(),
            gh_updated_at: None,
            macro_updated_at: None,
        };
        let hashes = FieldHashes {
            title: "a".into(),
            body: "b".into(),
            state: "c".into(),
            labels: "d".into(),
        };
        assert!(github_issue_is_echo(&link, &hashes));
    }
}
