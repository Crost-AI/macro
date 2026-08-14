//! The same scenarios as `sync_machine`'s table tests, expressed the only way
//! an impure actor allows: shared mock logs, paused tokio time, and yielding
//! to let the actor task run. Compare the shape of these tests with the pure
//! crate's — that comparison is the point of this crate.

use super::*;
use crate::domain::ports::StoreError;
use std::sync::Mutex;
use sync_machine::model::RawSnapshot;
use sync_machine::replica::mock::MockReplica;
use tokio::sync::Notify;

/// One shared, ordered record of everything that happened across all ports —
/// the impure stand-in for the pure machine's effect list.
#[derive(Default)]
struct Log {
    entries: Mutex<Vec<String>>,
}

impl Log {
    fn push(&self, entry: String) {
        self.entries.lock().unwrap().push(entry);
    }
    fn take(&self) -> Vec<String> {
        std::mem::take(&mut self.entries.lock().unwrap())
    }
}

#[derive(Default)]
struct MockStore {
    log: Arc<Log>,
    /// Snapshot served by `load`.
    stored: Option<RawSnapshot>,
    /// How many times `append_ops` fails before succeeding.
    append_failures: Mutex<u32>,
    /// When set, `load` waits until notified (for queue-during-load tests).
    hold_load: Option<Arc<Notify>>,
}

impl DocStore for MockStore {
    async fn load(&self, _doc: &DocId) -> Result<Option<RawSnapshot>, StoreError> {
        if let Some(hold) = &self.hold_load {
            hold.notified().await;
        }
        self.log.push("load".into());
        Ok(self.stored.clone())
    }

    async fn append_ops(&self, _doc: &DocId, ops: &[(u64, RawUpdate)]) -> Result<(), StoreError> {
        let mut failures = self.append_failures.lock().unwrap();
        if *failures > 0 {
            *failures -= 1;
            self.log.push("append:fail".into());
            return Err(StoreError("injected".into()));
        }
        let seqs: Vec<String> = ops.iter().map(|(seq, _)| seq.to_string()).collect();
        self.log.push(format!("append:{}", seqs.join(",")));
        Ok(())
    }

    async fn store_snapshot(
        &self,
        _doc: &DocId,
        _snapshot: RawSnapshot,
        through_seq: u64,
    ) -> Result<(), StoreError> {
        self.log.push(format!("snapshot:{through_seq}"));
        Ok(())
    }
}

struct MockSink {
    log: Arc<Log>,
}

impl ClientSink for MockSink {
    async fn send(&self, conn: ConnId, frame: ServerFrame) {
        let kind = match &frame {
            ServerFrame::InitialSync { .. } => "initial-sync".into(),
            ServerFrame::Update { .. } => "update".into(),
            ServerFrame::Ack { id } => format!("ack:{id}"),
            ServerFrame::Presence { .. } => "presence".into(),
            ServerFrame::PresenceLeft { .. } => "presence-left".into(),
            ServerFrame::Snapshot { .. } => "snapshot".into(),
            ServerFrame::Since { .. } => "since".into(),
        };
        self.log.push(format!("send:{}:{kind}", conn.0));
    }

    async fn close(&self, conn: ConnId, reason: CloseReason) {
        self.log.push(format!("close:{}:{reason:?}", conn.0));
    }
}

struct MockEvents {
    log: Arc<Log>,
}

impl DocEvents for MockEvents {
    async fn lifecycle(&self, _doc: &DocId, event: Lifecycle) {
        self.log.push(format!("lifecycle:{event:?}"));
    }
    async fn blame(&self, _doc: &DocId, events: Vec<BlameEvent>) {
        self.log.push(format!("blame:{}", events.len()));
    }
    async fn peer_mapping(&self, _doc: &DocId, peer_id: u64, user_id: String) {
        self.log.push(format!("peer-mapping:{peer_id}:{user_id}"));
    }
}

struct Rig {
    tx: mpsc::Sender<DocMsg>,
    log: Arc<Log>,
    handle: tokio::task::JoinHandle<()>,
}

fn rig_with(store: MockStore, log: Arc<Log>) -> Rig {
    let (tx, rx) = mpsc::channel(64);
    let handle = tokio::spawn(actor_run(rx, store, Arc::clone(&log)));
    Rig { tx, log, handle }
}

async fn actor_run(rx: mpsc::Receiver<DocMsg>, store: MockStore, log: Arc<Log>) {
    super::run::<MockReplica, _, _, _>(
        DocId("doc-a".into()),
        rx,
        Arc::new(store),
        Arc::new(MockSink {
            log: Arc::clone(&log),
        }),
        Arc::new(MockEvents { log }),
    )
    .await;
}

fn rig() -> Rig {
    let log = Arc::<Log>::default();
    rig_with(
        MockStore {
            log: Arc::clone(&log),
            stored: Some(RawSnapshot::from(&b"base"[..])),
            ..Default::default()
        },
        log,
    )
}

fn caps() -> Caps {
    Caps {
        can_edit: true,
        user_id: Some("user-1".into()),
    }
}

async fn settle() {
    // Let the actor task run until it parks. Yield a few times: each message
    // may involve several awaits before the actor returns to its select.
    for _ in 0..32 {
        tokio::task::yield_now().await;
    }
}

fn update(payload: &[u8], id: &str) -> ClientFrame {
    ClientFrame::Update {
        updates: vec![RawUpdate::from(payload)],
        id: id.into(),
    }
}

const C1: ConnId = ConnId(1);
const C2: ConnId = ConnId(2);

#[tokio::test(start_paused = true)]
async fn attach_gets_initial_sync_and_first_join() {
    let r = rig();
    r.tx.send(DocMsg::Attach {
        conn: C1,
        caps: caps(),
    })
    .await
    .unwrap();
    settle().await;
    assert_eq!(
        r.log.take(),
        vec!["load", "send:1:initial-sync", "lifecycle:FirstJoin"]
    );
}

#[tokio::test(start_paused = true)]
async fn update_order_is_apply_persist_ack_broadcast() {
    let r = rig();
    r.tx.send(DocMsg::Attach {
        conn: C1,
        caps: caps(),
    })
    .await
    .unwrap();
    r.tx.send(DocMsg::Attach {
        conn: C2,
        caps: caps(),
    })
    .await
    .unwrap();
    settle().await;
    r.log.take();

    r.tx.send(DocMsg::Frame {
        conn: C1,
        frame: update(b"x", "op-1"),
    })
    .await
    .unwrap();
    settle().await;
    // Durability strictly precedes the ack, which precedes the broadcast.
    assert_eq!(
        r.log.take(),
        vec!["append:1", "send:1:ack:op-1", "send:2:update"]
    );
}

#[tokio::test(start_paused = true)]
async fn persist_failure_retries_after_a_delay_and_still_acks() {
    let log = Arc::<Log>::default();
    let r = rig_with(
        MockStore {
            log: Arc::clone(&log),
            stored: Some(RawSnapshot::from(&b"base"[..])),
            append_failures: Mutex::new(1),
            ..Default::default()
        },
        log,
    );
    r.tx.send(DocMsg::Attach {
        conn: C1,
        caps: caps(),
    })
    .await
    .unwrap();
    settle().await;
    r.log.take();

    r.tx.send(DocMsg::Frame {
        conn: C1,
        frame: update(b"x", "op-1"),
    })
    .await
    .unwrap();
    settle().await;
    assert_eq!(r.log.take(), vec!["append:fail"]); // stalled in the retry sleep
    tokio::time::advance(PERSIST_RETRY).await;
    settle().await;
    assert_eq!(r.log.take(), vec!["append:1", "send:1:ack:op-1"]);
}

#[tokio::test(start_paused = true)]
async fn frames_arriving_during_load_wait_in_the_mailbox() {
    let hold = Arc::new(Notify::new());
    let log = Arc::<Log>::default();
    let r = rig_with(
        MockStore {
            log: Arc::clone(&log),
            stored: Some(RawSnapshot::from(&b"base"[..])),
            hold_load: Some(Arc::clone(&hold)),
            ..Default::default()
        },
        log,
    );
    r.tx.send(DocMsg::Attach {
        conn: C1,
        caps: caps(),
    })
    .await
    .unwrap();
    r.tx.send(DocMsg::Frame {
        conn: C1,
        frame: update(b"early", "op-1"),
    })
    .await
    .unwrap();
    settle().await;
    assert!(r.log.take().is_empty()); // still parked in load

    hold.notify_one();
    settle().await;
    let entries = r.log.take();
    assert_eq!(
        entries,
        vec![
            "load",
            "send:1:initial-sync",
            "lifecycle:FirstJoin",
            "append:1",
            "send:1:ack:op-1",
        ]
    );
}

#[tokio::test(start_paused = true)]
async fn compaction_fires_after_the_debounce() {
    let r = rig();
    r.tx.send(DocMsg::Attach {
        conn: C1,
        caps: caps(),
    })
    .await
    .unwrap();
    r.tx.send(DocMsg::Frame {
        conn: C1,
        frame: update(b"x", "op-1"),
    })
    .await
    .unwrap();
    settle().await;
    r.log.take();

    tokio::time::advance(PERSIST_DEBOUNCE).await;
    settle().await;
    assert_eq!(r.log.take(), vec!["snapshot:1", "lifecycle:Edited"]);
}

#[tokio::test(start_paused = true)]
async fn idle_actor_exits_after_the_evict_window() {
    let r = rig();
    r.tx.send(DocMsg::Attach {
        conn: C1,
        caps: caps(),
    })
    .await
    .unwrap();
    settle().await;
    r.tx.send(DocMsg::Detach { conn: C1 }).await.unwrap();
    settle().await;

    tokio::time::advance(IDLE_EVICT).await;
    settle().await;
    assert!(r.handle.is_finished());
}

#[tokio::test(start_paused = true)]
async fn poison_update_closes_and_detaches_the_sender() {
    let r = rig();
    r.tx.send(DocMsg::Attach {
        conn: C1,
        caps: caps(),
    })
    .await
    .unwrap();
    settle().await;
    r.log.take();

    r.tx.send(DocMsg::Frame {
        conn: C1,
        frame: update(b"__poison__", "op-1"),
    })
    .await
    .unwrap();
    settle().await;
    assert_eq!(r.log.take(), vec!["close:1:Protocol"]);
}

/// THE comparison test: while a slow persist is in flight, the pure machine
/// keeps serving other frames; this actor stalls the whole document. A
/// presence frame sent behind a slow append is only broadcast after the
/// append finishes.
#[tokio::test(start_paused = true)]
async fn head_of_line_blocking_a_slow_persist_stalls_presence() {
    let log = Arc::<Log>::default();
    let r = rig_with(
        MockStore {
            log: Arc::clone(&log),
            stored: Some(RawSnapshot::from(&b"base"[..])),
            append_failures: Mutex::new(1), // fail once → 1s retry sleep = "slow store"
            ..Default::default()
        },
        log,
    );
    r.tx.send(DocMsg::Attach {
        conn: C1,
        caps: caps(),
    })
    .await
    .unwrap();
    r.tx.send(DocMsg::Attach {
        conn: C2,
        caps: caps(),
    })
    .await
    .unwrap();
    settle().await;
    r.log.take();

    r.tx.send(DocMsg::Frame {
        conn: C1,
        frame: update(b"x", "op-1"),
    })
    .await
    .unwrap();
    r.tx.send(DocMsg::Frame {
        conn: C2,
        frame: ClientFrame::Presence {
            payload: sync_machine::model::RawPresence::from(&b"cursor"[..]),
        },
    })
    .await
    .unwrap();
    settle().await;

    // The presence frame is stuck behind the retry sleep.
    assert_eq!(r.log.take(), vec!["append:fail"]);

    tokio::time::advance(PERSIST_RETRY).await;
    settle().await;
    let entries = r.log.take();
    // Only after the persist completes does presence flow.
    assert_eq!(
        entries,
        vec![
            "append:1",
            "send:1:ack:op-1",
            "send:2:update",
            "send:1:presence",
        ]
    );
}
