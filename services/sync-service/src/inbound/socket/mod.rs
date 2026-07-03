pub mod websocket;

use bebop::Record;
use worker::WebSocket;

use crate::{
    domain::{ports::SyncServiceError, socket::Socket},
    error::ResultExt,
};

pub struct WorkerSocket {
    ws: WebSocket,
    id: String,
}

impl WorkerSocket {
    pub fn new(ws: WebSocket, id: String) -> Self {
        Self { ws, id }
    }
}

impl Socket for WorkerSocket {
    fn id(&self) -> &str {
        &self.id
    }

    fn send<'m, T: Record<'m>>(&self, msg: T) -> Result<(), SyncServiceError> {
        let mut buf = Vec::new();
        msg.serialize(&mut buf)
            .context("failed to serialize message")?;
        self.ws
            .send_with_bytes(&buf)
            .context("failed to send message")?;
        Ok(())
    }
}
