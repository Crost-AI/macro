CREATE TABLE IF NOT EXISTS sync_links (
    project_id TEXT NOT NULL,
    gh_owner TEXT NOT NULL,
    gh_repo TEXT NOT NULL,
    gh_issue_number INTEGER NOT NULL,
    macro_task_id TEXT NOT NULL,
    title_hash TEXT NOT NULL DEFAULT '',
    body_hash TEXT NOT NULL DEFAULT '',
    state_hash TEXT NOT NULL DEFAULT '',
    labels_hash TEXT NOT NULL DEFAULT '',
    gh_updated_at TEXT,
    macro_updated_at TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (project_id, gh_owner, gh_repo, gh_issue_number)
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_sync_links_macro_task
    ON sync_links (project_id, macro_task_id);

CREATE TABLE IF NOT EXISTS sync_comments (
    project_id TEXT NOT NULL,
    gh_owner TEXT NOT NULL,
    gh_repo TEXT NOT NULL,
    gh_comment_id INTEGER NOT NULL,
    macro_comment_id TEXT NOT NULL,
    body_hash TEXT NOT NULL DEFAULT '',
    gh_updated_at TEXT,
    macro_updated_at TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (project_id, gh_owner, gh_repo, gh_comment_id)
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_sync_comments_macro
    ON sync_comments (project_id, macro_comment_id);
