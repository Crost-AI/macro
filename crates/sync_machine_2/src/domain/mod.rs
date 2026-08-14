//! Domain layer: the document actor, its supervisor, and the ports they
//! drive. Outbound adapters (Postgres store, gateway sink) arrive in pass 2;
//! pass 1 exercises everything through test mocks.

pub mod actor;
pub mod ports;
pub mod supervisor;
