//! Owns the live document actors: spawn on demand, respawn after exit.
//!
//! Compare with `sync_machine::manager::ConnManager`. Eviction here is the
//! actor breaking its loop; the supervisor discovers the closed mailbox on
//! the next send and respawns. There is no token namespacing because there
//! are no tokens — completions never leave an actor.

#[cfg(test)]
mod test;

use crate::domain::actor::{self, DocMsg};
use crate::domain::ports::{ClientSink, DocEvents, DocStore};
use std::collections::BTreeMap;
use std::marker::PhantomData;
use std::sync::Arc;
use sync_machine::model::{Caps, ClientFrame, ConnId, DocId};
use sync_machine::replica::Replica;
use tokio::sync::mpsc;

/// Frames buffered per document mailbox. Overflow drops the message (the
/// sync protocol self-heals), mirroring the router's downstream buffer.
const MAILBOX: usize = 256;

/// See the module docs.
pub struct Supervisor<R, Store, Sink, Events> {
    docs: BTreeMap<DocId, mpsc::Sender<DocMsg>>,
    subscriptions: BTreeMap<ConnId, Vec<DocId>>,
    store: Arc<Store>,
    sink: Arc<Sink>,
    events: Arc<Events>,
    _replica: PhantomData<R>,
}

impl<R, Store, Sink, Events> Supervisor<R, Store, Sink, Events>
where
    R: Replica + Send + Sync + 'static,
    R::Error: Send,
    Store: DocStore,
    Sink: ClientSink,
    Events: DocEvents,
{
    /// A supervisor over the given ports.
    pub fn new(store: Arc<Store>, sink: Arc<Sink>, events: Arc<Events>) -> Self {
        Self {
            docs: BTreeMap::new(),
            subscriptions: BTreeMap::new(),
            store,
            sink,
            events,
            _replica: PhantomData,
        }
    }

    /// How many document actors currently look alive.
    pub fn resident_docs(&self) -> usize {
        self.docs.len()
    }

    /// Attach a connection to a document, spawning (or respawning) its actor.
    pub async fn subscribe(&mut self, conn: ConnId, doc: DocId, caps: Caps) {
        self.subscriptions
            .entry(conn)
            .or_default()
            .push(doc.clone());
        self.send(&doc, DocMsg::Attach { conn, caps }).await;
    }

    /// Detach a connection from one document.
    pub async fn unsubscribe(&mut self, conn: ConnId, doc: DocId) {
        if let Some(docs) = self.subscriptions.get_mut(&conn) {
            docs.retain(|d| d != &doc);
        }
        // A dead actor means the document is already peerless; don't respawn
        // just to detach.
        self.send_if_alive(&doc, DocMsg::Detach { conn }).await;
    }

    /// A connection is gone; detach it everywhere.
    pub async fn disconnected(&mut self, conn: ConnId) {
        let docs = self.subscriptions.remove(&conn).unwrap_or_default();
        for doc in docs {
            self.send_if_alive(&doc, DocMsg::Detach { conn }).await;
        }
    }

    /// Route one frame.
    pub async fn frame(&mut self, conn: ConnId, doc: DocId, frame: ClientFrame) {
        // Frames don't respawn: a frame for a dead actor is a frame for a
        // document the connection is no longer attached to server-side (the
        // actor exits only when peerless), so it would be NotAttached anyway.
        self.send_if_alive(&doc, DocMsg::Frame { conn, frame })
            .await;
    }

    /// Send, spawning or respawning the actor as needed.
    async fn send(&mut self, doc: &DocId, msg: DocMsg) {
        let sender = match self.docs.get(doc) {
            Some(sender) if !sender.is_closed() => sender.clone(),
            _ => self.spawn(doc),
        };
        sender.send(msg).await.ok();
    }

    async fn send_if_alive(&mut self, doc: &DocId, msg: DocMsg) {
        if let Some(sender) = self.docs.get(doc)
            && !sender.is_closed()
        {
            sender.send(msg).await.ok();
        }
    }

    fn spawn(&mut self, doc: &DocId) -> mpsc::Sender<DocMsg> {
        let (sender, receiver) = mpsc::channel(MAILBOX);
        tokio::spawn(actor::run::<R, Store, Sink, Events>(
            doc.clone(),
            receiver,
            Arc::clone(&self.store),
            Arc::clone(&self.sink),
            Arc::clone(&self.events),
        ));
        self.docs.insert(doc.clone(), sender.clone());
        sender
    }
}
