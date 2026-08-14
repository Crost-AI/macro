//! One document's actor: an async task owning the replica and its ports.
//!
//! Compare with `sync_machine::machine::DocMachine`. The logic is the same;
//! the encoding is straight-line async. What that buys and costs:
//!
//! - Ack-after-durable is program order (`append_ops(...).await` then
//!   `sink.send(Ack)`), so the pure machine's `pending_acks` /
//!   `persisted_seq` / persist tokens simply don't exist. The invariant is
//!   free — because **the whole document blocks on every store call**: any
//!   frame arriving while a persist is in flight waits in the mailbox,
//!   including presence and reads the pure machine would have served
//!   concurrently.
//! - Retry is a loop around the await instead of a retry timer + state.
//! - Debounce/idle are `select!` deadlines on real (tokio) time.

#[cfg(test)]
mod test;

use crate::domain::ports::{ClientSink, DocEvents, DocStore};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;
use sync_machine::model::{
    BlameEvent, Caps, ClientFrame, CloseReason, ConnId, DocId, Lifecycle, RawPresence, RawUpdate,
    ServerFrame,
};
use sync_machine::replica::Replica;
use tokio::sync::mpsc;
use tokio::time::Instant;
use tracing::warn;

/// Debounce between an accepted update and snapshot compaction (the Durable
/// Object's 5-second alarm).
pub const PERSIST_DEBOUNCE: Duration = Duration::from_secs(5);

/// How long a peerless, clean document stays resident before exiting.
pub const IDLE_EVICT: Duration = Duration::from_secs(60);

/// Delay between persistence retries.
pub const PERSIST_RETRY: Duration = Duration::from_secs(1);

/// Give up on a store call after this many attempts and close the document
/// (clients re-subscribe and the supervisor respawns).
pub const MAX_PERSIST_ATTEMPTS: u32 = 5;

/// Everything a client connection can tell the actor.
#[derive(Debug)]
pub enum DocMsg {
    /// A connection attached (capabilities already resolved by the edge).
    Attach {
        /// The attaching connection.
        conn: ConnId,
        /// What it may do.
        caps: Caps,
    },
    /// A connection detached.
    Detach {
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
}

struct Peer {
    caps: Caps,
    peer_ids: Vec<u64>,
    presence: Option<RawPresence>,
}

/// The actor's state; owned by [`run`]'s task, never shared.
struct DocActor<R: Replica, Store, Sink, Events> {
    doc: DocId,
    replica: R,
    peers: BTreeMap<ConnId, Peer>,
    seq: u64,
    snapshot_seq: u64,
    store: Arc<Store>,
    sink: Arc<Sink>,
    events: Arc<Events>,
}

/// Run one document to completion: load, serve until idle, exit.
///
/// The supervisor spawns this with the mailbox receiver; messages sent while
/// the (retried) load is in flight simply wait in the channel — the mailbox
/// *is* the pure machine's `Loading { queued }`.
pub async fn run<R, Store, Sink, Events>(
    doc: DocId,
    mut mailbox: mpsc::Receiver<DocMsg>,
    store: Arc<Store>,
    sink: Arc<Sink>,
    events: Arc<Events>,
) where
    R: Replica,
    Store: DocStore,
    Sink: ClientSink,
    Events: DocEvents,
{
    // ── load ────────────────────────────────────────────────────────────
    let replica = 'load: {
        let mut attempt = 0;
        loop {
            match store.load(&doc).await {
                Ok(Some(snapshot)) => match R::load(&snapshot) {
                    Ok(replica) => break 'load Some(replica),
                    Err(error) => {
                        warn!(error = ?error, "stored snapshot is unreadable");
                        break 'load None;
                    }
                },
                Ok(None) => break 'load Some(R::empty()),
                Err(error) => {
                    attempt += 1;
                    if attempt >= MAX_PERSIST_ATTEMPTS {
                        warn!(error = ?error, "giving up loading document");
                        break 'load None;
                    }
                    tokio::time::sleep(PERSIST_RETRY).await;
                }
            }
        }
    };
    let Some(replica) = replica else {
        // Broken: refuse everyone until the mailbox closes.
        while let Some(msg) = mailbox.recv().await {
            if let DocMsg::Attach { conn, .. } | DocMsg::Frame { conn, .. } = msg {
                sink.close(conn, CloseReason::LoadFailed).await;
            }
        }
        return;
    };

    let mut actor = DocActor {
        doc,
        replica,
        peers: BTreeMap::new(),
        seq: 0,
        snapshot_seq: 0,
        store,
        sink,
        events,
    };

    // ── serve ───────────────────────────────────────────────────────────
    // `None` deadlines are disabled select arms.
    let mut compact_at: Option<Instant> = None;
    let mut idle_at: Option<Instant> = Some(Instant::now() + IDLE_EVICT);

    loop {
        tokio::select! {
            biased;
            msg = mailbox.recv() => {
                let Some(msg) = msg else { break }; // supervisor dropped us
                actor.on_msg(msg, &mut compact_at, &mut idle_at).await;
            }
            _ = deadline(compact_at) => {
                compact_at = None;
                actor.compact().await;
            }
            _ = deadline(idle_at) => {
                if !actor.peers.is_empty() {
                    idle_at = None;
                    continue;
                }
                if actor.seq != actor.snapshot_seq {
                    actor.compact().await;
                }
                break; // exit; the supervisor respawns on next use
            }
        }
    }
}

/// Sleep until `at`, or forever when disabled.
async fn deadline(at: Option<Instant>) {
    match at {
        Some(at) => tokio::time::sleep_until(at).await,
        None => std::future::pending().await,
    }
}

impl<R, Store, Sink, Events> DocActor<R, Store, Sink, Events>
where
    R: Replica,
    Store: DocStore,
    Sink: ClientSink,
    Events: DocEvents,
{
    async fn on_msg(
        &mut self,
        msg: DocMsg,
        compact_at: &mut Option<Instant>,
        idle_at: &mut Option<Instant>,
    ) {
        match msg {
            DocMsg::Attach { conn, caps } => {
                let first_join = self.peers.is_empty();
                *idle_at = None;
                self.peers.entry(conn).or_insert(Peer {
                    caps,
                    peer_ids: Vec::new(),
                    presence: None,
                });
                let presence = self
                    .peers
                    .values()
                    .filter_map(|peer| peer.presence.clone())
                    .collect();
                self.sink
                    .send(
                        conn,
                        ServerFrame::InitialSync {
                            snapshot: self.replica.snapshot(),
                            presence,
                        },
                    )
                    .await;
                if first_join {
                    self.events.lifecycle(&self.doc, Lifecycle::FirstJoin).await;
                }
            }
            DocMsg::Detach { conn } => {
                let Some(peer) = self.peers.remove(&conn) else {
                    return;
                };
                if !peer.peer_ids.is_empty() {
                    self.broadcast(
                        conn,
                        ServerFrame::PresenceLeft {
                            peer_ids: peer.peer_ids,
                        },
                    )
                    .await;
                }
                if self.peers.is_empty() {
                    self.events.lifecycle(&self.doc, Lifecycle::LastLeave).await;
                    *idle_at = Some(Instant::now() + IDLE_EVICT);
                }
            }
            DocMsg::Frame { conn, frame } => {
                if !self.peers.contains_key(&conn) {
                    self.sink.close(conn, CloseReason::NotAttached).await;
                    return;
                }
                self.on_frame(conn, frame, compact_at).await;
            }
        }
    }

    async fn on_frame(
        &mut self,
        conn: ConnId,
        frame: ClientFrame,
        compact_at: &mut Option<Instant>,
    ) {
        match frame {
            ClientFrame::Update { updates, id } => {
                self.on_update(conn, updates, id, compact_at).await;
            }
            ClientFrame::Presence { payload } => {
                if let Some(peer) = self.peers.get_mut(&conn) {
                    peer.presence = Some(payload.clone());
                }
                self.broadcast(conn, ServerFrame::Presence { payload })
                    .await;
            }
            ClientFrame::RequestSince { cursor } => match self.replica.diff_since(&cursor) {
                Ok(update) => {
                    self.sink
                        .send(conn, ServerFrame::Since { update, cursor })
                        .await;
                }
                Err(_) => self.sink.close(conn, CloseReason::Protocol).await,
            },
            ClientFrame::RequestSnapshot => {
                self.sink
                    .send(
                        conn,
                        ServerFrame::Snapshot {
                            snapshot: self.replica.snapshot(),
                        },
                    )
                    .await;
            }
            ClientFrame::RegisterPeer { peer_id } => {
                let Some(peer) = self.peers.get_mut(&conn) else {
                    return;
                };
                if !peer.peer_ids.contains(&peer_id) {
                    peer.peer_ids.push(peer_id);
                    if let Some(user_id) = peer.caps.user_id.clone() {
                        self.events.peer_mapping(&self.doc, peer_id, user_id).await;
                    }
                }
            }
        }
    }

    async fn on_update(
        &mut self,
        conn: ConnId,
        updates: Vec<RawUpdate>,
        id: String,
        compact_at: &mut Option<Instant>,
    ) {
        let Some(peer) = self.peers.get(&conn) else {
            return;
        };
        if !peer.caps.can_edit {
            return; // silently dropped, as today
        }
        let author = peer.peer_ids.first().copied();

        let mut applied: Vec<(u64, RawUpdate)> = Vec::new();
        let mut blame: Vec<BlameEvent> = Vec::new();
        let mut poisoned = false;
        for update in updates {
            match self.replica.apply(&update) {
                Ok(result) => {
                    self.seq += 1;
                    if let Some(peer_id) = author {
                        blame.extend(
                            result
                                .touched_nodes
                                .into_iter()
                                .map(|node_id| BlameEvent { node_id, peer_id }),
                        );
                    }
                    applied.push((self.seq, update));
                }
                Err(_) => {
                    poisoned = true;
                    break;
                }
            }
        }

        if !applied.is_empty() {
            // The whole point of comparison: durability then ack is just
            // program order... and the whole document waits right here.
            if !self.persist_with_retry(&applied).await {
                // The store is gone; close everyone and let re-subscription
                // (through a respawned actor) sort the world out.
                self.close_all(CloseReason::LoadFailed).await;
                return;
            }
            self.sink.send(conn, ServerFrame::Ack { id }).await;
            if !blame.is_empty() {
                self.events.blame(&self.doc, blame).await;
            }
            for (_, update) in applied {
                self.broadcast(conn, ServerFrame::Update { update }).await;
            }
            if compact_at.is_none() {
                *compact_at = Some(Instant::now() + PERSIST_DEBOUNCE);
            }
        }

        if poisoned {
            self.sink.close(conn, CloseReason::Protocol).await;
            self.peers.remove(&conn);
        }
    }

    async fn persist_with_retry(&self, ops: &[(u64, RawUpdate)]) -> bool {
        for attempt in 1..=MAX_PERSIST_ATTEMPTS {
            match self.store.append_ops(&self.doc, ops).await {
                Ok(()) => return true,
                Err(error) => {
                    warn!(error = ?error, attempt, "append_ops failed");
                    if attempt < MAX_PERSIST_ATTEMPTS {
                        tokio::time::sleep(PERSIST_RETRY).await;
                    }
                }
            }
        }
        false
    }

    async fn compact(&mut self) {
        if self.seq == self.snapshot_seq {
            return;
        }
        let snapshot = self.replica.snapshot();
        let through_seq = self.seq;
        for attempt in 1..=MAX_PERSIST_ATTEMPTS {
            match self
                .store
                .store_snapshot(&self.doc, snapshot.clone(), through_seq)
                .await
            {
                Ok(()) => {
                    self.snapshot_seq = through_seq;
                    self.events.lifecycle(&self.doc, Lifecycle::Edited).await;
                    return;
                }
                Err(error) => {
                    warn!(error = ?error, attempt, "store_snapshot failed");
                    if attempt < MAX_PERSIST_ATTEMPTS {
                        tokio::time::sleep(PERSIST_RETRY).await;
                    }
                }
            }
        }
    }

    async fn broadcast(&self, except: ConnId, frame: ServerFrame) {
        for conn in self.peers.keys().copied() {
            if conn != except {
                self.sink.send(conn, frame.clone()).await;
            }
        }
    }

    async fn close_all(&mut self, reason: CloseReason) {
        for conn in self.peers.keys().copied().collect::<Vec<_>>() {
            self.sink.close(conn, reason).await;
        }
        self.peers.clear();
    }
}
