//! Test harness: drive one machine and make transitions easy to assert.

use crate::machine::DocMachine;
use crate::model::{
    Caps, ClientFrame, ConnId, Effect, Input, PersistToken, RawSnapshot, RawUpdate, ServerFrame,
    TimerToken,
};
use crate::replica::mock::MockReplica;

/// A [`DocMachine<MockReplica>`] plus feed/inspect helpers.
pub(crate) struct Harness {
    pub machine: DocMachine<MockReplica>,
}

impl Harness {
    pub fn new() -> Self {
        Self {
            machine: DocMachine::new(),
        }
    }

    /// Feed one input and return the effects it produced.
    pub fn feed(&mut self, input: Input) -> Vec<Effect> {
        let mut out = Vec::new();
        self.machine.handle(input, &mut out);
        out
    }

    /// A machine already attached (edit caps) and loaded from `snapshot`,
    /// with the setup effects discarded. Returns the harness and the conn.
    pub fn ready(snapshot: &[u8]) -> (Self, ConnId) {
        let mut harness = Self::new();
        let conn = ConnId(1);
        harness.feed(Input::PeerAttached {
            conn,
            caps: edit_caps(),
        });
        harness.feed(Input::Loaded {
            snapshot: Some(RawSnapshot::from(snapshot)),
        });
        (harness, conn)
    }
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
