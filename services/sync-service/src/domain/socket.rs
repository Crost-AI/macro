use bebop::Record;

use crate::domain::ports::SyncServiceError;

/// A peer websocket, abstracted from the underlying transport so the protocol
/// logic is testable and not tied to `worker::WebSocket`.
pub trait Socket {
    /// Stable identity (the ws tag), used to skip the sender during broadcast
    /// and to look up per-socket metadata.
    fn id(&self) -> &str;
    /// Serialize and send one bebop message to this peer.
    fn send<'m, T: Record<'m>>(&self, msg: T) -> Result<(), SyncServiceError>;
}
