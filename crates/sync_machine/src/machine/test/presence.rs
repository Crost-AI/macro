use crate::machine::DocMachine;
use crate::model::{
    Caps, ClientFrame, ConnId, Effect, Input, RawPresence, RawSnapshot, ServerFrame,
};
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
fn presence_rebroadcasts_and_feeds_initial_sync() {
    let mut machine = ready_machine();
    let actions = machine.handle(Input::Frame {
        conn: C1,
        frame: ClientFrame::Presence {
            payload: RawPresence::from(&b"cursor@3"[..]),
        },
    });
    assert_eq!(
        actions,
        vec![Effect::Broadcast {
            except: C1,
            frame: ServerFrame::Presence {
                payload: RawPresence::from(&b"cursor@3"[..])
            }
        }]
    );

    // A later attach sees the stored payload in its initial sync.
    let actions = machine.handle(Input::PeerAttached {
        conn: C2,
        caps: Caps {
            can_edit: false,
            user_id: None,
        },
    });
    assert!(matches!(
        &actions[0],
        Effect::Send {
            frame: ServerFrame::InitialSync { presence, .. },
            ..
        } if presence == &vec![RawPresence::from(&b"cursor@3"[..])]
    ));
}

#[test]
fn detach_broadcasts_presence_left_for_registered_peers() {
    let mut machine = ready_machine();
    machine.handle(Input::PeerAttached {
        conn: C2,
        caps: Caps {
            can_edit: false,
            user_id: None,
        },
    });
    machine.handle(Input::Frame {
        conn: C1,
        frame: ClientFrame::RegisterPeer { peer_id: 7 },
    });

    let actions = machine.handle(Input::PeerDetached { conn: C1 });
    assert!(actions.iter().any(|action| matches!(
        action,
        Effect::Broadcast {
            except: ConnId(1),
            frame: ServerFrame::PresenceLeft { peer_ids }
        } if peer_ids == &vec![7]
    )));
    // C2 is still attached: no LastLeave, no idle timer.
    assert!(
        !actions
            .iter()
            .any(|action| matches!(action, Effect::Lifecycle { .. }))
    );
}
