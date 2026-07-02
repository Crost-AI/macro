//! Newtypes over the bare strings at the API/domain surface, so a document id
//! can't be confused with a peer id, user id, or token.

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct DocumentId(pub String);

impl DocumentId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for DocumentId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for DocumentId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl From<String> for DocumentId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for DocumentId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

/// An encoded JWT, distinct from the decoded [`crate::auth::AuthToken`].
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(transparent)]
pub struct SyncServiceJWT(pub String);

impl SyncServiceJWT {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
