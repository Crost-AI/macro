use thiserror::Error;

#[derive(Debug, Error)]
pub enum SyncError {
    #[error("configuration: {0}")]
    Config(String),
    #[error("database: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("http: {0}")]
    Http(#[from] reqwest::Error),
    #[error("github api: {status} {body}")]
    GitHub { status: u16, body: String },
    #[error("macro api: {status} {body}")]
    Macro { status: u16, body: String },
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("loop protection: skipped echo from {origin}")]
    Echo { origin: String },
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, SyncError>;
