use crate::machine::DocMachine;
use crate::model::{Caps, ClientFrame, ConnId, Effect, Input, Lifecycle, RawSnapshot, RawUpdate};
use crate::replica::mock::MockReplica;
use macro_user_id::user_id::MacroUserIdStr;

const C1: ConnId = ConnId(1);
const C2: ConnId = ConnId(2);

/// A machine with C1 attached (edit caps) and loaded from `b"base"`.
fn ready_machine() -> DocMachine<MockReplica> {
    let mut machine = DocMachine::new();
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
    machine
}

#[test]
fn last_leave_arms_idle_and_clean_idle_evicts() {
    let mut machine = ready_machine();
    let actions = machine.handle(Input::PeerDetached { conn: C1 });
    assert!(actions.iter().any(|action| matches!(
        action,
        Effect::Lifecycle {
            event: Lifecycle::LastLeave
        }
    )));
    let idle = actions
        .iter()
        .find_map(|action| match action {
            Effect::ScheduleTimer { token, .. } => Some(*token),
            _ => None,
        })
        .expect("idle timer");

    let actions = machine.handle(Input::TimerFired { token: idle });
    assert_eq!(actions, vec![Effect::Evict]);
}

#[test]
fn dirty_idle_compacts_first_and_evicts_on_the_next_tick() {
    let mut machine = ready_machine();
    let actions = machine.handle(Input::Frame {
        conn: C1,
        frame: ClientFrame::Update {
            updates: vec![RawUpdate::from(&b"a"[..])],
            id: "op-1".into(),
        },
    });
    let token = actions
        .iter()
        .find_map(|action| match action {
            Effect::PersistOps { token, .. } => Some(*token),
            _ => None,
        })
        .unwrap();
    machine.handle(Input::OpsPersisted {
        token,
        through_seq: 1,
    });

    let actions = machine.handle(Input::PeerDetached { conn: C1 });
    let idle = actions
        .iter()
        .find_map(|action| match action {
            Effect::ScheduleTimer { token, .. } => Some(*token),
            _ => None,
        })
        .expect("idle timer");

    // Dirty at idle: compact instead of evicting, re-arm.
    let actions = machine.handle(Input::TimerFired { token: idle });
    assert!(
        actions
            .iter()
            .any(|action| matches!(action, Effect::PersistSnapshot { .. }))
    );
    assert!(!actions.iter().any(|action| matches!(action, Effect::Evict)));
    let snapshot_token = actions
        .iter()
        .find_map(|action| match action {
            Effect::PersistSnapshot { token, .. } => Some(*token),
            _ => None,
        })
        .unwrap();
    let idle = actions
        .iter()
        .find_map(|action| match action {
            Effect::ScheduleTimer { token, .. } => Some(*token),
            _ => None,
        })
        .expect("re-armed idle timer");

    machine.handle(Input::SnapshotPersisted {
        token: snapshot_token,
    });
    let actions = machine.handle(Input::TimerFired { token: idle });
    assert_eq!(actions, vec![Effect::Evict]);
}

#[test]
fn reattach_before_idle_fires_cancels_eviction() {
    let mut machine = ready_machine();
    let actions = machine.handle(Input::PeerDetached { conn: C1 });
    let idle = actions
        .iter()
        .find_map(|action| match action {
            Effect::ScheduleTimer { token, .. } => Some(*token),
            _ => None,
        })
        .expect("idle timer");

    machine.handle(Input::PeerAttached {
        conn: C2,
        caps: Caps {
            can_edit: false,
            user_id: None,
        },
    });
    // The stale idle fire is ignored: the token was cancelled on attach.
    let actions = machine.handle(Input::TimerFired { token: idle });
    assert!(actions.is_empty());
}
