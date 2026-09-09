//! One AGP client connection: NDJSON framing, per-connection session and the
//! request dispatch loop.
//!
//! Framing (`docs/protocol.md` §1) is NDJSON: one `Frame` per line. A malformed
//! frame closes **only** that connection; a failed request yields an error
//! response and the connection stays open. Every request gets exactly one
//! response.

use tokio::net::UnixStream;
use tokio::sync::mpsc;

use adesk_proto::{Frame, NdjsonCodec};

use crate::context::ServerContext;
use crate::error::{Result, ServerError};

/// Capacity of the per-connection outbound frame queue.
///
/// Responses and subscription events share this queue; when it fills, event
/// frames are dropped (the client sees gaps) while responses await capacity.
pub const OUTBOUND_QUEUE_CAPACITY: usize = 1024;

/// Serves one connection until the peer disconnects or the server shuts down.
pub struct Connection {
    stream: UnixStream,
    context: ServerContext,
}

impl Connection {
    /// Wraps an accepted stream.
    pub fn new(stream: UnixStream, context: ServerContext) -> Connection {
        Connection { stream, context }
    }

    /// Reads requests until EOF, dispatching each through
    /// [`crate::dispatch::Dispatcher`].
    ///
    /// Spawns the writer task for this connection, allocates a
    /// [`crate::session::Session`], and on exit removes the session's
    /// subscriptions.
    ///
    /// # Errors
    ///
    /// Returns [`ServerError::Io`] only for socket failures; protocol errors are
    /// per-connection and end the loop with `Ok(())`.
    pub async fn run(self) -> Result<(), ServerError> {
        todo!()
    }
}

/// Outbound half of a connection: frames for responses and subscription events.
#[derive(Clone)]
pub struct ConnectionWriter {
    tx: mpsc::Sender<Frame>,
    codec: NdjsonCodec,
}

impl ConnectionWriter {
    /// Wraps the queue that feeds the connection's writer task.
    pub fn new(tx: mpsc::Sender<Frame>) -> ConnectionWriter {
        ConnectionWriter { tx, codec: NdjsonCodec }
    }

    /// Queues one frame, awaiting capacity (backpressure for responses).
    ///
    /// # Errors
    ///
    /// Returns [`ServerError::ShuttingDown`] when the writer task is gone.
    pub async fn send(&self, frame: Frame) -> Result<()> {
        todo!()
    }

    /// Queues one frame without waiting.
    ///
    /// Returns `false` when the queue is full or the connection is gone; used
    /// by the event fan-out, which must never block the pump.
    pub fn try_send(&self, frame: Frame) -> bool {
        todo!()
    }

    /// The NDJSON codec used for this connection.
    pub fn codec(&self) -> &NdjsonCodec {
        &self.codec
    }
}
