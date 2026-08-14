use crate::machine::DocMachine;
use crate::model::{Caps, ClientFrame, ConnId, Effect, Input, Lifecycle, RawSnapshot, RawUpdate};
use crate::replica::mock::MockReplica;
use macro_user_id::user_id::MacroUserIdStr;

const C1: ConnId = ConnId(1);

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
fn compaction_debounce_persists_a_snapshot_then_reports_edited() {
    let mut machine = ready_machine();
    let actions = machine.handle(Input::Frame {
        conn: C1,
        frame: ClientFrame::Update {
            updates: vec![RawUpdate::from(&b"a"[..])],
            id: "op-1".into(),
        },
    });
    let debounce = actions
        .iter()
        .find_map(|action| match action {
            Effect::ScheduleTimer { token, .. } => Some(*token),
            _ => None,
        })
        .expect("debounce timer");

    let actions = machine.handle(Input::TimerFired { token: debounce });
    assert!(matches!(
        actions[..],
        [Effect::PersistSnapshot { through_seq: 1, .. }]
    ));
    let token = actions
        .iter()
        .find_map(|action| match action {
            Effect::PersistSnapshot { token, .. } => Some(*token),
            _ => None,
        })
        .unwrap();

    let actions = machine.handle(Input::SnapshotPersisted { token });
    assert_eq!(
        actions,
        vec![Effect::Lifecycle {
            event: Lifecycle::Edited
        }]
    );
}

#[test]
fn a_second_compaction_does_not_start_while_one_is_in_flight() {
    let mut machine = ready_machine();
    let actions = machine.handle(Input::Frame {
        conn: C1,
        frame: ClientFrame::Update {
            updates: vec![RawUpdate::from(&b"a"[..])],
            id: "op-1".into(),
        },
    });
    let debounce = actions
        .iter()
        .find_map(|action| match action {
            Effect::ScheduleTimer { token, .. } => Some(*token),
            _ => None,
        })
        .unwrap();
    let actions = machine.handle(Input::TimerFired { token: debounce });
    assert!(matches!(actions[..], [Effect::PersistSnapshot { .. }]));

    // More edits arrive; the debounce re-arms, fires — but the snapshot
    // persist is still in flight, so no second PersistSnapshot is emitted.
    let actions = machine.handle(Input::Frame {
        conn: C1,
        frame: ClientFrame::Update {
            updates: vec![RawUpdate::from(&b"b"[..])],
            id: "op-2".into(),
        },
    });
    let debounce = actions
        .iter()
        .find_map(|action| match action {
            Effect::ScheduleTimer { token, .. } => Some(*token),
            _ => None,
        })
        .unwrap();
    let actions = machine.handle(Input::TimerFired { token: debounce });
    assert!(
        !actions
            .iter()
            .any(|action| matches!(action, Effect::PersistSnapshot { .. }))
    );
}
