use crate::machine::DocMachine;
use crate::model::{
    Caps, ClientFrame, ConnId, Effect, Input, RawCursor, RawSnapshot, RawUpdate, ServerFrame,
};
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
fn request_since_echoes_the_callers_cursor_verbatim() {
    let mut machine = ready_machine();
    let actions = machine.handle(Input::Frame {
        conn: C1,
        frame: ClientFrame::RequestSince {
            cursor: RawCursor::from(&b"vv-bytes"[..]),
        },
    });
    assert_eq!(
        actions,
        vec![Effect::Send {
            conn: C1,
            frame: ServerFrame::Since {
                update: RawUpdate::from(&b"diff-since[vv-bytes]"[..]),
                cursor: RawCursor::from(&b"vv-bytes"[..]),
            }
        }]
    );
}

#[test]
fn register_peer_records_the_user_mapping_once() {
    let mut machine = ready_machine();
    let actions = machine.handle(Input::Frame {
        conn: C1,
        frame: ClientFrame::RegisterPeer { peer_id: 42 },
    });
    assert_eq!(
        actions,
        vec![Effect::RecordPeerMapping {
            peer_id: 42,
            user_id: MacroUserIdStr::try_from("macro|user-1@test.com".to_string()).unwrap(),
        }]
    );
    // Duplicate registration is a no-op.
    let actions = machine.handle(Input::Frame {
        conn: C1,
        frame: ClientFrame::RegisterPeer { peer_id: 42 },
    });
    assert!(actions.is_empty());
}
