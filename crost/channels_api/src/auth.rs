use axum::{
    extract::Request,
    http::{StatusCode, header::AUTHORIZATION},
    middleware::Next,
    response::Response,
};
use std::sync::Arc;

/// Expected `Authorization: Bearer <token>` value for Crost service calls.
#[derive(Clone)]
pub struct ServiceApiToken(Arc<str>);

impl ServiceApiToken {
    /// Load from `SERVICE_API_TOKEN`. Panics at startup when unset (misconfiguration).
    pub fn from_env() -> Self {
        let token = std::env::var("SERVICE_API_TOKEN")
            .expect("SERVICE_API_TOKEN must be set for Crost channels API");
        Self(Arc::from(token))
    }

    /// Construct from an explicit token (tests).
    pub fn new(token: impl Into<Arc<str>>) -> Self {
        Self(token.into())
    }
}

/// Axum middleware: require `Authorization: Bearer` matching [`ServiceApiToken`].
pub async fn require_service_token(
    axum::extract::State(expected): axum::extract::State<ServiceApiToken>,
    request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let authorized = request
        .headers()
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .is_some_and(|provided| provided == expected.0.as_ref());
    if authorized {
        Ok(next.run(request).await)
    } else {
        Err(StatusCode::UNAUTHORIZED)
    }
}
