//! Signed HTTP delivery to `WEBHOOK_URL`.

#[cfg(test)]
mod test;

use hmac::{Hmac, Mac};
use reqwest::{Client, StatusCode, redirect::Policy};
use sha2::Sha256;
use std::time::Duration;

use crate::{config::Config, events::WebhookEnvelope};

type HmacSha256 = Hmac<Sha256>;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

/// HTTP client that posts signed Crost webhook payloads.
#[derive(Clone)]
pub struct DeliveryClient {
    http: Client,
    config: Config,
}

impl DeliveryClient {
    /// Build a client for the given configuration.
    pub fn new(config: Config) -> anyhow::Result<Self> {
        let http = Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .redirect(Policy::none())
            .build()?;
        Ok(Self { http, config })
    }

    /// POST the envelope and return whether the attempt succeeded.
    pub async fn deliver(&self, envelope: &WebhookEnvelope) -> DeliveryResult {
        let body = match serde_json::to_vec(envelope) {
            Ok(body) => body,
            Err(error) => {
                return DeliveryResult::PermanentFailure(format!(
                    "failed to serialize webhook body: {error}"
                ));
            }
        };

        let timestamp = chrono::Utc::now().timestamp().to_string();
        let signature = match sign(&self.config.webhook_secret, &timestamp, &body) {
            Ok(signature) => signature,
            Err(error) => {
                return DeliveryResult::PermanentFailure(format!(
                    "failed to sign webhook body: {error}"
                ));
            }
        };

        let response = match self
            .http
            .post(&self.config.webhook_url)
            .header("content-type", "application/json")
            .header("x-macro-event", &envelope.event_type)
            .header("x-macro-event-id", envelope.event_id.to_string())
            .header("x-macro-timestamp", &timestamp)
            .header("x-macro-signature", signature)
            .body(body)
            .send()
            .await
        {
            Ok(response) => response,
            Err(error) => {
                if error.is_timeout() || error.is_connect() || error.is_request() {
                    return DeliveryResult::RetryableFailure(error.to_string());
                }
                return DeliveryResult::PermanentFailure(error.to_string());
            }
        };

        let status = response.status();
        if status.is_success() {
            return DeliveryResult::Success;
        }
        if matches!(
            status,
            StatusCode::REQUEST_TIMEOUT | StatusCode::TOO_MANY_REQUESTS
        ) || status.is_server_error()
        {
            return DeliveryResult::RetryableFailure(format!("HTTP {status}"));
        }
        DeliveryResult::PermanentFailure(format!("HTTP {status}"))
    }
}

/// Outcome of one delivery attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeliveryResult {
    /// Endpoint accepted the payload.
    Success,
    /// Transient failure; outbox should retry with the same `event_id`.
    RetryableFailure(String),
    /// Non-retryable failure.
    PermanentFailure(String),
}

/// Compute `X-Macro-Signature` (`v1=<hex>`) for the given body bytes.
pub fn sign(secret: &str, timestamp: &str, body: &[u8]) -> anyhow::Result<String> {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes())?;
    mac.update(timestamp.as_bytes());
    mac.update(b".");
    mac.update(body);
    Ok(format!("v1={}", hex::encode(mac.finalize().into_bytes())))
}

/// Verify a Macro webhook signature (for tests and local tooling).
pub fn verify(secret: &str, timestamp: &str, body: &[u8], signature: &str) -> bool {
    sign(secret, timestamp, body)
        .map(|expected| expected == signature)
        .unwrap_or(false)
}
