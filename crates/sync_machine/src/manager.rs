//! Owns many [`DocMachine`]s and routes document-scoped events to them.
//!
//! The manager is the second (outer) pure machine: the runtime speaks only
//! [`ManagerInput`]/[`ManagerEffect`], never to a document directly. Its jobs
//! are routing, token namespacing (machine timer/persist tokens are
//! per-document; the runtime sees globally unique ones), and consuming
//! [`Effect::Evict`] by dropping the machine.
//!
//! A [`ConnId`] is attached to exactly ONE document: the edge (sync-router)
//! mints a fresh id per (client connection, document) route, so multiplexing,
//! disconnect fan-out, and resubscribe-race identity all live in one place —
//! the router's routing table — instead of being duplicated here.

#[cfg(test)]
mod test;

use crate::machine::DocMachine;
use crate::model::{
    Caps, ClientFrame, ConnId, DocId, Effect, Input, PersistToken, RawSnapshot, TimerToken,
};
use crate::replica::Replica;
use std::collections::BTreeMap;

/// Everything the runtime can tell the manager.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManagerInput {
    /// A per-document connection attached (capabilities already resolved).
    /// The machine for `doc` is created on first attach.
    Attach {
        /// The attaching connection (one document only, by construction).
        conn: ConnId,
        /// The document.
        doc: DocId,
        /// What the connection may do to it.
        caps: Caps,
    },
    /// A per-document connection detached (client unsubscribe, socket death —
    /// the edge translates both into this).
    Detach {
        /// The detaching connection.
        conn: ConnId,
        /// The document.
        doc: DocId,
    },
    /// A sync frame for one document.
    Frame {
        /// The sending connection.
        conn: ConnId,
        /// The document.
        doc: DocId,
        /// The decoded message.
        frame: ClientFrame,
    },
    /// A previously scheduled timer elapsed.
    TimerFired {
        /// The (manager-scoped) token from [`ManagerEffect::ScheduleTimer`].
        token: TimerToken,
    },
    /// Completion of a document's [`Effect::Load`].
    Loaded {
        /// The document that was loading.
        doc: DocId,
        /// The stored snapshot, or `None` for a never-persisted document.
        snapshot: Option<RawSnapshot>,
    },
    /// Completion of a document's [`Effect::Load`]: the store failed.
    LoadFailed {
        /// The document that was loading.
        doc: DocId,
        /// For logs only.
        error: String,
    },
    /// Completion of a document's [`Effect::PersistOps`].
    OpsPersisted {
        /// The document.
        doc: DocId,
        /// The (manager-scoped) request token.
        token: PersistToken,
        /// Durable through this sequence.
        through_seq: u64,
    },
    /// Completion of a document's [`Effect::PersistSnapshot`].
    SnapshotPersisted {
        /// The document.
        doc: DocId,
        /// The (manager-scoped) request token.
        token: PersistToken,
    },
    /// A document's persistence request failed.
    PersistFailed {
        /// The document.
        doc: DocId,
        /// The (manager-scoped) request token.
        token: PersistToken,
    },
}

/// A document machine's [`Effect`], stamped with its document. `Evict` never
/// appears — the manager consumes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagerEffect {
    /// The document the effect belongs to.
    pub doc: DocId,
    /// The effect itself, with timer/persist tokens rewritten to be unique
    /// across documents.
    pub effect: Effect,
}

/// See the module docs.
pub struct ConnManager<R: Replica> {
    machines: BTreeMap<DocId, DocMachine<R>>,
    /// Manager-scoped timer token → (document, the machine's own token).
    timers: BTreeMap<TimerToken, (DocId, TimerToken)>,
    /// Manager-scoped persist token → (document, the machine's own token).
    persists: BTreeMap<PersistToken, (DocId, PersistToken)>,
    next_token: u64,
    scratch: Vec<Effect>,
}

impl<R: Replica> Default for ConnManager<R> {
    fn default() -> Self {
        Self::new()
    }
}

impl<R: Replica> ConnManager<R> {
    /// An empty manager.
    pub fn new() -> Self {
        Self {
            machines: BTreeMap::new(),
            timers: BTreeMap::new(),
            persists: BTreeMap::new(),
            next_token: 0,
            scratch: Vec::new(),
        }
    }

    /// How many documents are resident.
    pub fn resident_docs(&self) -> usize {
        self.machines.len()
    }

    /// Feed one input; emitted effects are appended to `out`.
    pub fn handle(&mut self, input: ManagerInput, out: &mut Vec<ManagerEffect>) {
        match input {
            ManagerInput::Attach { conn, doc, caps } => {
                self.machines.entry(doc.clone()).or_default();
                self.drive(&doc, Input::PeerAttached { conn, caps }, out);
            }
            ManagerInput::Detach { conn, doc } => {
                self.drive(&doc, Input::PeerDetached { conn }, out);
            }
            ManagerInput::Frame { conn, doc, frame } => {
                self.drive(&doc, Input::Frame { conn, frame }, out);
            }
            ManagerInput::TimerFired { token } => {
                let Some((doc, machine_token)) = self.timers.remove(&token) else {
                    return; // stale: the machine was evicted meanwhile
                };
                self.drive(
                    &doc,
                    Input::TimerFired {
                        token: machine_token,
                    },
                    out,
                );
            }
            ManagerInput::Loaded { doc, snapshot } => {
                self.drive(&doc, Input::Loaded { snapshot }, out);
            }
            ManagerInput::LoadFailed { doc, error } => {
                self.drive(&doc, Input::LoadFailed { error }, out);
            }
            ManagerInput::OpsPersisted {
                doc,
                token,
                through_seq,
            } => {
                let Some((_, machine_token)) = self.persists.remove(&token) else {
                    return;
                };
                self.drive(
                    &doc,
                    Input::OpsPersisted {
                        token: machine_token,
                        through_seq,
                    },
                    out,
                );
            }
            ManagerInput::SnapshotPersisted { doc, token } => {
                let Some((_, machine_token)) = self.persists.remove(&token) else {
                    return;
                };
                self.drive(
                    &doc,
                    Input::SnapshotPersisted {
                        token: machine_token,
                    },
                    out,
                );
            }
            ManagerInput::PersistFailed { doc, token } => {
                let Some((_, machine_token)) = self.persists.remove(&token) else {
                    return;
                };
                self.drive(
                    &doc,
                    Input::PersistFailed {
                        token: machine_token,
                    },
                    out,
                );
            }
        }
    }

    /// Run one machine, then lift its effects: stamp the document, rewrite
    /// tokens to manager scope, and consume `Evict`.
    fn drive(&mut self, doc: &DocId, input: Input, out: &mut Vec<ManagerEffect>) {
        let Some(machine) = self.machines.get_mut(doc) else {
            return; // e.g. a frame for a document already evicted
        };
        let mut effects = std::mem::take(&mut self.scratch);
        machine.handle(input, &mut effects);

        let mut evicted = false;
        for effect in effects.drain(..) {
            match effect {
                Effect::Evict => evicted = true,
                Effect::ScheduleTimer { token, after_ms } => {
                    self.next_token += 1;
                    let manager_token = TimerToken(self.next_token);
                    self.timers.insert(manager_token, (doc.clone(), token));
                    out.push(ManagerEffect {
                        doc: doc.clone(),
                        effect: Effect::ScheduleTimer {
                            token: manager_token,
                            after_ms,
                        },
                    });
                }
                Effect::PersistOps {
                    token,
                    ops,
                    through_seq,
                } => {
                    let manager_token = self.lift_persist(doc, token);
                    out.push(ManagerEffect {
                        doc: doc.clone(),
                        effect: Effect::PersistOps {
                            token: manager_token,
                            ops,
                            through_seq,
                        },
                    });
                }
                Effect::PersistSnapshot {
                    token,
                    snapshot,
                    through_seq,
                } => {
                    let manager_token = self.lift_persist(doc, token);
                    out.push(ManagerEffect {
                        doc: doc.clone(),
                        effect: Effect::PersistSnapshot {
                            token: manager_token,
                            snapshot,
                            through_seq,
                        },
                    });
                }
                other => out.push(ManagerEffect {
                    doc: doc.clone(),
                    effect: other,
                }),
            }
        }
        self.scratch = effects;

        if evicted {
            self.machines.remove(doc);
            // Drop the evicted document's outstanding token mappings so stale
            // completions and timer fires route nowhere.
            self.timers.retain(|_, (d, _)| d != doc);
            self.persists.retain(|_, (d, _)| d != doc);
        }
    }

    fn lift_persist(&mut self, doc: &DocId, machine_token: PersistToken) -> PersistToken {
        self.next_token += 1;
        let manager_token = PersistToken(self.next_token);
        self.persists
            .insert(manager_token, (doc.clone(), machine_token));
        manager_token
    }
}
