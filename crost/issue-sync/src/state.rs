use std::path::Path;
use std::sync::Mutex;

use rusqlite::{Connection, params};

use crate::error::Result;
use crate::models::SyncLink;

const MIGRATION: &str = include_str!("../migrations/001_init.sql");

pub struct StateStore {
    conn: Mutex<Connection>,
}

impl StateStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch(MIGRATION)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    pub fn in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(MIGRATION)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    pub fn upsert_link(&self, link: &SyncLink) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO sync_links (
                project_id, gh_owner, gh_repo, gh_issue_number, macro_task_id,
                title_hash, body_hash, state_hash, labels_hash,
                gh_updated_at, macro_updated_at, updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, datetime('now'))
            ON CONFLICT(project_id, gh_owner, gh_repo, gh_issue_number) DO UPDATE SET
                macro_task_id = excluded.macro_task_id,
                title_hash = excluded.title_hash,
                body_hash = excluded.body_hash,
                state_hash = excluded.state_hash,
                labels_hash = excluded.labels_hash,
                gh_updated_at = excluded.gh_updated_at,
                macro_updated_at = excluded.macro_updated_at,
                updated_at = datetime('now')",
            params![
                link.project_id,
                link.gh_owner,
                link.gh_repo,
                link.gh_issue_number as i64,
                link.macro_task_id,
                link.title_hash,
                link.body_hash,
                link.state_hash,
                link.labels_hash,
                link.gh_updated_at,
                link.macro_updated_at,
            ],
        )?;
        Ok(())
    }

    pub fn link_by_issue(
        &self,
        project_id: &str,
        owner: &str,
        repo: &str,
        issue_number: u64,
    ) -> Result<Option<SyncLink>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT project_id, gh_owner, gh_repo, gh_issue_number, macro_task_id,
                    title_hash, body_hash, state_hash, labels_hash,
                    gh_updated_at, macro_updated_at
             FROM sync_links
             WHERE project_id = ?1 AND gh_owner = ?2 AND gh_repo = ?3 AND gh_issue_number = ?4",
        )?;
        let mut rows = stmt.query(params![project_id, owner, repo, issue_number as i64])?;
        if let Some(row) = rows.next()? {
            return Ok(Some(row_to_link(row)?));
        }
        Ok(None)
    }

    pub fn link_by_task(&self, project_id: &str, task_id: &str) -> Result<Option<SyncLink>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT project_id, gh_owner, gh_repo, gh_issue_number, macro_task_id,
                    title_hash, body_hash, state_hash, labels_hash,
                    gh_updated_at, macro_updated_at
             FROM sync_links
             WHERE project_id = ?1 AND macro_task_id = ?2",
        )?;
        let mut rows = stmt.query(params![project_id, task_id])?;
        if let Some(row) = rows.next()? {
            return Ok(Some(row_to_link(row)?));
        }
        Ok(None)
    }

    pub fn upsert_comment(
        &self,
        project_id: &str,
        owner: &str,
        repo: &str,
        gh_comment_id: u64,
        macro_comment_id: &str,
        body_hash: &str,
        gh_updated_at: Option<&str>,
        macro_updated_at: Option<&str>,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO sync_comments (
                project_id, gh_owner, gh_repo, gh_comment_id, macro_comment_id,
                body_hash, gh_updated_at, macro_updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
            ON CONFLICT(project_id, gh_owner, gh_repo, gh_comment_id) DO UPDATE SET
                macro_comment_id = excluded.macro_comment_id,
                body_hash = excluded.body_hash,
                gh_updated_at = excluded.gh_updated_at,
                macro_updated_at = excluded.macro_updated_at",
            params![
                project_id,
                owner,
                repo,
                gh_comment_id as i64,
                macro_comment_id,
                body_hash,
                gh_updated_at,
                macro_updated_at,
            ],
        )?;
        Ok(())
    }

    pub fn comment_by_gh(
        &self,
        project_id: &str,
        owner: &str,
        repo: &str,
        gh_comment_id: u64,
    ) -> Result<Option<(String, String)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT macro_comment_id, body_hash FROM sync_comments
             WHERE project_id = ?1 AND gh_owner = ?2 AND gh_repo = ?3 AND gh_comment_id = ?4",
        )?;
        let mut rows = stmt.query(params![project_id, owner, repo, gh_comment_id as i64])?;
        if let Some(row) = rows.next()? {
            return Ok(Some((row.get(0)?, row.get(1)?)));
        }
        Ok(None)
    }
}

fn row_to_link(row: &rusqlite::Row<'_>) -> rusqlite::Result<SyncLink> {
    Ok(SyncLink {
        project_id: row.get(0)?,
        gh_owner: row.get(1)?,
        gh_repo: row.get(2)?,
        gh_issue_number: row.get::<_, i64>(3)? as u64,
        macro_task_id: row.get(4)?,
        title_hash: row.get(5)?,
        body_hash: row.get(6)?,
        state_hash: row.get(7)?,
        labels_hash: row.get(8)?,
        gh_updated_at: row.get(9)?,
        macro_updated_at: row.get(10)?,
    })
}
