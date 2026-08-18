use std::sync::Arc;

use axum::{
    Router,
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::post,
};
use hmac::{Hmac, Mac};
use sha2::Sha256;

use crate::models::{GitHubIssuesWebhook, MacroWebhookEvent};
use crate::sync::SyncEngine;

type HmacSha256 = Hmac<Sha256>;

#[derive(Clone)]
pub struct WebhookState {
    pub engine: Arc<SyncEngine>,
    pub github_secret: Option<String>,
    pub macro_secret: Option<String>,
}

pub fn router(state: WebhookState) -> Router {
    Router::new()
        .route("/webhooks/github", post(github_handler))
        .route("/webhooks/macro", post(macro_handler))
        .with_state(state)
}

async fn github_handler(
    State(state): State<WebhookState>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    if let Some(secret) = &state.github_secret {
        if !verify_github_signature(secret, &headers, &body) {
            return StatusCode::UNAUTHORIZED;
        }
    }
    let event = headers
        .get("x-github-event")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if event != "issues" && event != "issue_comment" {
        return StatusCode::ACCEPTED;
    }
    match serde_json::from_slice::<GitHubIssuesWebhook>(&body) {
        Ok(payload) => match state.engine.handle_github_webhook(payload).await {
            Ok(()) => StatusCode::OK,
            Err(err) => {
                tracing::warn!(error = %err, "github webhook sync failed");
                StatusCode::INTERNAL_SERVER_ERROR
            }
        },
        Err(err) => {
            tracing::warn!(error = %err, "invalid github webhook payload");
            StatusCode::BAD_REQUEST
        }
    }
}

async fn macro_handler(
    State(state): State<WebhookState>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    if let Some(secret) = &state.macro_secret {
        if !verify_macro_signature(secret, &headers, &body) {
            return StatusCode::UNAUTHORIZED;
        }
    }
    match serde_json::from_slice::<MacroWebhookEvent>(&body) {
        Ok(event) => match state.engine.handle_macro_webhook(event).await {
            Ok(()) => StatusCode::OK,
            Err(err) => {
                tracing::warn!(error = %err, "macro webhook sync failed");
                StatusCode::INTERNAL_SERVER_ERROR
            }
        },
        Err(err) => {
            tracing::warn!(error = %err, "invalid macro webhook payload");
            StatusCode::BAD_REQUEST
        }
    }
}

fn verify_github_signature(secret: &str, headers: &HeaderMap, body: &[u8]) -> bool {
    let Some(sig) = headers
        .get("x-hub-signature-256")
        .and_then(|v| v.to_str().ok())
    else {
        return false;
    };
    let Some(hex) = sig.strip_prefix("sha256=") else {
        return false;
    };
    verify_hmac(secret, body, hex)
}

/// W2.7: `X-Macro-Signature: v1=<hex>` over `{X-Macro-Timestamp}.{body}`.
fn verify_macro_signature(secret: &str, headers: &HeaderMap, body: &[u8]) -> bool {
    let Some(sig_header) = headers
        .get("x-macro-signature")
        .and_then(|v| v.to_str().ok())
    else {
        return false;
    };
    let Some(hex) = sig_header.strip_prefix("v1=") else {
        return false;
    };
    let Some(timestamp) = headers
        .get("x-macro-timestamp")
        .and_then(|v| v.to_str().ok())
    else {
        return false;
    };
    let signed_payload = format!("{timestamp}.{}", String::from_utf8_lossy(body));
    verify_hmac(secret, signed_payload.as_bytes(), hex)
}

fn verify_hmac(secret: &str, body: &[u8], expected_hex: &str) -> bool {
    let Ok(mut mac) = HmacSha256::new_from_slice(secret.as_bytes()) else {
        return false;
    };
    mac.update(body);
    let Ok(expected) = hex::decode(expected_hex) else {
        return false;
    };
    mac.verify_slice(&expected).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn macro_signature_matches_w27_format() {
        let secret = "test-secret";
        let body = br#"{"event_type":"task.comment"}"#;
        let timestamp = "1700000000";
        let payload = format!("{timestamp}.{}", String::from_utf8_lossy(body));
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(payload.as_bytes());
        let sig = hex::encode(mac.finalize().into_bytes());

        let mut headers = HeaderMap::new();
        headers.insert("x-macro-timestamp", timestamp.parse().unwrap());
        headers.insert("x-macro-signature", format!("v1={sig}").parse().unwrap());

        assert!(verify_macro_signature(secret, &headers, body));
    }
}
