//! The router core: one task owns this, so per-connection ordering is
//! enforced by construction and no state needs locks.

#[cfg(test)]
mod test;

use crate::domain::{
    envelope::{self, ClientEnvelope},
    models::{ConnectionId, DocId, EdgeEvent, Event},
    ports::{DownstreamFactory, EdgeSink},
};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, warn};

/// Routes envelope frames between gateway connections and per-document
/// downstreams. Owned and driven by a single task; see [`Router::handle`].
pub struct Router<Sink: EdgeSink, Downstreams: DownstreamFactory> {
    sink: Arc<Sink>,
    downstreams: Downstreams,
    routes: HashMap<(ConnectionId, DocId), mpsc::Sender<Vec<u8>>>,
    /// Which documents each connection has open, for disconnect teardown.
    by_conn: HashMap<ConnectionId, HashSet<DocId>>,
}

impl<Sink: EdgeSink, Downstreams: DownstreamFactory> Router<Sink, Downstreams> {
    /// Create a router over the given sink and downstream factory.
    pub fn new(sink: Arc<Sink>, downstreams: Downstreams) -> Self {
        Self {
            sink,
            downstreams,
            routes: HashMap::new(),
            by_conn: HashMap::new(),
        }
    }

    /// Drive the router from an event stream until it closes.
    pub async fn run(mut self, mut events: mpsc::Receiver<Event>) {
        while let Some(event) = events.recv().await {
            self.handle(event).await;
        }
    }

    /// Process one event.
    #[tracing::instrument(skip_all)]
    pub async fn handle(&mut self, event: Event) {
        match event {
            Event::Edge(EdgeEvent::Frame { conn, payload }) => {
                match envelope::decode_client(&payload) {
                    Ok(frame) => self.handle_client(conn, frame).await,
                    Err(error) => {
                        warn!(error = ?error, conn = ?conn, "dropping undecodable client frame");
                    }
                }
            }
            Event::Edge(EdgeEvent::Disconnected { conn }) => self.drop_conn(&conn),
            Event::Edge(EdgeEvent::GatewayLost { gateway }) => {
                let conns: Vec<ConnectionId> = self
                    .by_conn
                    .keys()
                    .filter(|conn| conn.gateway == gateway)
                    .cloned()
                    .collect();
                debug!(
                    gateway,
                    count = conns.len(),
                    "gateway lost; dropping its connections"
                );
                for conn in conns {
                    self.drop_conn(&conn);
                }
            }
            Event::DownstreamClosed { conn, doc } => {
                // The pump already told the client; just forget the route.
                self.routes.remove(&(conn.clone(), doc.clone()));
                if let Some(docs) = self.by_conn.get_mut(&conn) {
                    docs.remove(&doc);
                    if docs.is_empty() {
                        self.by_conn.remove(&conn);
                    }
                }
            }
        }
    }

    #[tracing::instrument(skip_all, fields(conn = %conn.conn, gateway = %conn.gateway))]
    async fn handle_client(&mut self, conn: ConnectionId, frame: ClientEnvelope) {
        match frame {
            ClientEnvelope::Subscribe { doc, token } => {
                let doc = DocId(doc);
                let key = (conn.clone(), doc.clone());
                if self.routes.contains_key(&key) {
                    // Idempotent: the downstream is already up (or dialing).
                    // Re-ack so a retrying client settles.
                    self.deliver(&conn, envelope::subscribed(doc.as_str()))
                        .await;
                    return;
                }
                debug!(doc = doc.as_str(), "opening downstream");
                let sender = self.downstreams.open(conn.clone(), doc.clone(), token);
                self.routes.insert(key, sender);
                self.by_conn.entry(conn).or_default().insert(doc);
            }
            ClientEnvelope::Unsubscribe { doc } => {
                let doc = DocId(doc);
                // Dropping the sender closes the downstream quietly (the pump
                // distinguishes our hangup from an upstream death).
                self.routes.remove(&(conn.clone(), doc.clone()));
                if let Some(docs) = self.by_conn.get_mut(&conn) {
                    docs.remove(&doc);
                }
            }
            ClientEnvelope::Frame { doc, payload } => {
                let doc = DocId(doc);
                let Some(sender) = self.routes.get(&(conn.clone(), doc.clone())) else {
                    warn!(
                        doc = doc.as_str(),
                        "frame for unsubscribed document; dropping"
                    );
                    return;
                };
                // try_send: a full buffer means a downstream that can't keep
                // up; drop the frame — the sync protocol self-heals via
                // catch-up requests. A closed channel means the downstream
                // died; DownstreamClosed will clean the route up.
                if let Err(error) = sender.try_send(payload) {
                    warn!(doc = doc.as_str(), error = %error, "dropping frame for downstream");
                }
            }
        }
    }

    fn drop_conn(&mut self, conn: &ConnectionId) {
        let Some(docs) = self.by_conn.remove(conn) else {
            return;
        };
        debug!(conn = %conn.conn, count = docs.len(), "dropping connection's downstreams");
        for doc in docs {
            self.routes.remove(&(conn.clone(), doc));
        }
    }

    async fn deliver(&self, conn: &ConnectionId, frame: Vec<u8>) {
        self.sink
            .deliver(conn, frame)
            .await
            .map_err(Into::into)
            .inspect_err(|error: &anyhow::Error| {
                warn!(error = ?error, conn = %conn.conn, "failed to deliver frame to client");
            })
            .ok();
    }
}
