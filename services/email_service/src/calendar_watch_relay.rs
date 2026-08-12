//! Relay that lets locally running stacks receive real Google Calendar push
//! notifications without any public ingress of their own.
//!
//! Google requires a watch channel's address to be public HTTPS on a domain
//! verified in the Cloud project owning the OAuth client, which a laptop can
//! never satisfy. Instead, a local stack opens its channels with the DEV
//! deployment's already-verified webhook address and a per-instance token,
//! then connects OUT to dev and subscribes to deliveries for that token:
//!
//! ```text
//! Google ──POST──▶ dev /calendar/notifications
//!                    │ token == dev's own token → dev's normal flow
//!                    │ relay serving            → publish to Redis
//!                    ▼
//!            Redis pub/sub (replica-safe fan-out)
//!                    │
//!                    ▼ SSE (outbound connection from the laptop)
//!            local stack re-injects the ping into its own
//!            `handle_watch_notification` flow
//! ```
//!
//! The relay is inert unless explicitly configured: dev serves it by setting
//! `CALENDAR_WATCH_RELAY_SERVE=true` plus `CALENDAR_WATCH_RELAY_SECRET`;
//! local stacks consume it by setting `CALENDAR_WATCH_RELAY_URL` plus the
//! same secret. Production sets neither. Deliveries are content-free header
//! triples, subscribing requires the shared secret, and each subscriber only
//! sees pings for its own channel token, so a lapsed instance's strays die
//! here exactly like any other unknown-token notification.

use futures::{Stream, StreamExt};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// One relayed push notification: the meaningful subset of Google's
/// `x-goog-*` headers, in the wire shape shared by the Redis payload and the
/// SSE stream.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelayedWatchNotification {
    /// `x-goog-resource-state`: `sync`, `exists`, or `not_exists`.
    pub state: String,
    /// `x-goog-channel-id`.
    pub channel_id: String,
    /// `x-goog-resource-id`.
    pub resource_id: String,
}

/// Server-side relay configuration, present only on deployments that serve
/// relayed deliveries (dev).
pub struct WatchRelayServeConfig {
    /// Shared secret subscribers must present.
    pub secret: String,
}

/// Subscriber-side relay configuration, present only on stacks that consume
/// relayed deliveries (local).
pub struct WatchRelaySubscriberConfig {
    /// Base URL of the serving deployment, e.g. `https://email-service-dev.macro.com`.
    pub url: String,
    /// Shared secret presented when subscribing.
    pub secret: String,
}

fn read_env(name: &'static str) -> Option<String> {
    macro_env_var::maybe_read_env(name)
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

/// Read the serve-side configuration. Serving requires the explicit
/// `CALENDAR_WATCH_RELAY_SERVE=true` opt-in on top of the secret so that a
/// stack holding the secret merely to subscribe never serves.
pub fn watch_relay_serve_config() -> Option<WatchRelayServeConfig> {
    let serve = read_env("CALENDAR_WATCH_RELAY_SERVE")?;
    if !serve.eq_ignore_ascii_case("true") {
        return None;
    }
    let secret = read_env("CALENDAR_WATCH_RELAY_SECRET")?;
    Some(WatchRelayServeConfig { secret })
}

/// Read the subscriber-side configuration.
pub fn watch_relay_subscriber_config() -> Option<WatchRelaySubscriberConfig> {
    let url = read_env("CALENDAR_WATCH_RELAY_URL")?;
    let secret = read_env("CALENDAR_WATCH_RELAY_SECRET")?;
    Some(WatchRelaySubscriberConfig {
        url: url.trim_end_matches('/').to_owned(),
        secret,
    })
}

/// Compare two secrets without early exit on the first differing byte.
pub fn secrets_match(presented: &str, expected: &str) -> bool {
    Sha256::digest(presented.as_bytes()) == Sha256::digest(expected.as_bytes())
}

/// Redis channel carrying deliveries for one channel token. The token is
/// hashed so it never appears in `PUBSUB CHANNELS` output.
fn redis_channel(token: &str) -> String {
    format!(
        "calendar-watch-relay:{:x}",
        Sha256::digest(token.as_bytes())
    )
}

/// Redis pub/sub fan-out between the replica receiving a Google POST and the
/// replica holding the matching subscriber connection.
#[derive(Clone)]
pub struct RedisWatchRelayBus {
    client: redis::Client,
}

impl RedisWatchRelayBus {
    /// Construct the bus over the service's shared Redis.
    pub fn new(client: redis::Client) -> Self {
        Self { client }
    }

    /// Publish one delivery for `token`'s subscribers. Publishing to a token
    /// nobody is subscribed to is a no-op, which is exactly how strays from
    /// lapsed instances are dropped.
    pub async fn publish(
        &self,
        token: &str,
        notification: &RelayedWatchNotification,
    ) -> anyhow::Result<()> {
        let payload = serde_json::to_string(notification)?;
        let mut connection = self.client.get_multiplexed_async_connection().await?;
        redis::AsyncCommands::publish::<_, _, ()>(&mut connection, redis_channel(token), payload)
            .await?;
        Ok(())
    }

    /// Open a dedicated pub/sub connection streaming `token`'s deliveries.
    pub async fn subscribe(
        &self,
        token: &str,
    ) -> anyhow::Result<impl Stream<Item = RelayedWatchNotification> + Send + use<>> {
        let mut pubsub = self.client.get_async_pubsub().await?;
        pubsub.subscribe(redis_channel(token)).await?;
        Ok(pubsub.into_on_message().filter_map(|message| async move {
            let payload: String = message
                .get_payload()
                .inspect_err(
                    |error| tracing::warn!(error=?error, "unreadable relayed watch payload"),
                )
                .ok()?;
            serde_json::from_str(&payload)
                .inspect_err(
                    |error| tracing::warn!(error=?error, "undecodable relayed watch payload"),
                )
                .ok()
        }))
    }
}

/// Serve-side relay dependencies, carried by the API context on deployments
/// that serve relayed deliveries.
pub struct WatchRelayServer {
    /// Serve-side configuration.
    pub config: WatchRelayServeConfig,
    /// Redis fan-out bus.
    pub bus: RedisWatchRelayBus,
}

impl WatchRelayServer {
    /// Build the server from the environment; `None` when this deployment
    /// does not serve the relay.
    pub fn from_env(client: redis::Client) -> Option<Self> {
        let config = watch_relay_serve_config()?;
        Some(Self {
            config,
            bus: RedisWatchRelayBus::new(client),
        })
    }
}

/// Incremental parser extracting `data:` payloads from an SSE byte stream.
/// Comment lines (keep-alives) and other fields are ignored; multi-line data
/// is joined with `\n` per the SSE specification.
#[derive(Default)]
pub struct SseDataParser {
    buffer: String,
    data_lines: Vec<String>,
}

impl SseDataParser {
    /// Feed one chunk, returning every event payload it completed.
    pub fn push(&mut self, chunk: &str) -> Vec<String> {
        self.buffer.push_str(chunk);
        let mut completed = Vec::new();
        while let Some(newline) = self.buffer.find('\n') {
            let line: String = self.buffer.drain(..=newline).collect();
            let line = line.trim_end_matches(['\n', '\r']);
            if line.is_empty() {
                if !self.data_lines.is_empty() {
                    completed.push(self.data_lines.join("\n"));
                    self.data_lines.clear();
                }
            } else if let Some(data) = line.strip_prefix("data:") {
                self.data_lines
                    .push(data.strip_prefix(' ').unwrap_or(data).to_owned());
            }
        }
        completed
    }
}

#[cfg(test)]
mod test;
