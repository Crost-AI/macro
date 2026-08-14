//! Narrated end-to-end flows through the [`ConnManager`] on the mock replica.
//!
//! Run with `cargo run -p sync_machine --example flows`. Each step prints the
//! input fed to the manager and the effects it emitted — the exact
//! conversation the pass-2 runtime will have with storage, timers, and the
//! gateway sink, with every byte faked and every "await" a printed line.

use sync_machine::manager::{ConnManager, ManagerEffect, ManagerInput};
use sync_machine::model::{
    Caps, ClientFrame, ConnId, DocId, Effect, PersistToken, RawCursor, RawPresence, RawSnapshot,
    RawUpdate, ServerFrame, TimerToken,
};
use sync_machine::replica::mock::MockReplica;

fn main() {
    two_users_edit_a_document();
    reconnect_and_catch_up();
    store_outage_and_recovery();
    document_that_never_existed();
}

// ── flow 1 ────────────────────────────────────────────────────────────────

fn two_users_edit_a_document() {
    banner("flow 1: two users edit a document, then leave");
    let mut flow = Flow::new();
    let (alice, bob) = (ConnId(1), ConnId(2));
    let doc = DocId("doc-a".into());

    flow.step(
        "alice subscribes (first touch: the document must be loaded)",
        ManagerInput::Subscribe {
            conn: alice,
            doc: doc.clone(),
            caps: edit_caps("alice"),
        },
    );
    flow.step(
        "bob subscribes while the load is still in flight",
        ManagerInput::Subscribe {
            conn: bob,
            doc: doc.clone(),
            caps: edit_caps("bob"),
        },
    );
    flow.step(
        "the store answers: both waiting peers get their initial sync",
        ManagerInput::Loaded {
            doc: doc.clone(),
            snapshot: Some(RawSnapshot::from(&b"stored-snapshot"[..])),
        },
    );
    flow.step(
        "alice registers her CRDT peer id (binds edits to her user)",
        ManagerInput::Frame {
            conn: alice,
            doc: doc.clone(),
            frame: ClientFrame::RegisterPeer { peer_id: 11 },
        },
    );
    flow.step(
        "alice edits: applied + persistence requested — NO ack, NO broadcast yet",
        ManagerInput::Frame {
            conn: alice,
            doc: doc.clone(),
            frame: ClientFrame::Update {
                updates: vec![RawUpdate::from(&b"insert 'hello'"[..])],
                id: "op-1".into(),
            },
        },
    );
    let persist = flow.last_persist_ops();
    let debounce = flow.last_timer();
    flow.step(
        "the op log write commits: ack to alice, THEN broadcast to bob",
        ManagerInput::OpsPersisted {
            doc: doc.clone(),
            token: persist,
            through_seq: 1,
        },
    );
    flow.step(
        "alice moves her cursor (presence is ephemeral: relayed, never persisted)",
        ManagerInput::Frame {
            conn: alice,
            doc: doc.clone(),
            frame: ClientFrame::Presence {
                payload: RawPresence::from(&b"alice@line3"[..]),
            },
        },
    );
    flow.step(
        "the 5s compaction debounce fires: fold the op into a fresh snapshot",
        ManagerInput::TimerFired { token: debounce },
    );
    let snapshot_persist = flow.last_persist_snapshot();
    flow.step(
        "the snapshot commits: the edit is reported to the product (Edited)",
        ManagerInput::SnapshotPersisted {
            doc: doc.clone(),
            token: snapshot_persist,
        },
    );
    flow.step(
        "bob closes the tab",
        ManagerInput::Unsubscribe {
            conn: bob,
            doc: doc.clone(),
        },
    );
    flow.step(
        "alice's connection drops entirely: last peer leaves, idle timer armed",
        ManagerInput::Disconnected { conn: alice },
    );
    let idle = flow.last_timer();
    flow.step(
        "a minute later, nothing is dirty: the machine asks to be evicted",
        ManagerInput::TimerFired { token: idle },
    );
    println!(
        "  resident documents now: {}\n",
        flow.manager.resident_docs()
    );
}

// ── flow 2 ────────────────────────────────────────────────────────────────

fn reconnect_and_catch_up() {
    banner("flow 2: a client reconnects and catches up from its cursor");
    let mut flow = Flow::new();
    let carol = ConnId(3);
    let doc = DocId("doc-b".into());

    flow.step(
        "carol re-subscribes after a network blip",
        ManagerInput::Subscribe {
            conn: carol,
            doc: doc.clone(),
            caps: edit_caps("carol"),
        },
    );
    flow.step(
        "the document loads (it had state from her earlier session)",
        ManagerInput::Loaded {
            doc: doc.clone(),
            snapshot: Some(RawSnapshot::from(&b"earlier-state"[..])),
        },
    );
    flow.step(
        "carol asks for everything since her last-known cursor; the reply \
         echoes her cursor bytes verbatim so her client can correlate it",
        ManagerInput::Frame {
            conn: carol,
            doc: doc.clone(),
            frame: ClientFrame::RequestSince {
                cursor: RawCursor::from(&b"carol-vv"[..]),
            },
        },
    );
}

// ── flow 3 ────────────────────────────────────────────────────────────────

fn store_outage_and_recovery() {
    banner("flow 3: the store fails mid-session and recovers");
    let mut flow = Flow::new();
    let dave = ConnId(4);
    let doc = DocId("doc-c".into());

    flow.step(
        "dave subscribes",
        ManagerInput::Subscribe {
            conn: dave,
            doc: doc.clone(),
            caps: edit_caps("dave"),
        },
    );
    flow.step(
        "loaded",
        ManagerInput::Loaded {
            doc: doc.clone(),
            snapshot: Some(RawSnapshot::from(&b"base"[..])),
        },
    );
    flow.step(
        "dave edits",
        ManagerInput::Frame {
            conn: dave,
            doc: doc.clone(),
            frame: ClientFrame::Update {
                updates: vec![RawUpdate::from(&b"edit"[..])],
                id: "op-9".into(),
            },
        },
    );
    let persist = flow.last_persist_ops();
    flow.step(
        "the op-log write FAILS: no ack (dave's client will wait), retry scheduled",
        ManagerInput::PersistFailed {
            doc: doc.clone(),
            token: persist,
        },
    );
    let retry = flow.last_timer();
    flow.step(
        "the retry timer fires: the unpersisted tail is re-sent to the store",
        ManagerInput::TimerFired { token: retry },
    );
    let persist = flow.last_persist_ops();
    flow.step(
        "this time it commits: the ack finally reaches dave",
        ManagerInput::OpsPersisted {
            doc: doc.clone(),
            token: persist,
            through_seq: 1,
        },
    );
}

// ── flow 4 ────────────────────────────────────────────────────────────────

fn document_that_never_existed() {
    banner("flow 4: subscribing to a document with no stored state");
    let mut flow = Flow::new();
    let erin = ConnId(5);
    let doc = DocId("doc-new".into());

    flow.step(
        "erin subscribes to a brand-new document",
        ManagerInput::Subscribe {
            conn: erin,
            doc: doc.clone(),
            caps: edit_caps("erin"),
        },
    );
    flow.step(
        "the store has nothing: an empty document is materialized \
         (create-default-state, like the deployed service)",
        ManagerInput::Loaded {
            doc: doc.clone(),
            snapshot: None,
        },
    );
}

// ── the driver ────────────────────────────────────────────────────────────

struct Flow {
    manager: ConnManager<MockReplica>,
    effects: Vec<ManagerEffect>,
}

impl Flow {
    fn new() -> Self {
        Self {
            manager: ConnManager::new(),
            effects: Vec::new(),
        }
    }

    fn step(&mut self, label: &str, input: ManagerInput) {
        println!("→ {label}");
        println!("    input:  {}", describe_input(&input));
        self.effects.clear();
        self.manager.handle(input, &mut self.effects);
        if self.effects.is_empty() {
            println!("    effects: (none)");
        }
        for effect in &self.effects {
            println!(
                "    effect: [{}] {}",
                effect.doc.as_str(),
                describe_effect(&effect.effect)
            );
        }
        println!();
    }

    fn last_timer(&self) -> TimerToken {
        self.effects
            .iter()
            .rev()
            .find_map(|e| match e.effect {
                Effect::ScheduleTimer { token, .. } => Some(token),
                _ => None,
            })
            .expect("a timer was scheduled")
    }

    fn last_persist_ops(&self) -> PersistToken {
        self.effects
            .iter()
            .rev()
            .find_map(|e| match e.effect {
                Effect::PersistOps { token, .. } => Some(token),
                _ => None,
            })
            .expect("a PersistOps was emitted")
    }

    fn last_persist_snapshot(&self) -> PersistToken {
        self.effects
            .iter()
            .rev()
            .find_map(|e| match e.effect {
                Effect::PersistSnapshot { token, .. } => Some(token),
                _ => None,
            })
            .expect("a PersistSnapshot was emitted")
    }
}

fn edit_caps(user: &str) -> Caps {
    Caps {
        can_edit: true,
        user_id: Some(user.to_string()),
    }
}

fn banner(title: &str) {
    println!("──────────────────────────────────────────────────────");
    println!("{title}");
    println!("──────────────────────────────────────────────────────");
}

fn describe_input(input: &ManagerInput) -> String {
    match input {
        ManagerInput::Subscribe { conn, doc, caps } => format!(
            "Subscribe(conn {}, doc {}, user {:?}, can_edit {})",
            conn.0,
            doc.as_str(),
            caps.user_id.as_deref().unwrap_or("-"),
            caps.can_edit
        ),
        ManagerInput::Unsubscribe { conn, doc } => {
            format!("Unsubscribe(conn {}, doc {})", conn.0, doc.as_str())
        }
        ManagerInput::Disconnected { conn } => format!("Disconnected(conn {})", conn.0),
        ManagerInput::Frame { conn, frame, .. } => {
            format!("Frame(conn {}, {})", conn.0, describe_client_frame(frame))
        }
        ManagerInput::TimerFired { token } => format!("TimerFired(#{})", token.0),
        ManagerInput::Loaded { snapshot, .. } => match snapshot {
            Some(bytes) => format!("Loaded({}B snapshot)", bytes.as_slice().len()),
            None => "Loaded(nothing stored)".into(),
        },
        ManagerInput::LoadFailed { error, .. } => format!("LoadFailed({error})"),
        ManagerInput::OpsPersisted {
            token, through_seq, ..
        } => {
            format!("OpsPersisted(#{}, through seq {through_seq})", token.0)
        }
        ManagerInput::SnapshotPersisted { token, .. } => {
            format!("SnapshotPersisted(#{})", token.0)
        }
        ManagerInput::PersistFailed { token, .. } => format!("PersistFailed(#{})", token.0),
    }
}

fn describe_client_frame(frame: &ClientFrame) -> String {
    match frame {
        ClientFrame::Update { updates, id } => {
            format!("Update({} update(s), id {id})", updates.len())
        }
        ClientFrame::Presence { .. } => "Presence".into(),
        ClientFrame::RequestSince { .. } => "RequestSince".into(),
        ClientFrame::RequestSnapshot => "RequestSnapshot".into(),
        ClientFrame::RegisterPeer { peer_id } => format!("RegisterPeer({peer_id})"),
    }
}

fn describe_effect(effect: &Effect) -> String {
    match effect {
        Effect::Send { conn, frame } => {
            format!("Send(conn {}) ← {}", conn.0, describe_server_frame(frame))
        }
        Effect::Broadcast { except, frame } => format!(
            "Broadcast(all except conn {}) ← {}",
            except.0,
            describe_server_frame(frame)
        ),
        Effect::Close { conn, reason } => format!("Close(conn {}, {reason:?})", conn.0),
        Effect::ScheduleTimer { token, after_ms } => {
            format!("ScheduleTimer(#{}, {after_ms}ms)", token.0)
        }
        Effect::Load => "Load — fetch the stored snapshot".into(),
        Effect::PersistOps {
            token,
            ops,
            through_seq,
        } => format!(
            "PersistOps(#{}, {} op(s), through seq {through_seq})",
            token.0,
            ops.len()
        ),
        Effect::PersistSnapshot {
            token,
            snapshot,
            through_seq,
        } => format!(
            "PersistSnapshot(#{}, {}B, covers through seq {through_seq})",
            token.0,
            snapshot.as_slice().len()
        ),
        Effect::RecordBlame { events } => format!("RecordBlame({} row(s))", events.len()),
        Effect::RecordPeerMapping { peer_id, user_id } => {
            format!("RecordPeerMapping(peer {peer_id} → {user_id})")
        }
        Effect::Lifecycle { event } => format!("Lifecycle({event:?})"),
        Effect::Evict => "Evict — drop this machine".into(),
    }
}

fn describe_server_frame(frame: &ServerFrame) -> String {
    match frame {
        ServerFrame::InitialSync { snapshot, presence } => format!(
            "InitialSync({}B snapshot, {} presence payload(s))",
            snapshot.as_slice().len(),
            presence.len()
        ),
        ServerFrame::Update { .. } => "Update".into(),
        ServerFrame::Presence { .. } => "Presence".into(),
        ServerFrame::PresenceLeft { peer_ids } => format!("PresenceLeft({peer_ids:?})"),
        ServerFrame::Snapshot { snapshot } => {
            format!("Snapshot({}B)", snapshot.as_slice().len())
        }
        ServerFrame::Ack { id } => format!("Ack({id})"),
        ServerFrame::Since { .. } => "Since(diff + echoed cursor)".into(),
    }
}
