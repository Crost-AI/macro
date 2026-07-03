/// An encoded JWT, distinct from the decoded
/// [`crate::domain::permissions::AuthToken`].
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(transparent)]
pub struct SyncServiceJWT(pub String);

impl SyncServiceJWT {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
