//! Test harness: drive one machine and make transitions easy to assert.

use crate::machine::DocMachine;
use crate::model::{
    Caps, ClientFrame, ConnId, Effect, Input, PersistToken, RawSnapshot, RawUpdate, ServerFrame,
    TimerToken,
};
use crate::replica::mock::MockReplica;

/// A [`DocMachine<MockReplica>`] already attached (edit caps) and loaded from
/// `snapshot`, with the setup effects discarded. Returns the machine and the
/// attached conn.
pub(crate) fn ready(snapshot: &[u8]) -> (DocMachine<MockReplica>, ConnId) {
    let mut machine = DocMachine::new();
    let conn = ConnId(1);
    machine.handle(Input::PeerAttached {
        conn,
        caps: edit_caps(),
    });
    machine.handle(Input::Loaded {
        snapshot: Some(RawSnapshot::from(snapshot)),
    });
    (machine, conn)
}

pub(crate) fn edit_caps() -> Caps {
    Caps {
        can_edit: true,
        user_id: Some(user_id("macro|user-1@test.com")),
    }
}

pub(crate) fn view_caps() -> Caps {
    Caps {
        can_edit: false,
        user_id: Some(user_id("macro|viewer@test.com")),
    }
}

pub(crate) fn user_id(raw: &str) -> macro_user_id::user_id::MacroUserIdStr<'static> {
    macro_user_id::user_id::MacroUserIdStr::try_from(raw.to_string()).expect("valid test user id")
}

pub(crate) fn update(payload: &[u8], id: &str) -> ClientFrame {
    ClientFrame::Update {
        updates: vec![RawUpdate::from(payload)],
        id: id.to_string(),
    }
}

/// The single `ScheduleTimer` token in `effects`; panics on zero or many.
pub(crate) fn scheduled_timer(effects: &[Effect]) -> TimerToken {
    let mut tokens = effects.iter().filter_map(|effect| match effect {
        Effect::ScheduleTimer { token, .. } => Some(*token),
        _ => None,
    });
    let token = tokens.next().expect("no timer scheduled");
    assert!(tokens.next().is_none(), "more than one timer scheduled");
    token
}

/// The single `PersistOps` token in `effects`.
pub(crate) fn persist_ops_token(effects: &[Effect]) -> PersistToken {
    effects
        .iter()
        .find_map(|effect| match effect {
            Effect::PersistOps { token, .. } => Some(*token),
            _ => None,
        })
        .expect("no PersistOps emitted")
}

/// The single `PersistSnapshot` token in `effects`.
pub(crate) fn persist_snapshot_token(effects: &[Effect]) -> PersistToken {
    effects
        .iter()
        .find_map(|effect| match effect {
            Effect::PersistSnapshot { token, .. } => Some(*token),
            _ => None,
        })
        .expect("no PersistSnapshot emitted")
}

/// All acks in `effects`, in order.
pub(crate) fn acks(effects: &[Effect]) -> Vec<String> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::Send {
                frame: ServerFrame::Ack { id },
                ..
            } => Some(id.clone()),
            _ => None,
        })
        .collect()
}
