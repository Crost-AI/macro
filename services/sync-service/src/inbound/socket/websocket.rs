use bebop::{Record, SliceWrapper};
use loro::{VersionVector, awareness::EphemeralStore};
use tracing::trace;

use crate::{
    domain::{document_id::DocumentId, ports::SyncServiceError, state::DocumentState},
    error::ResultExt,
    generated::schema::{FromPeer, FromRemote},
    inbound::{
        socket::WorkerSocket,
        sync_service::{SyncServiceImpl, Wsm},
    },
    outbound::storage::SessionStorage,
};

/// Sends the initial sync message to the client over the websocket
/// The initial sync message contains the snapshot of the current state of the document
pub fn send_initial_sync(
    socket: &WorkerSocket,
    snapshot: &[u8],
    awareness: &[u8],
) -> Result<(), SyncServiceError> {
    socket.send(FromRemote::RemoteInitialSync {
        snapshot: SliceWrapper::Raw(snapshot),
        awareness: SliceWrapper::Raw(awareness),
    })
}

pub fn broadcast_awareness(
    from: &WorkerSocket,
    sockets: &[WorkerSocket],
    awareness: &[u8],
) -> Result<(), SyncServiceError> {
    for s in sockets.iter().filter(|s| s.id() != from.id()) {
        // A dead peer socket must not abort delivery to the remaining peers.
        if let Err(e) = s.send(FromRemote::RemoteAwareness {
            awareness: SliceWrapper::Raw(awareness),
        }) {
            tracing::warn!(error = ?e, "failed to send awareness to a peer; continuing");
        }
    }

    Ok(())
}

// Max receiving websocket message is 1Mb
const MAX_MESSAGE_SIZE: usize = 1000 * 1000;

#[allow(
    clippy::too_many_arguments,
    reason = "lots of args lets us avoid having multiple mutable refs to same object"
)]
pub async fn process_message(
    sender: &WorkerSocket,
    sockets: &[WorkerSocket],
    document_id: &DocumentId,
    document_state: &DocumentState,
    session_storage: &SessionStorage,
    awareness: &EphemeralStore,
    message: Vec<u8>,
    dss: &SyncServiceImpl,
) -> Result<(), SyncServiceError> {
    if message.len() > MAX_MESSAGE_SIZE {
        tracing::warn!("received message might be too large {}", message.len());
    }

    let message: FromPeer = FromPeer::deserialize(message.as_slice()).context(format!(
        "failed to deserialize message, message length {}",
        message.len()
    ))?;

    trace!(
        message = tracing::field::display(&message),
        "process websocket message"
    );
    match message {
        // Handle peer id registration
        // This registers a peer id to the owner of the current websocket
        FromPeer::PeerRegisterId { peerid } => {
            Wsm::new(dss, sender.id().to_string())
                .add_new_peerid(peerid, document_id)
                .await?;
        }
        // Handle an incoming update from a peer
        // Should extract binary update and broadcast it to all other connected peers
        // Should also store the update in the operation log to be applied to the remote doc
        FromPeer::PeerUpdate { updates, id } => {
            if !Wsm::new(dss, sender.id().to_string()).can_edit().await? {
                tracing::warn!("received update from peer without edit permission");
                return Ok(());
            }

            for update in &updates {
                session_storage
                    .append_pending_operation(update, document_state)
                    .await?;
            }

            // ACK the sender before broadcasting: the batch is durably
            // stored at this point, and a failed broadcast to some other
            // peer must not block the ack.
            sender.send(FromRemote::RemoteUpdateAck { id })?;

            for update in &updates {
                // broadcast each update to other peers
                for s in sockets.iter().filter(|s| s.id() != sender.id()) {
                    // A dead peer socket must not abort delivery to the
                    // remaining peers.
                    if let Err(e) = s.send(FromRemote::RemoteUpdate {
                        update: SliceWrapper::Raw(update),
                    }) {
                        tracing::warn!(error = ?e, "failed to send update to a peer; continuing");
                    }
                }
            }
        }
        // Handle an incoming awareness update from a peer
        // Should apply the update to the local epehemeral awareness strore
        FromPeer::PeerAwareness {
            awareness: awareness_update,
        } => {
            if let Err(e) = awareness.apply(*awareness_update) {
                tracing::warn!(error = ?e, "failed to apply awareness update; ignoring it");
                return Ok(());
            }
            let encodede = awareness.encode_all();
            broadcast_awareness(sender, sockets, &encodede)
                .context("failed to broadcast awareness")?;
        }
        // Handle a peer requesting a specific set of updates from the document.
        // The client sends a version vector (not frontiers) so unknown peers
        // — e.g. a peer that made offline edits the server hasn't seen yet —
        // don't cause a panic in `frontiersToVV` lookup.
        FromPeer::PeerRequestSince { vv } => {
            let decoded = VersionVector::decode(*vv).context("failed to decode version vector")?;

            let update = document_state
                .export_updates_since(&decoded)
                .context("failed to export updates")?;

            // Echo the client's *original* vv bytes back, not a re-encoded copy.
            // The client correlates the response by byte-exact match on the vv it
            // sent; `decode(vv).encode()` is not guaranteed to reproduce the same
            // bytes for a multi-peer version vector, which would make the client
            // discard a perfectly good response and time out.
            sender.send(FromRemote::RemoteUpdateSince {
                update: SliceWrapper::Raw(&update),
                vv,
            })?;
        }
        // Peer is requesting a snapshot from the remote
        FromPeer::PeerRequestSnapshot {} => {
            let snapshot = document_state.export_shallow_snapshot()?;

            sender.send(FromRemote::RemoteSnapshot {
                snapshot: SliceWrapper::Raw(&snapshot),
            })?;
        }
        FromPeer::Unknown => {
            return Err(worker::Error::from("unknown message type").into());
        }
    };

    Ok(())
}
