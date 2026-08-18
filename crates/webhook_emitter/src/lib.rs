#![deny(missing_docs)]
#![doc = include_str!("../README.md")]

/// Kafka bridge translating broker topics into Crost webhook events.
#[cfg(feature = "bridge")]
pub mod bridge;
/// Environment-backed emitter configuration.
pub mod config;
/// Delivery HTTP client and signing helpers.
#[cfg(feature = "worker")]
pub mod delivery;
/// Event type constants and payload envelope.
pub mod events;
/// Public enqueue helpers.
pub mod emit;
/// Postgres outbox persistence.
pub mod outbox;
/// Background worker draining the outbox.
#[cfg(feature = "worker")]
pub mod worker;
