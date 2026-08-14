use crate::harness::{
    acks, edit_caps, persist_ops_token, persist_snapshot_token, ready, scheduled_timer, update,
    user_id, view_caps,
};
use crate::machine::DocMachine;
use crate::model::{
    ClientFrame, CloseReason, ConnId, Effect, Input, Lifecycle, RawPresence, RawSnapshot,
    RawUpdate, ServerFrame,
};
use crate::replica::mock::MockReplica;

const C1: ConnId = ConnId(1);
const C2: ConnId = ConnId(2);

// ── load path (table rows 1–6) ───────────────────────────────────────────

#[test]
fn first_attach_emits_load_and_defers_initial_sync() {
    let mut h = DocMachine::<MockReplica>::new();
    let fx = h.handle(Input::PeerAttached {
        conn: C1,
        caps: edit_caps(),
    });
    assert_eq!(fx, vec![Effect::Load]);
}

#[test]
fn frames_during_loading_queue_and_replay_in_order_after_loaded() {
    let mut h = DocMachine::<MockReplica>::new();
    h.handle(Input::PeerAttached {
        conn: C1,
        caps: edit_caps(),
    });
    assert!(
        h.handle(Input::Frame {
            conn: C1,
            frame: update(b"early-1", "op-1"),
        })
        .is_empty()
    );
    assert!(
        h.handle(Input::Frame {
            conn: C1,
            frame: update(b"early-2", "op-2"),
        })
        .is_empty()
    );

    let fx = h.handle(Input::Loaded {
        snapshot: Some(RawSnapshot::from(&b"base"[..])),
    });

    // Initial sync + FirstJoin first, then the replayed updates' effects.
    assert!(matches!(
        fx[0],
        Effect::Send {
            conn: ConnId(1),
            frame: ServerFrame::InitialSync { .. }
        }
    ));
    assert!(matches!(
        fx[1],
        Effect::Lifecycle {
            event: Lifecycle::FirstJoin
        }
    ));
    // Both queued updates were applied, in order.
    assert_eq!(
        h.replica().unwrap().applied,
        vec![b"early-1".to_vec(), b"early-2".to_vec()],
    );
    // And persisted: one in-flight request for seq 1, the second queued
    // behind it (single in-flight ops persist).
    let persists: Vec<_> = fx
        .iter()
        .filter(|e| matches!(e, Effect::PersistOps { .. }))
        .collect();
    assert_eq!(persists.len(), 1);
}

#[test]
fn loaded_none_creates_an_empty_document() {
    // Matches the deployed DO (`create-default-state`): subscribing to a
    // never-persisted document materializes an empty one.
    let mut h = DocMachine::<MockReplica>::new();
    h.handle(Input::PeerAttached {
        conn: C1,
        caps: edit_caps(),
    });
    let fx = h.handle(Input::Loaded { snapshot: None });
    assert!(matches!(
        fx[0],
        Effect::Send {
            frame: ServerFrame::InitialSync { .. },
            ..
        }
    ));
    assert!(h.replica().unwrap().loaded_from.is_none());
}

#[test]
fn load_failed_breaks_the_machine_and_closes_everyone() {
    let mut h = DocMachine::<MockReplica>::new();
    h.handle(Input::PeerAttached {
        conn: C1,
        caps: edit_caps(),
    });
    h.handle(Input::PeerAttached {
        conn: C2,
        caps: view_caps(),
    });
    let fx = h.handle(Input::LoadFailed {
        error: "store down".into(),
    });
    assert_eq!(
        fx,
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
    let fx = h.handle(Input::PeerAttached {
        conn: C1,
        caps: edit_caps(),
    });
    assert_eq!(
        fx,
        vec![Effect::Close {
            conn: C1,
            reason: CloseReason::LoadFailed
        }]
    );
}

#[test]
fn stale_loaded_after_ready_is_ignored() {
    let (mut h, _c1) = ready(b"base");
    let fx = h.handle(Input::Loaded {
        snapshot: Some(RawSnapshot::from(&b"other"[..])),
    });
    assert!(fx.is_empty());
    assert_eq!(h.replica().unwrap().loaded_from, Some(b"base".to_vec()));
}

// ── attach when ready (row 7) ────────────────────────────────────────────

#[test]
fn attach_when_ready_gets_immediate_initial_sync() {
    let (mut h, _c1) = ready(b"base");
    let fx = h.handle(Input::PeerAttached {
        conn: C2,
        caps: view_caps(),
    });
    // Second peer: no FirstJoin.
    assert_eq!(fx.len(), 1);
    assert!(matches!(
        fx[0],
        Effect::Send {
            conn: ConnId(2),
            frame: ServerFrame::InitialSync { .. }
        }
    ));
}

// ── updates, persistence, acks (rows 8–12) ───────────────────────────────

#[test]
fn update_order_is_apply_persist_ack_broadcast() {
    let (mut h, c1) = ready(b"base");
    h.handle(Input::Frame {
        conn: c1,
        frame: ClientFrame::RegisterPeer { peer_id: 7 },
    });

    let fx = h.handle(Input::Frame {
        conn: c1,
        frame: update(b"edit", "op-1"),
    });

    assert!(matches!(fx[0], Effect::PersistOps { through_seq: 1, .. }));
    assert!(fx.iter().any(|e| matches!(
        e,
        Effect::RecordBlame { events } if events.len() == 1 && events[0].peer_id == 7
    )));
    scheduled_timer(&fx); // the compaction debounce
    // Nothing is durable yet: no ack AND no broadcast — peers must never see
    // an op a crash could still erase from the log.
    assert!(acks(&fx).is_empty());
    assert!(!fx.iter().any(|e| matches!(e, Effect::Broadcast { .. })));

    // Durability releases the ack first, then the broadcast, in that order.
    let token = persist_ops_token(&fx);
    let fx = h.handle(Input::OpsPersisted {
        token,
        through_seq: 1,
    });
    let ack_at = fx
        .iter()
        .position(|e| {
            matches!(
                e,
                Effect::Send {
                    frame: ServerFrame::Ack { .. },
                    ..
                }
            )
        })
        .expect("ack after durability");
    let broadcast_at = fx
        .iter()
        .position(|e| {
            matches!(
                e,
                Effect::Broadcast {
                    except: ConnId(1),
                    frame: ServerFrame::Update { .. }
                }
            )
        })
        .expect("broadcast after durability");
    assert!(ack_at < broadcast_at);
}

#[test]
fn acks_release_only_after_ops_persisted() {
    let (mut h, c1) = ready(b"base");
    let fx1 = h.handle(Input::Frame {
        conn: c1,
        frame: update(b"a", "op-1"),
    });
    let token = persist_ops_token(&fx1);

    // A second batch while the first persist is in flight: no new PersistOps
    // (single in-flight), no acks.
    let fx2 = h.handle(Input::Frame {
        conn: c1,
        frame: update(b"b", "op-2"),
    });
    assert!(!fx2.iter().any(|e| matches!(e, Effect::PersistOps { .. })));
    assert!(acks(&fx2).is_empty());

    // First completion: acks op-1 (seq 1) and emits the follow-up persist
    // for seq 2.
    let fx3 = h.handle(Input::OpsPersisted {
        token,
        through_seq: 1,
    });
    assert_eq!(acks(&fx3), vec!["op-1".to_string()]);
    let token2 = persist_ops_token(&fx3);

    let fx4 = h.handle(Input::OpsPersisted {
        token: token2,
        through_seq: 2,
    });
    assert_eq!(acks(&fx4), vec!["op-2".to_string()]);
}

#[test]
fn viewer_updates_are_dropped_silently() {
    let (mut h, _c1) = ready(b"base");
    h.handle(Input::PeerAttached {
        conn: C2,
        caps: view_caps(),
    });
    let fx = h.handle(Input::Frame {
        conn: C2,
        frame: update(b"sneaky", "op-x"),
    });
    assert!(fx.is_empty());
    assert_eq!(h.replica().unwrap().applied, Vec::<Vec<u8>>::new());
}

#[test]
fn poison_update_closes_the_sender_and_never_reaches_the_log() {
    let (mut h, c1) = ready(b"base");
    let fx = h.handle(Input::Frame {
        conn: c1,
        frame: ClientFrame::Update {
            updates: vec![
                RawUpdate::from(&b"fine"[..]),
                RawUpdate::from(&b"__poison__"[..]),
                RawUpdate::from(&b"after"[..]),
            ],
            id: "op-1".into(),
        },
    });
    // The good op before the poison stands (it's already in the replica);
    // the poison and everything after are dropped; the sender is closed.
    assert_eq!(h.replica().unwrap().applied, vec![b"fine".to_vec()]);
    assert!(fx.iter().any(|e| matches!(
        e,
        Effect::Close {
            conn: ConnId(1),
            reason: CloseReason::Protocol
        }
    )));
    let persisted: Vec<_> = fx
        .iter()
        .filter_map(|e| match e {
            Effect::PersistOps { ops, .. } => Some(ops.clone()),
            _ => None,
        })
        .flatten()
        .collect();
    assert_eq!(persisted, vec![(1, RawUpdate::from(&b"fine"[..]))]);
}

#[test]
fn persist_failure_schedules_retry_and_retry_resends_the_tail() {
    let (mut h, c1) = ready(b"base");
    let fx = h.handle(Input::Frame {
        conn: c1,
        frame: update(b"a", "op-1"),
    });
    let token = persist_ops_token(&fx);

    let fx = h.handle(Input::PersistFailed { token });
    let retry = scheduled_timer(&fx);
    assert!(acks(&fx).is_empty());

    let fx = h.handle(Input::TimerFired { token: retry });
    let retry_token = persist_ops_token(&fx);
    let fx = h.handle(Input::OpsPersisted {
        token: retry_token,
        through_seq: 1,
    });
    // The ack survives the failed attempt and flows after the retry.
    assert_eq!(acks(&fx), vec!["op-1".to_string()]);
}

// ── presence (row 13, 18) ────────────────────────────────────────────────

#[test]
fn presence_rebroadcasts_and_feeds_initial_sync() {
    let (mut h, c1) = ready(b"base");
    let fx = h.handle(Input::Frame {
        conn: c1,
        frame: ClientFrame::Presence {
            payload: RawPresence::from(&b"cursor@3"[..]),
        },
    });
    assert_eq!(
        fx,
        vec![Effect::Broadcast {
            except: c1,
            frame: ServerFrame::Presence {
                payload: RawPresence::from(&b"cursor@3"[..])
            }
        }]
    );

    // A later attach sees the stored payload in its initial sync.
    let fx = h.handle(Input::PeerAttached {
        conn: C2,
        caps: view_caps(),
    });
    assert!(matches!(
        &fx[0],
        Effect::Send {
            frame: ServerFrame::InitialSync { presence, .. },
            ..
        } if presence == &vec![RawPresence::from(&b"cursor@3"[..])]
    ));
}

// ── request/response frames (rows 14–15) ─────────────────────────────────

#[test]
fn request_since_echoes_the_callers_cursor_verbatim() {
    let (mut h, c1) = ready(b"base");
    let fx = h.handle(Input::Frame {
        conn: c1,
        frame: ClientFrame::RequestSince {
            cursor: crate::model::RawCursor::from(&b"vv-bytes"[..]),
        },
    });
    assert_eq!(
        fx,
        vec![Effect::Send {
            conn: c1,
            frame: ServerFrame::Since {
                update: RawUpdate::from(&b"diff-since[vv-bytes]"[..]),
                cursor: crate::model::RawCursor::from(&b"vv-bytes"[..]),
            }
        }]
    );
}

#[test]
fn register_peer_records_the_user_mapping_once() {
    let (mut h, c1) = ready(b"base");
    let fx = h.handle(Input::Frame {
        conn: c1,
        frame: ClientFrame::RegisterPeer { peer_id: 42 },
    });
    assert_eq!(
        fx,
        vec![Effect::RecordPeerMapping {
            peer_id: 42,
            user_id: user_id("macro|user-1@test.com"),
        }]
    );
    // Duplicate registration is a no-op.
    let fx = h.handle(Input::Frame {
        conn: c1,
        frame: ClientFrame::RegisterPeer { peer_id: 42 },
    });
    assert!(fx.is_empty());
}

#[test]
fn frames_from_unattached_conns_are_closed() {
    let (mut h, _c1) = ready(b"base");
    let fx = h.handle(Input::Frame {
        conn: ConnId(99),
        frame: ClientFrame::RequestSnapshot,
    });
    assert_eq!(
        fx,
        vec![Effect::Close {
            conn: ConnId(99),
            reason: CloseReason::NotAttached
        }]
    );
}

// ── compaction (rows 16–17) ──────────────────────────────────────────────

#[test]
fn compaction_debounce_persists_a_snapshot_then_reports_edited() {
    let (mut h, c1) = ready(b"base");
    let fx = h.handle(Input::Frame {
        conn: c1,
        frame: update(b"a", "op-1"),
    });
    let debounce = scheduled_timer(&fx);

    let fx = h.handle(Input::TimerFired { token: debounce });
    assert!(matches!(
        fx[..],
        [Effect::PersistSnapshot { through_seq: 1, .. }]
    ));
    let snap_token = persist_snapshot_token(&fx);

    let fx = h.handle(Input::SnapshotPersisted { token: snap_token });
    assert_eq!(
        fx,
        vec![Effect::Lifecycle {
            event: Lifecycle::Edited
        }]
    );
}

#[test]
fn a_second_compaction_does_not_start_while_one_is_in_flight() {
    let (mut h, c1) = ready(b"base");
    let fx = h.handle(Input::Frame {
        conn: c1,
        frame: update(b"a", "op-1"),
    });
    let debounce = scheduled_timer(&fx);
    let fx = h.handle(Input::TimerFired { token: debounce });
    assert!(matches!(fx[..], [Effect::PersistSnapshot { .. }]));

    // More edits arrive; the debounce re-arms, fires — but the snapshot
    // persist is still in flight, so no second PersistSnapshot is emitted.
    let fx = h.handle(Input::Frame {
        conn: c1,
        frame: update(b"b", "op-2"),
    });
    let debounce = scheduled_timer(&fx);
    let fx = h.handle(Input::TimerFired { token: debounce });
    assert!(
        !fx.iter()
            .any(|e| matches!(e, Effect::PersistSnapshot { .. }))
    );
}

// ── detach, idle, evict (rows 18–21) ─────────────────────────────────────

#[test]
fn last_leave_arms_idle_and_clean_idle_evicts() {
    let (mut h, c1) = ready(b"base");
    let fx = h.handle(Input::PeerDetached { conn: c1 });
    assert!(fx.iter().any(|e| matches!(
        e,
        Effect::Lifecycle {
            event: Lifecycle::LastLeave
        }
    )));
    let idle = scheduled_timer(&fx);

    let fx = h.handle(Input::TimerFired { token: idle });
    assert_eq!(fx, vec![Effect::Evict]);
}

#[test]
fn dirty_idle_compacts_first_and_evicts_on_the_next_tick() {
    let (mut h, c1) = ready(b"base");
    let fx = h.handle(Input::Frame {
        conn: c1,
        frame: update(b"a", "op-1"),
    });
    let ops_token = persist_ops_token(&fx);
    h.handle(Input::OpsPersisted {
        token: ops_token,
        through_seq: 1,
    });

    let fx = h.handle(Input::PeerDetached { conn: c1 });
    let idle = scheduled_timer(&fx);

    // Dirty at idle: compact instead of evicting, re-arm.
    let fx = h.handle(Input::TimerFired { token: idle });
    assert!(
        fx.iter()
            .any(|e| matches!(e, Effect::PersistSnapshot { .. }))
    );
    assert!(!fx.iter().any(|e| matches!(e, Effect::Evict)));
    let snap_token = persist_snapshot_token(&fx);
    let idle2 = scheduled_timer(&fx);

    h.handle(Input::SnapshotPersisted { token: snap_token });
    let fx = h.handle(Input::TimerFired { token: idle2 });
    assert_eq!(fx, vec![Effect::Evict]);
}

#[test]
fn reattach_before_idle_fires_cancels_eviction() {
    let (mut h, c1) = ready(b"base");
    let fx = h.handle(Input::PeerDetached { conn: c1 });
    let idle = scheduled_timer(&fx);

    h.handle(Input::PeerAttached {
        conn: C2,
        caps: view_caps(),
    });
    // The stale idle fire is ignored: the token was cancelled on attach.
    let fx = h.handle(Input::TimerFired { token: idle });
    assert!(fx.is_empty());
}

#[test]
fn detach_broadcasts_presence_left_for_registered_peers() {
    let (mut h, c1) = ready(b"base");
    h.handle(Input::PeerAttached {
        conn: C2,
        caps: view_caps(),
    });
    h.handle(Input::Frame {
        conn: c1,
        frame: ClientFrame::RegisterPeer { peer_id: 7 },
    });

    let fx = h.handle(Input::PeerDetached { conn: c1 });
    assert!(fx.iter().any(|e| matches!(
        e,
        Effect::Broadcast {
            except: ConnId(1),
            frame: ServerFrame::PresenceLeft { peer_ids }
        } if peer_ids == &vec![7]
    )));
    // C2 is still attached: no LastLeave, no idle timer.
    assert!(!fx.iter().any(|e| matches!(e, Effect::Lifecycle { .. })));
}
