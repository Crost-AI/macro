//! `WEBHOOK_URL` / `WEBHOOK_SECRET` configuration.

/// Active webhook emitter settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Destination URL for signed webhook POSTs.
    pub webhook_url: String,
    /// HMAC signing secret shared with the broker.
    pub webhook_secret: String,
}

impl Config {
    /// Load configuration from `WEBHOOK_URL` and `WEBHOOK_SECRET`.
    ///
    /// Returns `None` when either variable is unset or empty (emitter disabled).
    pub fn from_env() -> Option<Self> {
        let webhook_url = std::env::var("WEBHOOK_URL").ok()?;
        let webhook_secret = std::env::var("WEBHOOK_SECRET").ok()?;
        if webhook_url.trim().is_empty() || webhook_secret.is_empty() {
            return None;
        }
        Some(Self {
            webhook_url,
            webhook_secret,
        })
    }
}
