use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode};
use serde::{Deserialize, Serialize};
use tracing::error;

use crate::{
    constants::header_names,
    error::ResultExt,
    ids::{DocumentId, SyncServiceJWT},
    secrets::Secrets,
};

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone, Copy, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
#[derive(Default, enum_map::Enum)]
pub enum AccessLevel {
    /// User can view the document
    #[default]
    View = 0,
    /// User can comment on the document
    /// In this context, this is the same thing as [AccessLevel::View]
    Comment = 1,
    /// User can edit the document
    Edit = 2,
    /// User is the owner of the document
    Owner = 3,
    /// Internal communication
    Admin = 4,
}

impl AccessLevel {
    pub fn can_edit(&self) -> bool {
        self >= &AccessLevel::Comment
    }
}

#[derive(Deserialize, Debug)]
pub struct AuthToken {
    pub user_id: Option<String>,
    document_id: DocumentId,
    pub access_level: AccessLevel,
}

impl AuthToken {
    pub fn has_permission(&self, al: &AccessLevel) -> bool {
        if self.access_level < *al {
            error!(
                "Current permission level [{:?}] is not enough for [{:?}]",
                self.access_level, al
            );
            return false;
        }
        true
    }
    pub fn has_document_id_access(&self, document_id: &DocumentId) -> bool {
        if !(self.document_id == *document_id || matches!(self.access_level, AccessLevel::Admin)) {
            error!(
                "Don't have permission for document: [{:?}]
Auth'd document [{:?}]
access level [{:?}]",
                document_id, self.document_id, self.access_level
            );
            return false;
        }
        true
    }
}

#[derive(Deserialize, Debug)]
pub struct WebsocketQueryParams {
    pub token: SyncServiceJWT,
}

pub fn decode_jwt(token: &SyncServiceJWT, env: &worker::Env) -> worker::Result<AuthToken> {
    let secrets = Secrets::from(env);

    let validation = Validation::new(Algorithm::HS256);
    let key = DecodingKey::from_secret(secrets.document_permissions_secret.to_string().as_bytes());
    let claims = decode::<AuthToken>(token.as_str(), &key, &validation)
        .context("failed to decode `AuthToken`")?
        .claims;
    Ok(claims)
}

/// Extract the bearer token from the `Authorization` header (no validation —
/// pair with [`decode_jwt`]). `None` if the header is absent or not `Bearer …`.
pub fn extract_jwt_from_headers(headers: &axum::http::HeaderMap) -> Option<SyncServiceJWT> {
    headers
        .get(header_names::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|header| header.strip_prefix("Bearer "))
        .map(|token| SyncServiceJWT(token.to_string()))
}

/// True when the request carries the shared internal API key. Internal services
/// (e.g. the document-copy flow) use it to authenticate as [`AccessLevel::Admin`]
/// without a user JWT.
pub fn internal_request(headers: &axum::http::HeaderMap, env: &worker::Env) -> bool {
    headers
        .get(header_names::MACRO_INTERNAL_AUTH_KEY_HEADER_KEY)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|key| Secrets::from(env).internal_api_secret == key)
}

#[cfg(test)]
mod test {
    use super::*;
    #[test]
    #[allow(clippy::nonminimal_bool, reason = "demonstrate ordering")]
    fn orderable() {
        let view = AccessLevel::View;
        let comment = AccessLevel::Comment;
        let edit = AccessLevel::Edit;
        let owner = AccessLevel::Owner;

        assert!(view < comment);
        assert!(view <= view);
        assert!(!(view < view));
        assert!(edit <= owner);
        assert!(view <= edit);
        assert!(view < owner);
        assert!(!(owner < view));
    }
}
