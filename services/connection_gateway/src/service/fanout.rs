//! Republish this instance's entire inbound websocket traffic to Redis.
//!
//! Every accepted connection, every client frame (text or binary, unparsed),
//! and every disconnect is published on
//! [`connection_gateway_models::fanout::INBOUND_CHANNEL`], so backend services
//! can consume client traffic without the gateway knowing they exist. The
//! sync tier is the first consumer.
//!
//! Publishing is fire-and-forget by design: a publish failure is logged and
//! never fails the connection's own message handling.

use crate::model::connection::ConnectionContext;
use anyhow::{Context, Result};
use connection_gateway_models::fanout::{FromGateway, HEARTBEAT_INTERVAL_SECS, INBOUND_CHANNEL};
use macro_user_id::user_id::MacroUserIdStr;
use redis::AsyncCommands;

async fn publish(ctx: &ConnectionContext<'_>, message: &FromGateway) -> Result<()> {
    let payload = postcard::to_stdvec(message).context("failed to encode fanout message")?;
    let mut connection = ctx.api_context.get_multiplexed_async_connection()?;
    connection
        .publish::<&str, &[u8], ()>(INBOUND_CHANNEL, &payload)
        .await
        .context("failed to publish fanout message")?;
    Ok(())
}

/// Log-and-continue wrapper: fanout must never break connection handling.
async fn publish_best_effort(ctx: &ConnectionContext<'_>, message: &FromGateway) {
    publish(ctx, message)
        .await
        .inspect_err(|error| tracing::warn!(error=?error, "failed to publish fanout message"))
        .ok();
}

/// Announce an accepted, authenticated connection.
pub async fn connected(ctx: &ConnectionContext<'_>, user_id: &MacroUserIdStr<'static>) {
    publish_best_effort(
        ctx,
        &FromGateway::Connected {
            gateway: ctx.api_context.fanout_gateway_id.to_string(),
            conn: ctx.connection_id.to_string(),
            user_id: user_id.clone(),
        },
    )
    .await;
}

/// Forward one client frame, unparsed.
pub async fn frame(ctx: &ConnectionContext<'_>, text: bool, payload: Vec<u8>) {
    publish_best_effort(
        ctx,
        &FromGateway::Frame {
            gateway: ctx.api_context.fanout_gateway_id.to_string(),
            conn: ctx.connection_id.to_string(),
            text,
            payload,
        },
    )
    .await;
}

/// Announce a closed connection.
pub async fn disconnected(ctx: &ConnectionContext<'_>) {
    publish_best_effort(
        ctx,
        &FromGateway::Disconnected {
            gateway: ctx.api_context.fanout_gateway_id.to_string(),
            conn: ctx.connection_id.to_string(),
        },
    )
    .await;
}

/// Liveness beacon so consumers can drop state for dead gateway instances.
/// Spawned once at boot.
pub async fn heartbeat_loop(
    redis_connection: redis::aio::MultiplexedConnection,
    gateway_id: std::sync::Arc<str>,
) {
    let mut tick = tokio::time::interval(std::time::Duration::from_secs(HEARTBEAT_INTERVAL_SECS));
    loop {
        tick.tick().await;
        let message = FromGateway::Heartbeat {
            gateway: gateway_id.to_string(),
        };
        let Ok(payload) = postcard::to_stdvec(&message) else {
            continue;
        };
        let mut connection = redis_connection.clone();
        connection
            .publish::<&str, &[u8], ()>(INBOUND_CHANNEL, &payload)
            .await
            .inspect_err(|error| tracing::warn!(error=?error, "failed to publish fanout heartbeat"))
            .ok();
    }
}
