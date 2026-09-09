//! One AGP client connection: NDJSON framing, per-connection session and the
//! request dispatch loop.
//!
//! Framing (`docs/protocol.md` §1) is NDJSON: one `Frame` per line. A malformed
//! frame closes **only** that connection; a failed request yields an error
//! response and the connection stays open. Every request gets exactly one
//! response.

use std::sync::Arc;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::{mpsc, Semaphore};
use tracing::Instrument;

use adesk_proto::{Frame, NdjsonCodec};

use crate::context::ServerContext;
use crate::dispatch::{error_response, forget_session_sink, register_session_sink, Dispatcher};
use crate::error::{Result, ServerError};
use crate::session::Session;

/// Capacity of the per-connection outbound frame queue.
///
/// Responses and subscription events share this queue; when it fills, event
/// frames are dropped (the client sees gaps) while responses await capacity.
pub const OUTBOUND_QUEUE_CAPACITY: usize = 1024;

/// Upper bound on requests dispatched concurrently for one connection.
///
/// Requests run concurrently so a slow `wait_for_*` cannot block `ping`
/// (`docs/architecture.md` §9); the semaphore keeps a flooding client from
/// spawning unbounded tasks.
const MAX_IN_FLIGHT_REQUESTS: usize = 64;

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
        let Connection { stream, context } = self;
        let session_id = context.next_connection_id();
        let session = Session::new(session_id);
        let span = tracing::info_span!("connection", id = session_id);

        let (read_half, write_half) = stream.into_split();
        let (frames_tx, frames_rx) = mpsc::channel::<Frame>(OUTBOUND_QUEUE_CAPACITY);
        let writer = ConnectionWriter::new(frames_tx.clone());
        let writer_task =
            tokio::spawn(write_loop(write_half, frames_rx, NdjsonCodec).instrument(span.clone()));
        // Subscription handlers (§5.6/§5.7) push frames from outside this task,
        // so the writer queue is published under the session id.
        register_session_sink(session_id, frames_tx);

        let result = read_loop(read_half, &context, &session, &writer)
            .instrument(span.clone())
            .await;

        // Drop the connection's subscriptions and close the outbound queue so
        // the writer task finishes; the peer then observes a clean EOF.
        context.subscriptions.remove_connection(session_id);
        context.inspect_subscriptions.remove_connection(session_id);
        forget_session_sink(session_id);
        drop(writer);
        let _ = writer_task.await;
        result
    }
}

/// Drains the outbound queue into the socket, one NDJSON line per frame.
async fn write_loop(mut half: OwnedWriteHalf, mut frames: mpsc::Receiver<Frame>, codec: NdjsonCodec) {
    while let Some(frame) = frames.recv().await {
        let mut line = match codec.encode_str(&frame) {
            Ok(line) => line,
            Err(error) => {
                tracing::warn!(%error, "dropping an unencodable outbound frame");
                continue;
            }
        };
        line.push('\n');
        if let Err(error) = half.write_all(line.as_bytes()).await {
            tracing::debug!(%error, "connection write failed");
            break;
        }
    }
    let _ = half.shutdown().await;
}

/// Reads NDJSON requests, dispatching each on its own task so slow requests do
/// not block the connection (`docs/architecture.md` §9).
///
/// A malformed frame (bad UTF-8, undecodable JSON, or a non-request frame)
/// closes only this connection; every accepted request is answered exactly once
/// by its dispatch task. An unknown method is the one decode failure that is
/// answered (`unknown_method`, §1) instead of closing the connection.
///
/// Each dispatch task races its handler against the shutdown token, so a request
/// that is already in flight when the runtime stops answers `shutting_down`.
async fn read_loop(
    read_half: OwnedReadHalf,
    context: &ServerContext,
    session: &Session,
    writer: &ConnectionWriter,
) -> Result<(), ServerError> {
    let dispatcher = Arc::new(Dispatcher::new(context.clone()));
    let in_flight = Arc::new(Semaphore::new(MAX_IN_FLIGHT_REQUESTS));
    let mut reader = BufReader::new(read_half);
    let mut line = Vec::with_capacity(1024);

    loop {
        line.clear();
        let read = tokio::select! {
            biased;
            () = context.shutdown.cancelled() => break,
            read = reader.read_until(b'\n', &mut line) => read?,
        };
        if read == 0 {
            break; // peer closed
        }

        let Ok(text) = std::str::from_utf8(&line) else {
            tracing::warn!("malformed frame (invalid utf-8); closing connection");
            break;
        };
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        let frame = match NdjsonCodec.decode_str(text) {
            Ok(frame) => frame,
            // Protocol §1: an unknown method answers `unknown_method` and the
            // connection stays open. `ProtoError::UnknownMethod` carries only
            // the method name, so the request id is lifted from the raw line;
            // without a usable id the line is unusable framing after all.
            Err(error @ adesk_proto::ProtoError::UnknownMethod(_)) => {
                let Some(id) = request_id_from_line(text) else {
                    tracing::warn!(%error, "unknown method without a usable id; closing connection");
                    break;
                };
                tracing::debug!(%error, id, "unknown method");
                let response = error_response(id, &ServerError::Proto(error));
                if let Err(error) = writer.send(Frame::Response(response)).await {
                    tracing::debug!(%error, "failed to deliver response");
                    break;
                }
                continue;
            }
            Err(error) => {
                tracing::warn!(%error, "malformed frame; closing connection");
                break;
            }
        };
        let Frame::Request(request) = frame else {
            tracing::warn!("client sent a non-request frame; closing connection");
            break;
        };

        let permit = Arc::clone(&in_flight)
            .acquire_owned()
            .await
            .map_err(|_| ServerError::ShuttingDown)?;
        let dispatcher = Arc::clone(&dispatcher);
        let session = session.clone();
        let writer = writer.clone();
        let shutdown = context.shutdown.clone();
        let span =
            tracing::info_span!("request", id = request.id, method = request.method.method_name());
        tokio::spawn(
            async move {
                let _permit = permit;
                let id = request.id;
                // Shutdown step 2 (`docs/architecture.md` §9): a request that is
                // already inside a handler when the runtime stops fails with
                // `shutting_down` instead of answering from a torn-down runtime.
                // `biased` so an already-flagged token wins over a handler that
                // would otherwise run to its own timeout.
                let response = tokio::select! {
                    biased;
                    () = shutdown.cancelled() => error_response(id, &ServerError::ShuttingDown),
                    response = dispatcher.dispatch(&session, request) => response,
                };
                if let Err(error) = writer.send(Frame::Response(response)).await {
                    tracing::debug!(%error, "failed to deliver response");
                }
            }
            .instrument(span),
        );
    }

    Ok(())
}

/// Lifts the `id` of a raw request line that failed to decode as an unknown
/// method.
///
/// `ProtoError::UnknownMethod` keeps only the method name, so the id has to come
/// from the line itself. Returns `None` when the line is not a JSON object with
/// a `u64` id — the caller then treats it as malformed framing.
fn request_id_from_line(text: &str) -> Option<u64> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    value.get("id")?.as_u64()
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
        self.tx.send(frame).await.map_err(|_| ServerError::ShuttingDown)
    }

    /// Queues one frame without waiting.
    ///
    /// Returns `false` when the queue is full or the connection is gone; used
    /// by the event fan-out, which must never block the pump.
    pub fn try_send(&self, frame: Frame) -> bool {
        self.tx.try_send(frame).is_ok()
    }

    /// The NDJSON codec used for this connection.
    pub fn codec(&self) -> &NdjsonCodec {
        &self.codec
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use adesk_core::ErrorCode;
    use adesk_proto::{ErrorPayload, ResponseFrame};

    fn response(id: u64) -> Frame {
        Frame::Response(ResponseFrame::error(
            id,
            ErrorPayload::new(ErrorCode::Internal, "test"),
        ))
    }

    #[tokio::test]
    async fn send_awaits_capacity_and_delivers_frames() {
        let (tx, mut rx) = mpsc::channel(2);
        let writer = ConnectionWriter::new(tx);
        writer.send(response(1)).await.unwrap();
        writer.send(response(2)).await.unwrap();
        assert_eq!(rx.recv().await, Some(response(1)));
        assert_eq!(rx.recv().await, Some(response(2)));
    }

    #[tokio::test]
    async fn send_reports_shutting_down_when_the_writer_is_gone() {
        let (tx, rx) = mpsc::channel(1);
        let writer = ConnectionWriter::new(tx);
        drop(rx);
        let error = writer.send(response(1)).await.expect_err("writer gone");
        assert!(matches!(error, ServerError::ShuttingDown));
    }

    #[tokio::test]
    async fn try_send_drops_frames_when_the_queue_is_full() {
        let (tx, mut rx) = mpsc::channel(1);
        let writer = ConnectionWriter::new(tx);
        assert!(writer.try_send(response(1)));
        assert!(!writer.try_send(response(2)), "full queue must drop, not block");
        assert_eq!(rx.recv().await, Some(response(1)));
    }

    #[tokio::test]
    async fn try_send_reports_failure_when_the_writer_is_gone() {
        let (tx, rx) = mpsc::channel(1);
        let writer = ConnectionWriter::new(tx);
        drop(rx);
        assert!(!writer.try_send(response(1)));
    }

    #[test]
    fn request_id_is_lifted_from_a_raw_request_line() {
        assert_eq!(
            request_id_from_line(r#"{"id":77,"method":"definitely_not_a_method","params":{}}"#),
            Some(77)
        );
        assert_eq!(
            request_id_from_line(r#"{"id": 4294967297, "method": "nope"}"#),
            Some(4_294_967_297)
        );
    }

    #[test]
    fn request_id_is_absent_for_unusable_lines() {
        for line in [
            "not json at all",
            "{}",
            r#"{"method":"nope","params":{}}"#,
            r#"{"id":"77","method":"nope"}"#,
            r#"{"id":-1,"method":"nope"}"#,
            r#"{"id":null,"method":"nope"}"#,
        ] {
            assert_eq!(request_id_from_line(line), None, "line: {line}");
        }
    }
}
