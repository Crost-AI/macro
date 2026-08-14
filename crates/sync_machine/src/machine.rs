//! One document's sync machine.
//!
//! Owns the replica, the attached peers, and the persistence bookkeeping for
//! a single document. Pure: `handle` is the entire API — feed an [`Input`],
//! collect [`Effect`]s. The invariants that were implicit in the Durable
//! Object (ack only after durable storage, one compaction at a time, initial
//! sync deferred until loaded, evict only when clean) are explicit state here
//! and covered by table-driven tests.

#[cfg(test)]
mod test;

use crate::model::{
    BlameEvent, Caps, ClientFrame, CloseReason, ConnId, Effect, Input, Lifecycle, PersistToken,
    RawPresence, RawSnapshot, RawUpdate, ServerFrame, TimerToken,
};
use crate::replica::Replica;
use std::collections::{BTreeMap, VecDeque};

/// Debounce between an accepted update and the snapshot compaction that folds
/// it in — the Durable Object's 5-second alarm.
pub const PERSIST_DEBOUNCE_MS: u64 = 5_000;

/// How long a peerless, clean document stays resident before asking to be
/// evicted.
pub const IDLE_EVICT_MS: u64 = 60_000;

/// Delay before retrying a failed persistence request.
pub const PERSIST_RETRY_MS: u64 = 1_000;

/// What a scheduled timer means when it fires.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TimerKind {
    /// Compact: persist a snapshot if anything changed.
    PersistDebounce,
    /// Evict if still peerless and clean.
    Idle,
    /// Re-attempt persistence after a failure.
    PersistRetry,
}

/// Where the document is in its life.
enum Phase<R> {
    /// Created, but nothing has attached yet; no load requested.
    Fresh,
    /// [`Effect::Load`] emitted; frames queue until the store answers.
    Loading {
        /// Frames received before the snapshot arrived, replayed in order
        /// once it does.
        queued: Vec<(ConnId, ClientFrame)>,
    },
    /// Live. The replica exists only here, so "apply before load" is
    /// unrepresentable.
    Ready {
        /// The materialized document.
        replica: R,
    },
    /// The store failed to load the document; attaches are refused.
    Broken,
}

/// Per-connection state.
#[derive(Debug, Clone)]
struct Peer {
    caps: Caps,
    /// CRDT peer ids this connection registered (usually one).
    peer_ids: Vec<u64>,
    /// The connection's latest presence payload, for initial sync.
    presence: Option<RawPresence>,
}

/// An update batch acked once `persisted_seq` reaches `through_seq`.
#[derive(Debug, Clone)]
struct PendingAck {
    conn: ConnId,
    id: String,
    through_seq: u64,
}

/// See the module docs.
pub struct DocMachine<R: Replica> {
    phase: Phase<R>,
    peers: BTreeMap<ConnId, Peer>,

    /// Last op sequence assigned. Assigned only *after* a successful apply,
    /// so any op holding a seq is contained in every later snapshot.
    seq: u64,
    /// Ops are durable through here; acks release up to this watermark.
    persisted_seq: u64,
    /// The last durable snapshot covers ops through here. `seq >
    /// snapshot_seq` is the machine's only definition of "dirty".
    snapshot_seq: u64,

    /// Ops not yet covered by a durable snapshot, retained for retry after a
    /// failed [`Effect::PersistOps`]. Trimmed when a snapshot commits.
    op_tail: VecDeque<(u64, RawUpdate)>,
    pending_acks: VecDeque<PendingAck>,

    /// At most one op-persist in flight; further ops wait in `op_tail` and go
    /// out when the current request completes.
    inflight_ops: Option<(PersistToken, u64)>,
    /// At most one snapshot-persist in flight.
    inflight_snapshot: Option<(PersistToken, u64)>,

    /// Live timers, so stale [`Input::TimerFired`]s are ignored.
    timers: BTreeMap<TimerToken, TimerKind>,
    persist_timer: Option<TimerToken>,
    idle_timer: Option<TimerToken>,
    retry_timer: Option<TimerToken>,

    next_token: u64,
}

impl<R: Replica> Default for DocMachine<R> {
    fn default() -> Self {
        Self::new()
    }
}

impl<R: Replica> DocMachine<R> {
    /// A fresh machine for a document nothing has touched yet.
    pub fn new() -> Self {
        Self {
            phase: Phase::Fresh,
            peers: BTreeMap::new(),
            seq: 0,
            persisted_seq: 0,
            snapshot_seq: 0,
            op_tail: VecDeque::new(),
            pending_acks: VecDeque::new(),
            inflight_ops: None,
            inflight_snapshot: None,
            timers: BTreeMap::new(),
            persist_timer: None,
            idle_timer: None,
            retry_timer: None,
            next_token: 0,
        }
    }

    /// Whether the machine holds live peers or unpersisted work. `false`
    /// means dropping it loses nothing.
    pub fn is_evictable(&self) -> bool {
        self.peers.is_empty()
            && self.seq == self.snapshot_seq
            && self.inflight_ops.is_none()
            && self.inflight_snapshot.is_none()
    }

    /// Feed one input; emitted effects are appended to `out`.
    pub fn handle(&mut self, input: Input, out: &mut Vec<Effect>) {
        match input {
            Input::PeerAttached { conn, caps } => self.on_attached(conn, caps, out),
            Input::PeerDetached { conn } => self.on_detached(conn, out),
            Input::Frame { conn, frame } => self.on_frame(conn, frame, out),
            Input::TimerFired { token } => self.on_timer(token, out),
            Input::Loaded { snapshot } => self.on_loaded(snapshot, out),
            Input::LoadFailed { error: _ } => self.on_load_failed(out),
            Input::OpsPersisted { token, through_seq } => {
                self.on_ops_persisted(token, through_seq, out);
            }
            Input::SnapshotPersisted { token } => self.on_snapshot_persisted(token, out),
            Input::PersistFailed { token } => self.on_persist_failed(token, out),
        }
    }

    // ── attach / detach ────────────────────────────────────────────────────

    fn on_attached(&mut self, conn: ConnId, caps: Caps, out: &mut Vec<Effect>) {
        if let Phase::Broken = self.phase {
            out.push(Effect::Close {
                conn,
                reason: CloseReason::LoadFailed,
            });
            return;
        }

        let first_join = self.peers.is_empty();
        self.peers.entry(conn).or_insert(Peer {
            caps,
            peer_ids: Vec::new(),
            presence: None,
        });
        // Any attach cancels a pending idle eviction.
        if let Some(token) = self.idle_timer.take() {
            self.timers.remove(&token);
        }

        match &self.phase {
            Phase::Fresh => {
                self.phase = Phase::Loading { queued: Vec::new() };
                out.push(Effect::Load);
                // Initial sync and FirstJoin are deferred to `Loaded`.
            }
            Phase::Loading { .. } => {
                // Deferred to `Loaded` alongside everyone else waiting.
            }
            Phase::Ready { replica } => {
                out.push(Effect::Send {
                    conn,
                    frame: ServerFrame::InitialSync {
                        snapshot: replica.snapshot(),
                        presence: self.presence_payloads(),
                    },
                });
                if first_join {
                    out.push(Effect::Lifecycle {
                        event: Lifecycle::FirstJoin,
                    });
                }
            }
            Phase::Broken => unreachable!("handled above"),
        }
    }

    fn on_detached(&mut self, conn: ConnId, out: &mut Vec<Effect>) {
        let Some(peer) = self.peers.remove(&conn) else {
            return;
        };
        if !peer.peer_ids.is_empty() {
            out.push(Effect::Broadcast {
                except: conn,
                frame: ServerFrame::PresenceLeft {
                    peer_ids: peer.peer_ids,
                },
            });
        }
        if self.peers.is_empty() {
            out.push(Effect::Lifecycle {
                event: Lifecycle::LastLeave,
            });
            let token = self.schedule(TimerKind::Idle, IDLE_EVICT_MS, out);
            self.idle_timer = Some(token);
        }
    }

    // ── frames ────────────────────────────────────────────────────────────

    fn on_frame(&mut self, conn: ConnId, frame: ClientFrame, out: &mut Vec<Effect>) {
        if !self.peers.contains_key(&conn) {
            out.push(Effect::Close {
                conn,
                reason: CloseReason::NotAttached,
            });
            return;
        }
        match &mut self.phase {
            Phase::Fresh | Phase::Broken => {
                // Fresh is unreachable for an attached conn; Broken conns were
                // closed at attach. Drop defensively.
                return;
            }
            Phase::Loading { queued } => {
                queued.push((conn, frame));
                return;
            }
            Phase::Ready { .. } => {}
        }
        self.on_ready_frame(conn, frame, out);
    }

    fn on_ready_frame(&mut self, conn: ConnId, frame: ClientFrame, out: &mut Vec<Effect>) {
        match frame {
            ClientFrame::Update { updates, id } => self.on_update(conn, updates, id, out),
            ClientFrame::Presence { payload } => {
                if let Some(peer) = self.peers.get_mut(&conn) {
                    peer.presence = Some(payload.clone());
                }
                out.push(Effect::Broadcast {
                    except: conn,
                    frame: ServerFrame::Presence { payload },
                });
            }
            ClientFrame::RequestSince { cursor } => {
                let Phase::Ready { replica } = &self.phase else {
                    unreachable!("on_ready_frame is only called in Ready");
                };
                match replica.diff_since(&cursor) {
                    Ok(update) => out.push(Effect::Send {
                        conn,
                        frame: ServerFrame::Since { update, cursor },
                    }),
                    Err(_) => out.push(Effect::Close {
                        conn,
                        reason: CloseReason::Protocol,
                    }),
                }
            }
            ClientFrame::RequestSnapshot => {
                let Phase::Ready { replica } = &self.phase else {
                    unreachable!("on_ready_frame is only called in Ready");
                };
                out.push(Effect::Send {
                    conn,
                    frame: ServerFrame::Snapshot {
                        snapshot: replica.snapshot(),
                    },
                });
            }
            ClientFrame::RegisterPeer { peer_id } => {
                let Some(peer) = self.peers.get_mut(&conn) else {
                    return;
                };
                if !peer.peer_ids.contains(&peer_id) {
                    peer.peer_ids.push(peer_id);
                    if let Some(user_id) = peer.caps.user_id.clone() {
                        out.push(Effect::RecordPeerMapping { peer_id, user_id });
                    }
                }
            }
        }
    }

    fn on_update(
        &mut self,
        conn: ConnId,
        updates: Vec<RawUpdate>,
        id: String,
        out: &mut Vec<Effect>,
    ) {
        let Some(peer) = self.peers.get(&conn) else {
            return;
        };
        if !peer.caps.can_edit {
            // Matches today's service: silently drop (the client's ack
            // timeout surfaces it). Not a Close — view-only tabs shouldn't be
            // disconnected for a stray keystroke race.
            return;
        }
        let author_peer_id = peer.peer_ids.first().copied();

        let Phase::Ready { replica } = &mut self.phase else {
            unreachable!("on_ready_frame is only called in Ready");
        };

        let mut blame: Vec<BlameEvent> = Vec::new();
        let mut applied: Vec<RawUpdate> = Vec::new();
        let mut poisoned = false;
        for update in updates {
            match replica.apply(&update) {
                Ok(result) => {
                    // Apply first, THEN assign the sequence: any op holding a
                    // seq is already in the replica, so a snapshot taken at
                    // any later watermark necessarily contains it.
                    self.seq += 1;
                    self.op_tail.push_back((self.seq, update.clone()));
                    if let Some(peer_id) = author_peer_id {
                        blame.extend(
                            result
                                .touched_nodes
                                .into_iter()
                                .map(|node_id| BlameEvent { node_id, peer_id }),
                        );
                    }
                    applied.push(update);
                }
                Err(_) => {
                    // Poison never reaches the log or the peers. Ops applied
                    // earlier in the batch stand (they're already in the
                    // replica), matching the service's abort-on-error today.
                    poisoned = true;
                    break;
                }
            }
        }

        if !applied.is_empty() {
            self.pending_acks.push_back(PendingAck {
                conn,
                id,
                through_seq: self.seq,
            });
            self.maybe_emit_persist_ops(out);
            if !blame.is_empty() {
                out.push(Effect::RecordBlame { events: blame });
            }
            // Broadcast immediately rather than after durability: peers
            // converge faster, and a crash-before-persist is healed by the
            // sender re-pushing its unacked batch.
            for update in applied {
                out.push(Effect::Broadcast {
                    except: conn,
                    frame: ServerFrame::Update { update },
                });
            }
            if self.persist_timer.is_none() {
                let token = self.schedule(TimerKind::PersistDebounce, PERSIST_DEBOUNCE_MS, out);
                self.persist_timer = Some(token);
            }
        }

        if poisoned {
            out.push(Effect::Close {
                conn,
                reason: CloseReason::Protocol,
            });
        }
    }

    // ── load ──────────────────────────────────────────────────────────────

    fn on_loaded(&mut self, snapshot: Option<RawSnapshot>, out: &mut Vec<Effect>) {
        if !matches!(self.phase, Phase::Loading { .. }) {
            // A completion for a load we no longer care about; ignore.
            return;
        }
        let Phase::Loading { queued } = std::mem::replace(&mut self.phase, Phase::Fresh) else {
            unreachable!("checked above");
        };

        let replica = match snapshot {
            Some(bytes) => match R::load(&bytes) {
                Ok(replica) => replica,
                Err(_) => {
                    self.phase = Phase::Broken;
                    self.close_all(CloseReason::LoadFailed, out);
                    return;
                }
            },
            None => R::empty(),
        };
        self.phase = Phase::Ready { replica };

        // Everyone who attached during the load gets their initial sync now.
        let (snapshot, presence) = {
            let Phase::Ready { replica } = &self.phase else {
                unreachable!("just set");
            };
            (replica.snapshot(), self.presence_payloads())
        };
        for conn in self.peers.keys().copied().collect::<Vec<_>>() {
            out.push(Effect::Send {
                conn,
                frame: ServerFrame::InitialSync {
                    snapshot: snapshot.clone(),
                    presence: presence.clone(),
                },
            });
        }
        if !self.peers.is_empty() {
            out.push(Effect::Lifecycle {
                event: Lifecycle::FirstJoin,
            });
        }

        // Replay frames that raced the load, in arrival order.
        for (conn, frame) in queued {
            // A conn may have detached while loading; on_frame handles it.
            if self.peers.contains_key(&conn) {
                self.on_ready_frame(conn, frame, out);
            }
        }
    }

    fn on_load_failed(&mut self, out: &mut Vec<Effect>) {
        if !matches!(self.phase, Phase::Loading { .. }) {
            return;
        }
        self.phase = Phase::Broken;
        self.close_all(CloseReason::LoadFailed, out);
    }

    // ── persistence completions ───────────────────────────────────────────

    fn on_ops_persisted(&mut self, token: PersistToken, through_seq: u64, out: &mut Vec<Effect>) {
        if self.inflight_ops.map(|(t, _)| t) != Some(token) {
            return; // stale completion
        }
        self.inflight_ops = None;
        self.persisted_seq = self.persisted_seq.max(through_seq);

        // Release every ack the new watermark covers, in order.
        while let Some(ack) = self.pending_acks.front() {
            if ack.through_seq > self.persisted_seq {
                break;
            }
            let ack = self.pending_acks.pop_front().expect("front checked");
            // The conn may have detached since; sending to a gone conn is the
            // runtime's no-op, not ours.
            out.push(Effect::Send {
                conn: ack.conn,
                frame: ServerFrame::Ack { id: ack.id },
            });
        }

        // More ops arrived while this request was in flight.
        self.maybe_emit_persist_ops(out);
    }

    fn on_snapshot_persisted(&mut self, token: PersistToken, out: &mut Vec<Effect>) {
        let Some((inflight, through_seq)) = self.inflight_snapshot else {
            return;
        };
        if inflight != token {
            return; // stale completion
        }
        self.inflight_snapshot = None;
        self.snapshot_seq = self.snapshot_seq.max(through_seq);
        // Ops covered by the snapshot are no longer needed for retry.
        while self
            .op_tail
            .front()
            .is_some_and(|(seq, _)| *seq <= self.snapshot_seq)
        {
            self.op_tail.pop_front();
        }
        out.push(Effect::Lifecycle {
            event: Lifecycle::Edited,
        });
        // If everyone left while the compaction was in flight, we may be
        // clean now; the idle timer (already armed at LastLeave) will evict.
    }

    fn on_persist_failed(&mut self, token: PersistToken, out: &mut Vec<Effect>) {
        let failed_ops = self.inflight_ops.map(|(t, _)| t) == Some(token);
        let failed_snapshot = self.inflight_snapshot.map(|(t, _)| t) == Some(token);
        if !failed_ops && !failed_snapshot {
            return; // stale completion
        }
        if failed_ops {
            self.inflight_ops = None;
        }
        if failed_snapshot {
            self.inflight_snapshot = None;
        }
        // Keep pending acks: if the retry succeeds the acks flow late, which
        // clients treat as a no-op after their own timeout has fired.
        if self.retry_timer.is_none() {
            let token = self.schedule(TimerKind::PersistRetry, PERSIST_RETRY_MS, out);
            self.retry_timer = Some(token);
        }
    }

    // ── timers ────────────────────────────────────────────────────────────

    fn on_timer(&mut self, token: TimerToken, out: &mut Vec<Effect>) {
        let Some(kind) = self.timers.remove(&token) else {
            return; // cancelled or unknown
        };
        match kind {
            TimerKind::PersistDebounce => {
                self.persist_timer = None;
                self.maybe_emit_persist_snapshot(out);
            }
            TimerKind::Idle => {
                self.idle_timer = None;
                if !self.peers.is_empty() {
                    return;
                }
                if self.is_evictable() {
                    out.push(Effect::Evict);
                } else {
                    // Dirty at idle: compact first, evict on the next tick.
                    self.maybe_emit_persist_ops(out);
                    self.maybe_emit_persist_snapshot(out);
                    let token = self.schedule(TimerKind::Idle, IDLE_EVICT_MS, out);
                    self.idle_timer = Some(token);
                }
            }
            TimerKind::PersistRetry => {
                self.retry_timer = None;
                self.maybe_emit_persist_ops(out);
                // A failed snapshot persist is retried by re-exporting fresh
                // state rather than resending stale bytes.
                self.maybe_emit_persist_snapshot(out);
            }
        }
    }

    // ── helpers ───────────────────────────────────────────────────────────

    /// Emit a `PersistOps` for the unpersisted tail, if any and none in
    /// flight.
    fn maybe_emit_persist_ops(&mut self, out: &mut Vec<Effect>) {
        if self.inflight_ops.is_some() || self.persisted_seq >= self.seq {
            return;
        }
        let ops: Vec<(u64, RawUpdate)> = self
            .op_tail
            .iter()
            .filter(|(seq, _)| *seq > self.persisted_seq)
            .cloned()
            .collect();
        if ops.is_empty() {
            return;
        }
        let through_seq = ops.last().expect("non-empty").0;
        let token = self.next_persist_token();
        self.inflight_ops = Some((token, through_seq));
        out.push(Effect::PersistOps {
            token,
            ops,
            through_seq,
        });
    }

    /// Emit a `PersistSnapshot` if the replica has changes no snapshot
    /// covers and none is in flight.
    fn maybe_emit_persist_snapshot(&mut self, out: &mut Vec<Effect>) {
        if self.inflight_snapshot.is_some() || self.seq == self.snapshot_seq {
            return;
        }
        let snapshot = {
            let Phase::Ready { replica } = &self.phase else {
                return;
            };
            replica.snapshot()
        };
        let token = self.next_persist_token();
        self.inflight_snapshot = Some((token, self.seq));
        out.push(Effect::PersistSnapshot {
            token,
            snapshot,
            through_seq: self.seq,
        });
    }

    fn presence_payloads(&self) -> Vec<RawPresence> {
        self.peers
            .values()
            .filter_map(|peer| peer.presence.clone())
            .collect()
    }

    fn close_all(&mut self, reason: CloseReason, out: &mut Vec<Effect>) {
        for conn in self.peers.keys().copied().collect::<Vec<_>>() {
            out.push(Effect::Close { conn, reason });
        }
        self.peers.clear();
    }

    fn schedule(&mut self, kind: TimerKind, after_ms: u64, out: &mut Vec<Effect>) -> TimerToken {
        self.next_token += 1;
        let token = TimerToken(self.next_token);
        self.timers.insert(token, kind);
        out.push(Effect::ScheduleTimer { token, after_ms });
        token
    }

    fn next_persist_token(&mut self) -> PersistToken {
        self.next_token += 1;
        PersistToken(self.next_token)
    }

    /// Test-only access to the live replica.
    #[cfg(test)]
    pub(crate) fn replica(&self) -> Option<&R> {
        match &self.phase {
            Phase::Ready { replica } => Some(replica),
            _ => None,
        }
    }
}
