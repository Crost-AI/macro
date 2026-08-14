use super::*;
use crate::domain::actor::IDLE_EVICT;
use crate::domain::ports::{ClientSink, DocEvents, DocStore, StoreError};
use std::sync::Mutex;
use sync_machine::model::{
    BlameEvent, CloseReason, Lifecycle, RawSnapshot, RawUpdate, ServerFrame,
};
use sync_machine::replica::mock::MockReplica;

#[derive(Default)]
struct Recorder {
    sends: Mutex<Vec<(u64, &'static str)>>,
}

struct Store;
impl DocStore for Store {
    async fn load(&self, _doc: &DocId) -> Result<Option<RawSnapshot>, StoreError> {
        Ok(Some(RawSnapshot::from(&b"base"[..])))
    }
    async fn append_ops(&self, _doc: &DocId, _ops: &[(u64, RawUpdate)]) -> Result<(), StoreError> {
        Ok(())
    }
    async fn store_snapshot(
        &self,
        _doc: &DocId,
        _snapshot: RawSnapshot,
        _through_seq: u64,
    ) -> Result<(), StoreError> {
        Ok(())
    }
}

impl ClientSink for Recorder {
    async fn send(&self, conn: ConnId, frame: ServerFrame) {
        let kind = match frame {
            ServerFrame::InitialSync { .. } => "initial-sync",
            ServerFrame::PresenceLeft { .. } => "presence-left",
            _ => "other",
        };
        self.sends.lock().unwrap().push((conn.0, kind));
    }
    async fn close(&self, _conn: ConnId, _reason: CloseReason) {}
}

impl DocEvents for Recorder {
    async fn lifecycle(&self, _doc: &DocId, _event: Lifecycle) {}
    async fn blame(&self, _doc: &DocId, _events: Vec<BlameEvent>) {}
    async fn peer_mapping(&self, _doc: &DocId, _peer_id: u64, _user_id: String) {}
}

fn caps() -> Caps {
    Caps {
        can_edit: true,
        user_id: None,
    }
}

async fn settle() {
    for _ in 0..32 {
        tokio::task::yield_now().await;
    }
}

#[tokio::test(start_paused = true)]
async fn subscribe_spawns_and_disconnect_detaches_everywhere() {
    let recorder = Arc::new(Recorder::default());
    let mut supervisor = Supervisor::<MockReplica, _, _, _>::new(
        Arc::new(Store),
        Arc::clone(&recorder),
        Arc::clone(&recorder),
    );

    supervisor
        .subscribe(ConnId(1), DocId("a".into()), caps())
        .await;
    supervisor
        .subscribe(ConnId(1), DocId("b".into()), caps())
        .await;
    settle().await;
    assert_eq!(supervisor.resident_docs(), 2);
    assert_eq!(recorder.sends.lock().unwrap().len(), 2); // two initial syncs

    supervisor.disconnected(ConnId(1)).await;
    settle().await;
    // Both actors are now peerless; after the idle window they exit.
    tokio::time::advance(IDLE_EVICT).await;
    settle().await;

    // A late frame routes nowhere (dead mailbox, no respawn)...
    supervisor
        .frame(ConnId(1), DocId("a".into()), ClientFrame::RequestSnapshot)
        .await;
    settle().await;
    let count_before = recorder.sends.lock().unwrap().len();
    assert_eq!(count_before, 2);

    // ...but a fresh subscribe respawns the actor and reloads.
    supervisor
        .subscribe(ConnId(2), DocId("a".into()), caps())
        .await;
    settle().await;
    assert_eq!(recorder.sends.lock().unwrap().len(), 3);
}
