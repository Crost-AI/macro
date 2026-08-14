//! Async ports the actor calls directly (compare: the pure machine emits
//! effects and never awaits anything).

use sync_machine::model::{
    BlameEvent, CloseReason, ConnId, DocId, Lifecycle, RawSnapshot, RawUpdate, ServerFrame,
};

/// Persistence for one document's snapshot and op log.
pub trait DocStore: Send + Sync + 'static {
    /// The stored snapshot, or `None` for a never-persisted document.
    fn load(
        &self,
        doc: &DocId,
    ) -> impl Future<Output = Result<Option<RawSnapshot>, StoreError>> + Send;

    /// Durably append `(seq, update)` pairs.
    fn append_ops(
        &self,
        doc: &DocId,
        ops: &[(u64, RawUpdate)],
    ) -> impl Future<Output = Result<(), StoreError>> + Send;

    /// Durably store a snapshot covering ops through `through_seq`.
    fn store_snapshot(
        &self,
        doc: &DocId,
        snapshot: RawSnapshot,
        through_seq: u64,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;
}

/// A store failure. The actor retries with a delay.
#[derive(Debug, thiserror::Error)]
#[error("store error: {0}")]
pub struct StoreError(pub String);

/// Delivery back to clients. Both methods are best-effort.
pub trait ClientSink: Send + Sync + 'static {
    /// Deliver one frame to one connection.
    fn send(&self, conn: ConnId, frame: ServerFrame) -> impl Future<Output = ()> + Send;
    /// Close a connection.
    fn close(&self, conn: ConnId, reason: CloseReason) -> impl Future<Output = ()> + Send;
}

/// Everything the rest of the product observes about a document session.
pub trait DocEvents: Send + Sync + 'static {
    /// A session transition (first join, edited, last leave).
    fn lifecycle(&self, doc: &DocId, event: Lifecycle) -> impl Future<Output = ()> + Send;
    /// Blame rows for an applied update.
    fn blame(&self, doc: &DocId, events: Vec<BlameEvent>) -> impl Future<Output = ()> + Send;
    /// A peer-id → user binding.
    fn peer_mapping(
        &self,
        doc: &DocId,
        peer_id: u64,
        user_id: String,
    ) -> impl Future<Output = ()> + Send;
}
