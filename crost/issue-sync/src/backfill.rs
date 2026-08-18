use crate::error::Result;
use crate::github_api::GitHubClient;
use crate::sync::SyncEngine;

pub async fn backfill_project(engine: &SyncEngine, project_id: &str) -> Result<usize> {
    let link = engine
        .cfg
        .project_link(project_id)
        .ok_or_else(|| crate::error::SyncError::Other(format!("unknown project {project_id}")))?;
    let gh = GitHubClient::new(&engine.cfg)?;
    let issues = gh
        .list_open_issues(&link.gh_owner, &link.gh_repo)
        .await?;
    let mut imported = 0usize;
    for issue in issues {
        if engine
            .store
            .link_by_issue(&link.project_id, &link.gh_owner, &link.gh_repo, issue.number)?
            .is_some()
        {
            continue;
        }
        engine.sync_from_github(link, &issue).await?;
        imported += 1;
    }
    Ok(imported)
}

pub async fn backfill_all(engine: &SyncEngine) -> Result<usize> {
    let mut total = 0usize;
    for link in &engine.cfg.projects {
        total += backfill_project(engine, &link.project_id).await?;
    }
    Ok(total)
}
