//! Google Calendar push notification webhook.
//!
//! Google delivers content-free notifications here for every calendar with
//! an open `events.watch` channel. The handler verifies the shared channel
//! token minted at channel creation, then re-arms the watched inbox's sync
//! job; the regular poll remains the backstop for dropped notifications, so
//! unmatched or failed notifications are acknowledged rather than retried.
//!
//! Deployments serving the watch relay (dev) additionally forward
//! notifications whose token is not their own to the Redis fan-out, where a
//! locally running stack may be subscribed to that token — see
//! [`email_service::calendar_watch_relay`] for the topology. `/relay/subscribe`
//! is the outbound-connection end of that relay.

use std::time::Duration;

use axum::Router;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use futures::StreamExt;

use crate::api::context::ApiContext;
use email_service::calendar_watch_relay::{RelayedWatchNotification, secrets_match};
use email_service::pubsub::context::calendar_watch_config;

/// Build the unauthenticated watch notification router.
pub fn router() -> Router<ApiContext> {
    Router::new()
        .route("/notifications", post(handler))
        .route("/relay/subscribe", get(subscribe_handler))
}

fn header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|value| value.to_str().ok())
}

#[tracing::instrument(skip_all)]
async fn handler(State(ctx): State<ApiContext>, headers: HeaderMap) -> StatusCode {
    let config = calendar_watch_config();
    let relay = ctx.watch_relay.as_deref();
    if config.is_none() && relay.is_none() {
        return StatusCode::NOT_FOUND;
    }
    let token = header(&headers, "x-goog-channel-token");
    if let Some(config) = &config
        && token == Some(config.token.as_str())
    {
        if header(&headers, "x-goog-resource-state") == Some("sync") {
            return StatusCode::OK;
        }
        let (Some(channel_id), Some(resource_id)) = (
            header(&headers, "x-goog-channel-id"),
            header(&headers, "x-goog-resource-id"),
        ) else {
            return StatusCode::BAD_REQUEST;
        };
        match ctx
            .calendar_service
            .handle_watch_notification(channel_id, resource_id)
            .await
        {
            Ok(matched) => {
                if !matched {
                    tracing::debug!(
                        channel_id,
                        "calendar notification matched no active channel"
                    );
                }
            }
            Err(error) => {
                tracing::warn!(error = ?error, channel_id, "failed to apply calendar watch notification");
            }
        }
        return StatusCode::OK;
    }
    if let Some(relay) = relay {
        let Some(token) = token else {
            return StatusCode::FORBIDDEN;
        };
        let (Some(state), Some(channel_id), Some(resource_id)) = (
            header(&headers, "x-goog-resource-state"),
            header(&headers, "x-goog-channel-id"),
            header(&headers, "x-goog-resource-id"),
        ) else {
            return StatusCode::BAD_REQUEST;
        };
        let notification = RelayedWatchNotification {
            state: state.to_owned(),
            channel_id: channel_id.to_owned(),
            resource_id: resource_id.to_owned(),
        };
        // Publishing to a token nobody subscribes to is a no-op, which is
        // how strays from torn-down local stacks are dropped. Failures are
        // acknowledged like every other notification: the subscriber's poll
        // backstop covers the gap.
        relay
            .bus
            .publish(token, &notification)
            .await
            .inspect_err(|error| {
                tracing::warn!(error = ?error, channel_id, "failed to relay calendar watch notification");
            })
            .ok();
        return StatusCode::OK;
    }
    StatusCode::FORBIDDEN
}

/// Stream relayed watch notifications for one channel token over SSE.
///
/// Subscribers authenticate with the shared relay secret; the stream then
/// carries only deliveries addressed to the presented token, so one local
/// stack can never observe another's notifications.
#[tracing::instrument(skip_all)]
async fn subscribe_handler(State(ctx): State<ApiContext>, headers: HeaderMap) -> Response {
    let Some(relay) = ctx.watch_relay.as_deref() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Some(secret) = header(&headers, "x-relay-secret") else {
        return StatusCode::FORBIDDEN.into_response();
    };
    if !secrets_match(secret, &relay.config.secret) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let Some(token) = header(&headers, "x-relay-token") else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    match relay.bus.subscribe(token).await {
        Ok(stream) => {
            Sse::new(stream.map(|notification| Event::default().json_data(&notification)))
                .keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
                .into_response()
        }
        Err(error) => {
            tracing::error!(error = ?error, "failed to open a relay subscription");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}
