use super::*;
use crate::harness::edit_caps;
use crate::model::{Lifecycle, RawSnapshot, RawUpdate, ServerFrame};
use crate::replica::mock::MockReplica;

fn doc(id: &str) -> DocId {
    DocId(id.to_string())
}

fn feed(manager: &mut ConnManager<MockReplica>, input: ManagerInput) -> Vec<ManagerEffect> {
    let mut out = Vec::new();
    manager.handle(input, &mut out);
    out
}

/// Bring one (conn, doc) to Ready, discarding setup effects.
fn attach_ready(manager: &mut ConnManager<MockReplica>, conn: ConnId, id: &str) {
    feed(
        manager,
        ManagerInput::Attach {
            conn,
            doc: doc(id),
            caps: edit_caps(),
        },
    );
    feed(
        manager,
        ManagerInput::Loaded {
            doc: doc(id),
            snapshot: Some(RawSnapshot::from(&b"base"[..])),
        },
    );
}

#[test]
fn attach_creates_the_machine_and_lifts_load_with_the_doc_stamped() {
    let mut manager = ConnManager::<MockReplica>::new();
    let fx = feed(
        &mut manager,
        ManagerInput::Attach {
            conn: ConnId(1),
            doc: doc("doc-a"),
            caps: edit_caps(),
        },
    );
    assert_eq!(
        fx,
        vec![ManagerEffect {
            doc: doc("doc-a"),
            effect: Effect::Load
        }]
    );
    assert_eq!(manager.resident_docs(), 1);
}

#[test]
fn frames_route_to_the_right_document() {
    let mut manager = ConnManager::<MockReplica>::new();
    attach_ready(&mut manager, ConnId(1), "doc-a");
    attach_ready(&mut manager, ConnId(1), "doc-b");

    let fx = feed(
        &mut manager,
        ManagerInput::Frame {
            conn: ConnId(1),
            doc: doc("doc-b"),
            frame: ClientFrame::RequestSnapshot,
        },
    );
    assert_eq!(fx.len(), 1);
    assert_eq!(fx[0].doc, doc("doc-b"));
    assert!(matches!(
        fx[0].effect,
        Effect::Send {
            frame: ServerFrame::Snapshot { .. },
            ..
        }
    ));
}

#[test]
fn per_doc_detaches_reach_their_documents() {
    // Socket-death fan-out is the edge's job (the router already tears down
    // each route); the manager just sees one Detach per (conn, doc).
    let mut manager = ConnManager::<MockReplica>::new();
    attach_ready(&mut manager, ConnId(1), "doc-a");
    attach_ready(&mut manager, ConnId(2), "doc-b");

    let mut fx = feed(
        &mut manager,
        ManagerInput::Detach {
            conn: ConnId(1),
            doc: doc("doc-a"),
        },
    );
    fx.extend(feed(
        &mut manager,
        ManagerInput::Detach {
            conn: ConnId(2),
            doc: doc("doc-b"),
        },
    ));
    let last_leaves: Vec<&DocId> = fx
        .iter()
        .filter(|e| {
            matches!(
                e.effect,
                Effect::Lifecycle {
                    event: Lifecycle::LastLeave
                }
            )
        })
        .map(|e| &e.doc)
        .collect();
    assert_eq!(last_leaves, vec![&doc("doc-a"), &doc("doc-b")]);
}

#[test]
fn manager_scoped_timer_tokens_route_back_to_their_document() {
    let mut manager = ConnManager::<MockReplica>::new();
    attach_ready(&mut manager, ConnId(1), "doc-a");
    attach_ready(&mut manager, ConnId(2), "doc-b");

    // Detach both; each doc arms an idle timer under a manager-scoped token.
    let fx = feed(
        &mut manager,
        ManagerInput::Detach {
            conn: ConnId(1),
            doc: doc("doc-a"),
        },
    );
    let timer_a = fx
        .iter()
        .find_map(|e| match (&e.doc, &e.effect) {
            (d, Effect::ScheduleTimer { token, .. }) if *d == doc("doc-a") => Some(*token),
            _ => None,
        })
        .expect("doc-a idle timer");
    let fx = feed(
        &mut manager,
        ManagerInput::Detach {
            conn: ConnId(2),
            doc: doc("doc-b"),
        },
    );
    let timer_b = fx
        .iter()
        .find_map(|e| match (&e.doc, &e.effect) {
            (d, Effect::ScheduleTimer { token, .. }) if *d == doc("doc-b") => Some(*token),
            _ => None,
        })
        .expect("doc-b idle timer");
    assert_ne!(timer_a, timer_b);

    // Firing doc-b's token evicts only doc-b.
    let fx = feed(&mut manager, ManagerInput::TimerFired { token: timer_b });
    assert!(fx.is_empty()); // Evict is consumed by the manager
    assert_eq!(manager.resident_docs(), 1);

    let fx = feed(&mut manager, ManagerInput::TimerFired { token: timer_a });
    assert!(fx.is_empty());
    assert_eq!(manager.resident_docs(), 0);
}

#[test]
fn eviction_drops_stale_tokens_and_late_inputs_route_nowhere() {
    let mut manager = ConnManager::<MockReplica>::new();
    attach_ready(&mut manager, ConnId(1), "doc-a");

    let fx = feed(
        &mut manager,
        ManagerInput::Detach {
            conn: ConnId(1),
            doc: doc("doc-a"),
        },
    );
    let idle = fx
        .iter()
        .find_map(|e| match &e.effect {
            Effect::ScheduleTimer { token, .. } => Some(*token),
            _ => None,
        })
        .expect("idle timer");
    feed(&mut manager, ManagerInput::TimerFired { token: idle });
    assert_eq!(manager.resident_docs(), 0);

    // A duplicate fire and a frame for the evicted doc are both harmless.
    assert!(feed(&mut manager, ManagerInput::TimerFired { token: idle }).is_empty());
    assert!(
        feed(
            &mut manager,
            ManagerInput::Frame {
                conn: ConnId(1),
                doc: doc("doc-a"),
                frame: ClientFrame::RequestSnapshot,
            },
        )
        .is_empty()
    );
}

#[test]
fn persist_completions_route_through_manager_scoped_tokens() {
    let mut manager = ConnManager::<MockReplica>::new();
    attach_ready(&mut manager, ConnId(1), "doc-a");

    let fx = feed(
        &mut manager,
        ManagerInput::Frame {
            conn: ConnId(1),
            doc: doc("doc-a"),
            frame: ClientFrame::Update {
                updates: vec![RawUpdate::from(&b"x"[..])],
                id: "op-1".into(),
            },
        },
    );
    let persist = fx
        .iter()
        .find_map(|e| match &e.effect {
            Effect::PersistOps { token, .. } => Some(*token),
            _ => None,
        })
        .expect("persist ops");

    let fx = feed(
        &mut manager,
        ManagerInput::OpsPersisted {
            doc: doc("doc-a"),
            token: persist,
            through_seq: 1,
        },
    );
    assert!(fx.iter().any(|e| matches!(
        &e.effect,
        Effect::Send {
            frame: ServerFrame::Ack { id },
            ..
        } if id == "op-1"
    )));

    // A duplicate completion is stale and ignored.
    let fx = feed(
        &mut manager,
        ManagerInput::OpsPersisted {
            doc: doc("doc-a"),
            token: persist,
            through_seq: 1,
        },
    );
    assert!(fx.is_empty());
}

#[test]
fn reattach_after_eviction_reloads_from_the_store() {
    let mut manager = ConnManager::<MockReplica>::new();
    attach_ready(&mut manager, ConnId(1), "doc-a");
    let fx = feed(
        &mut manager,
        ManagerInput::Detach {
            conn: ConnId(1),
            doc: doc("doc-a"),
        },
    );
    let idle = fx
        .iter()
        .find_map(|e| match &e.effect {
            Effect::ScheduleTimer { token, .. } => Some(*token),
            _ => None,
        })
        .expect("idle timer");
    feed(&mut manager, ManagerInput::TimerFired { token: idle });
    assert_eq!(manager.resident_docs(), 0);

    let fx = feed(
        &mut manager,
        ManagerInput::Attach {
            conn: ConnId(2),
            doc: doc("doc-a"),
            caps: edit_caps(),
        },
    );
    assert_eq!(
        fx,
        vec![ManagerEffect {
            doc: doc("doc-a"),
            effect: Effect::Load
        }]
    );
}
