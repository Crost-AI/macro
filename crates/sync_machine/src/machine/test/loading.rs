use crate::machine::DocMachine;
use crate::model::{
    Caps, CloseReason, ConnId, Effect, Input, Lifecycle, RawSnapshot, RawUpdate, ServerFrame,
};
use crate::replica::mock::MockReplica;
use macro_user_id::user_id::MacroUserIdStr;

const C1: ConnId = ConnId(1);
const C2: ConnId = ConnId(2);

#[test]
fn first_attach_emits_load_and_defers_initial_sync() {
    let mut machine = DocMachine::<MockReplica>::new();
    let actions = machine.handle(Input::PeerAttached {
        conn: C1,
        caps: Caps {
            can_edit: true,
            user_id: None,
        },
    });
    assert_eq!(actions, vec![Effect::Load]);
}

#[test]
fn frames_during_loading_queue_and_replay_in_order_after_loaded() {
    let mut machine = DocMachine::<MockReplica>::new();
    machine.handle(Input::PeerAttached {
        conn: C1,
        caps: Caps {
            can_edit: true,
            user_id: None,
        },
    });
    let actions = machine.handle(Input::Frame {
        conn: C1,
        frame: crate::model::ClientFrame::Update {
            updates: vec![RawUpdate::from(&b"early-1"[..])],
            id: "op-1".into(),
        },
    });
    assert!(actions.is_empty());
    let actions = machine.handle(Input::Frame {
        conn: C1,
        frame: crate::model::ClientFrame::Update {
            updates: vec![RawUpdate::from(&b"early-2"[..])],
            id: "op-2".into(),
        },
    });
    assert!(actions.is_empty());

    let actions = machine.handle(Input::Loaded {
        snapshot: Some(RawSnapshot::from(&b"base"[..])),
    });

    // Initial sync + FirstJoin first, then the replayed updates' effects.
    assert!(matches!(
        actions[0],
        Effect::Send {
            conn: ConnId(1),
            frame: ServerFrame::InitialSync { .. }
        }
    ));
    assert!(matches!(
        actions[1],
        Effect::Lifecycle {
            event: Lifecycle::FirstJoin
        }
    ));
    // Both queued updates were applied, in order.
    assert_eq!(
        machine.replica().unwrap().applied,
        vec![b"early-1".to_vec(), b"early-2".to_vec()],
    );
    // And persisted: one in-flight request for seq 1, the second queued
    // behind it (single in-flight ops persist).
    let persists: Vec<_> = actions
        .iter()
        .filter(|action| matches!(action, Effect::PersistOps { .. }))
        .collect();
    assert_eq!(persists.len(), 1);
}

#[test]
fn loaded_none_creates_an_empty_document() {
    // Matches the deployed DO (`create-default-state`): subscribing to a
    // never-persisted document materializes an empty one.
    let mut machine = DocMachine::<MockReplica>::new();
    machine.handle(Input::PeerAttached {
        conn: C1,
        caps: Caps {
            can_edit: true,
            user_id: None,
        },
    });
    let actions = machine.handle(Input::Loaded { snapshot: None });
    assert!(matches!(
        actions[0],
        Effect::Send {
            frame: ServerFrame::InitialSync { .. },
            ..
        }
    ));
    assert!(machine.replica().unwrap().loaded_from.is_none());
}

#[test]
fn load_failed_breaks_the_machine_and_closes_everyone() {
    let mut machine = DocMachine::<MockReplica>::new();
    machine.handle(Input::PeerAttached {
        conn: C1,
        caps: Caps {
            can_edit: true,
            user_id: None,
        },
    });
    machine.handle(Input::PeerAttached {
        conn: C2,
        caps: Caps {
            can_edit: false,
            user_id: None,
        },
    });
    let actions = machine.handle(Input::LoadFailed {
        error: "store down".into(),
    });
    assert_eq!(
        actions,
        vec![
            Effect::Close {
                conn: C1,
                reason: CloseReason::LoadFailed
            },
            Effect::Close {
                conn: C2,
                reason: CloseReason::LoadFailed
            },
        ]
    );
    // Later attaches are refused outright.
    let actions = machine.handle(Input::PeerAttached {
        conn: C1,
        caps: Caps {
            can_edit: true,
            user_id: None,
        },
    });
    assert_eq!(
        actions,
        vec![Effect::Close {
            conn: C1,
            reason: CloseReason::LoadFailed
        }]
    );
}

#[test]
fn stale_loaded_after_ready_is_ignored() {
    let mut machine = DocMachine::<MockReplica>::new();
    machine.handle(Input::PeerAttached {
        conn: C1,
        caps: Caps {
            can_edit: true,
            user_id: None,
        },
    });
    machine.handle(Input::Loaded {
        snapshot: Some(RawSnapshot::from(&b"base"[..])),
    });
    let actions = machine.handle(Input::Loaded {
        snapshot: Some(RawSnapshot::from(&b"other"[..])),
    });
    assert!(actions.is_empty());
    assert_eq!(
        machine.replica().unwrap().loaded_from,
        Some(b"base".to_vec())
    );
}

#[test]
fn attach_when_ready_gets_immediate_initial_sync() {
    let mut machine = DocMachine::<MockReplica>::new();
    machine.handle(Input::PeerAttached {
        conn: C1,
        caps: Caps {
            can_edit: true,
            user_id: Some(MacroUserIdStr::try_from("macro|user-1@test.com".to_string()).unwrap()),
        },
    });
    machine.handle(Input::Loaded {
        snapshot: Some(RawSnapshot::from(&b"base"[..])),
    });
    let actions = machine.handle(Input::PeerAttached {
        conn: C2,
        caps: Caps {
            can_edit: false,
            user_id: None,
        },
    });
    // Second peer: no FirstJoin.
    assert_eq!(actions.len(), 1);
    assert!(matches!(
        actions[0],
        Effect::Send {
            conn: ConnId(2),
            frame: ServerFrame::InitialSync { .. }
        }
    ));
}
