//! Shared vocabulary: identifiers, frames, inputs, and effects.
//!
//! [`ClientFrame`] and [`ServerFrame`] mirror the wire protocol's
//! `FromPeer`/`FromRemote` bebop unions one-to-one, so the runtime adapter is
//! a mechanical decode/encode with no translation logic.

use macro_user_id::user_id::MacroUserIdStr;

/// A connection attached to a document. Opaque to the machine; the runtime
/// interns its transport-level identity (e.g. gateway id + conn id) to one of
/// these.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ConnId(pub u64);

/// A document id as it appears in envelopes and storage keys.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DocId(pub String);

impl DocId {
    /// Borrow the raw id.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// What an attached connection is allowed to do, resolved by the edge before
/// the machine ever sees the connection. The machine holds no tokens and no
/// access levels — only the answers it needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Caps {
    /// May this connection submit document updates?
    pub can_edit: bool,
    /// The authenticated user, when known (anonymous share links have none).
    /// Used only for peer-id attribution.
    pub user_id: Option<MacroUserIdStr<'static>>,
}

/// Why the machine asked the runtime to close a connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseReason {
    /// A frame arrived for a connection the machine doesn't know.
    NotAttached,
    /// The connection sent a payload the replica rejected.
    Protocol,
    /// The document failed to load; nothing can be served.
    LoadFailed,
}

/// An opaque, replica-encoded full-document snapshot (in production: a raw
/// Loro snapshot export). The machine never looks inside.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawSnapshot(pub Vec<u8>);

/// An opaque, replica-encoded update (in production: a raw Loro update).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawUpdate(pub Vec<u8>);

/// An opaque position marker for [`ClientFrame::RequestSince`] (in
/// production: a Loro version vector). Echoed back verbatim — clients
/// correlate on exact bytes, and re-encoding is not byte-stable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawCursor(pub Vec<u8>);

/// An opaque presence/awareness payload (in production: a Loro ephemeral
/// store delta).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawPresence(pub Vec<u8>);

macro_rules! byte_newtype {
    ($name:ident) => {
        impl $name {
            /// Borrow the raw bytes.
            pub fn as_slice(&self) -> &[u8] {
                &self.0
            }
        }
        impl From<Vec<u8>> for $name {
            fn from(bytes: Vec<u8>) -> Self {
                Self(bytes)
            }
        }
        impl From<&[u8]> for $name {
            fn from(bytes: &[u8]) -> Self {
                Self(bytes.to_vec())
            }
        }
    };
}
byte_newtype!(RawSnapshot);
byte_newtype!(RawUpdate);
byte_newtype!(RawCursor);
byte_newtype!(RawPresence);

/// Identifies one scheduled timer. Meaning is machine-internal; the runtime
/// just echoes it back in [`Input::TimerFired`] when the delay elapses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TimerToken(pub u64);

/// Identifies one in-flight persistence request, echoed back in its
/// completion input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PersistToken(pub u64);

/// A decoded client sync message (the inner `FromPeer` union).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientFrame {
    /// A batch of CRDT updates with a client-chosen id, acked after the batch
    /// is durably stored.
    Update {
        /// The opaque update payloads, applied in order.
        updates: Vec<RawUpdate>,
        /// The client's correlation id for the ack.
        id: String,
    },
    /// An ephemeral presence/awareness payload.
    Presence {
        /// The opaque presence payload.
        payload: RawPresence,
    },
    /// Request every update the caller is missing, given their cursor (an
    /// opaque version vector).
    RequestSince {
        /// The caller's cursor, echoed back verbatim in the reply.
        cursor: RawCursor,
    },
    /// Request a full snapshot.
    RequestSnapshot,
    /// Bind a CRDT peer id to this connection (for blame/user attribution).
    RegisterPeer {
        /// The peer id the client will author updates as.
        peer_id: u64,
    },
}

/// A server sync message to one or more clients (the inner `FromRemote`
/// union).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerFrame {
    /// The first message after attach: full document state plus current
    /// presence payloads.
    InitialSync {
        /// A full snapshot of the document.
        snapshot: RawSnapshot,
        /// Every attached connection's latest presence payload.
        // TODO(pass 2): replace with a real ephemeral-store encoding (loro
        // `EphemeralStore::encode_all`) behind a `Presence` trait.
        presence: Vec<RawPresence>,
    },
    /// One CRDT update from another peer.
    Update {
        /// The opaque update payload.
        update: RawUpdate,
    },
    /// A presence payload from another peer.
    Presence {
        /// The opaque presence payload.
        payload: RawPresence,
    },
    /// Presence removal for peers whose connection went away.
    // TODO(pass 2): fold into the real presence encoding; the wire protocol
    // expresses removal as an ephemeral-store delta, not a distinct frame.
    PresenceLeft {
        /// The peer ids that left.
        peer_ids: Vec<u64>,
    },
    /// A full snapshot, answering [`ClientFrame::RequestSnapshot`].
    Snapshot {
        /// The snapshot payload.
        snapshot: RawSnapshot,
    },
    /// Durable-storage acknowledgement of [`ClientFrame::Update`].
    Ack {
        /// The client's correlation id.
        id: String,
    },
    /// Everything the caller was missing, answering
    /// [`ClientFrame::RequestSince`].
    Since {
        /// The combined update payload.
        update: RawUpdate,
        /// The caller's cursor bytes, echoed verbatim (clients correlate by
        /// exact match; re-encoding is not byte-stable).
        cursor: RawCursor,
    },
}

/// Session transitions the rest of the product cares about (interaction
/// reporting, search reindex triggers).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lifecycle {
    /// Peer count went 0 → 1.
    FirstJoin,
    /// A compaction persisted content changes.
    Edited,
    /// Peer count went 1 → 0.
    LastLeave,
}

/// One "who last touched this node" record for the blame store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlameEvent {
    /// The Lexical node id that changed.
    pub node_id: String,
    /// The CRDT peer id that changed it.
    pub peer_id: u64,
}

/// Everything a [`crate::machine::DocMachine`] reacts to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Input {
    /// A connection attached (edge already resolved its capabilities).
    PeerAttached {
        /// The attaching connection.
        conn: ConnId,
        /// What it may do.
        caps: Caps,
    },
    /// A connection detached (unsubscribe, disconnect, or edge death).
    PeerDetached {
        /// The detaching connection.
        conn: ConnId,
    },
    /// A sync frame from an attached connection.
    Frame {
        /// The sending connection.
        conn: ConnId,
        /// The decoded message.
        frame: ClientFrame,
    },
    /// A previously scheduled timer elapsed.
    TimerFired {
        /// The token from the corresponding [`Effect::ScheduleTimer`].
        token: TimerToken,
    },
    /// Completion of [`Effect::Load`]: the stored snapshot, or `None` when the
    /// document has never been persisted.
    Loaded {
        /// The snapshot bytes, if any exist.
        snapshot: Option<RawSnapshot>,
    },
    /// Completion of [`Effect::Load`]: the store failed.
    LoadFailed {
        /// A human-readable reason (for logs; never sent to clients).
        error: String,
    },
    /// Completion of [`Effect::PersistOps`]: ops are durable through the given
    /// sequence number.
    OpsPersisted {
        /// The originating request.
        token: PersistToken,
        /// Ops with `seq <= through_seq` are durable.
        through_seq: u64,
    },
    /// Completion of [`Effect::PersistSnapshot`].
    SnapshotPersisted {
        /// The originating request.
        token: PersistToken,
    },
    /// A persistence request failed; the machine schedules a retry.
    PersistFailed {
        /// The failed request.
        token: PersistToken,
    },
}

/// Everything a [`crate::machine::DocMachine`] can ask the world to do.
///
/// Request-shaped effects ([`Effect::Load`], [`Effect::PersistOps`],
/// [`Effect::PersistSnapshot`]) each have completion inputs; the rest are
/// fire-and-forget.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Deliver a frame to one connection.
    Send {
        /// The target connection.
        conn: ConnId,
        /// The frame to deliver.
        frame: ServerFrame,
    },
    /// Deliver a frame to every attached connection except one (the sender).
    Broadcast {
        /// The connection to skip.
        except: ConnId,
        /// The frame to deliver.
        frame: ServerFrame,
    },
    /// Close a connection.
    Close {
        /// The target connection.
        conn: ConnId,
        /// Why.
        reason: CloseReason,
    },
    /// Ask for [`Input::TimerFired`] with this token after the delay.
    ScheduleTimer {
        /// Echoed back on firing.
        token: TimerToken,
        /// The delay in milliseconds.
        after_ms: u64,
    },
    /// Fetch the stored snapshot (answered by [`Input::Loaded`] /
    /// [`Input::LoadFailed`]).
    Load,
    /// Durably append ops (answered by [`Input::OpsPersisted`] /
    /// [`Input::PersistFailed`]).
    PersistOps {
        /// Echoed back in the completion.
        token: PersistToken,
        /// `(seq, payload)` pairs, contiguous and ascending.
        ops: Vec<(u64, RawUpdate)>,
        /// The highest seq in `ops`.
        through_seq: u64,
    },
    /// Durably store a full snapshot covering ops through `through_seq`
    /// (answered by [`Input::SnapshotPersisted`] / [`Input::PersistFailed`]).
    /// The store may truncate ops with `seq <= through_seq` once it commits.
    // TODO(pass 2): carry a cheap replica fork instead of exported bytes so
    // serialization can happen off the driving thread.
    PersistSnapshot {
        /// Echoed back in the completion.
        token: PersistToken,
        /// The exported snapshot.
        snapshot: RawSnapshot,
        /// Every op with `seq <= through_seq` is contained in the snapshot.
        through_seq: u64,
    },
    /// Record blame rows for an applied update.
    RecordBlame {
        /// The rows to record.
        events: Vec<BlameEvent>,
    },
    /// Record a peer-id → user binding for attribution.
    RecordPeerMapping {
        /// The CRDT peer id.
        peer_id: u64,
        /// The authenticated user it belongs to.
        user_id: MacroUserIdStr<'static>,
    },
    /// Announce a session transition to the rest of the product.
    Lifecycle {
        /// The transition.
        event: Lifecycle,
    },
    /// The machine is finished: no peers, nothing dirty. The owner should
    /// drop it.
    Evict,
}
