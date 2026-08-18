//! Crost service-token channels REST API (`/api/v1/channels/*`).
//!
//! Contract: W2.8 server surface; consumed byte-for-byte by `crost-core`
//! `internal/macroclient` (W2.4).

#[cfg(feature = "inbound")]
pub mod auth;
#[cfg(feature = "inbound")]
pub mod error;
#[cfg(feature = "inbound")]
pub mod handlers;
#[cfg(feature = "inbound")]
pub mod models;
#[cfg(feature = "inbound")]
pub mod resolve;
#[cfg(feature = "inbound")]
pub mod router;

#[cfg(all(feature = "inbound", test))]
mod test;

#[cfg(feature = "inbound")]
pub use auth::ServiceApiToken;
#[cfg(feature = "inbound")]
pub use resolve::{DbUserResolver, UserResolver};
#[cfg(feature = "inbound")]
pub use router::{CrostChannelsRouterState, crost_channels_router};
