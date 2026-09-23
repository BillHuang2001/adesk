//! One AGP client connection: NDJSON framing, per-connection session and the
//! request dispatch loop.
//!
//! Framing (`docs/protocol.md` §1) is NDJSON: one `Frame` per line. Every
//! request gets exactly one response and the connection stays open, even when
//! the request is rejected (`unknown_method`, `invalid_request`). A connection
//! closes only on framing corruption — a line that is not a request at all
//! (invalid UTF-8, or JSON without a `u64` id) or a decoded non-request frame —
//! and then only that connection (§6).
//!
//! A line that closes the connection is also classified by shape
//! (`classify_raw_line`): when it is a VAP viewer message (a `type`-tagged
//! object, `docs/viewer.md` §2–§4) the closing warning names the viewer
//! endpoint socket, so a viewer pointed at the AGP socket diagnoses itself in
//! the log. This is log-only: the wire behavior is identical either way.

use std::sync::Arc;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::UnixStream;
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
///
/// The line buffer is reused across iterations, and every frame already queued
/// when the loop wakes is appended to it before a single `write_all`, so a burst
/// of responses/events costs one syscall instead of one per frame. The bytes on
/// the wire are unchanged: each frame is one `\n`-terminated NDJSON line, emitted
/// in queue order.
async fn write_loop(
    mut half: OwnedWriteHalf,
    mut frames: mpsc::Receiver<Frame>,
    codec: NdjsonCodec,
) {
    let mut buffer = String::new();
    while let Some(frame) = frames.recv().await {
        buffer.clear();
        append_line(&mut buffer, &codec, &frame);
        // Batch every frame already sitting in the queue into the same write.
        while let Ok(queued) = frames.try_recv() {
            append_line(&mut buffer, &codec, &queued);
        }
        if buffer.is_empty() {
            // The whole batch failed to encode: there is nothing to write.
            continue;
        }
        if let Err(error) = half.write_all(buffer.as_bytes()).await {
            tracing::debug!(%error, "connection write failed");
            break;
        }
    }
    let _ = half.shutdown().await;
}

/// Appends `frame` to `buffer` as one `\n`-terminated NDJSON line.
///
/// An unencodable frame is logged and dropped without breaking the batch, so
/// the remaining queued frames are still delivered.
fn append_line(buffer: &mut String, codec: &NdjsonCodec, frame: &Frame) {
    match codec.encode_str(frame) {
        Ok(line) => {
            buffer.push_str(&line);
            buffer.push('\n');
        }
        Err(error) => tracing::warn!(%error, "dropping an unencodable outbound frame"),
    }
}

/// Reads NDJSON requests, dispatching each on its own task so slow requests do
/// not block the connection (`docs/architecture.md` §9).
///
/// A frame that does not decode but still names a request is answered with an
/// error response and the connection stays open (§6): `unknown_method` for an
/// unknown method, `invalid_request` for params that fail validation. Only a
/// line that cannot be identified as a request — invalid UTF-8, or JSON with no
/// `u64` id — or a *decoded* non-request frame closes this connection. Every
/// accepted request is answered exactly once by its dispatch task.
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
            // Protocol §6: a client error never closes the connection — only
            // framing corruption does. Every `ProtoError` that describes a
            // *request* (unknown method, params that fail validation, a
            // request-shaped object that matches no frame) is answered with the
            // code it maps to (`unknown_method` / `invalid_request`) and the
            // connection stays usable. `ProtoError` carries no request id, so it
            // is lifted from the raw line; a line from which no id can be lifted
            // is not identifiable as a request and closes the connection.
            Err(error) => {
                let Some(id) = request_id_from_line(text) else {
                    tracing::warn!(%error, "undecodable frame without a usable id; closing connection");
                    // Log-only diagnosis of the one common cause: the client is
                    // a VAP viewer pointed at the AGP socket (§6 close stays
                    // exactly as it is — nothing is sent, nothing is added).
                    hint_if_viewer_line(context, classify_raw_line(text));
                    break;
                };
                tracing::debug!(%error, id, "answering an undecodable request");
                let response = error_response(id, &ServerError::Proto(error));
                if let Err(error) = writer.send(Frame::Response(response)).await {
                    tracing::debug!(%error, "failed to deliver response");
                    break;
                }
                continue;
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
        let span = tracing::info_span!(
            "request",
            id = request.id,
            method = request.method.method_name()
        );
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

/// Lifts the `id` of a raw request line that failed to decode into a frame.
///
/// `ProtoError` keeps only a description of the failure (the method name, the
/// params error), never the request id, so the id has to come from the line
/// itself — this is what lets `read_loop` answer an undecodable request
/// (`unknown_method`, `invalid_request`) without closing the connection (§6).
/// Returns `None` when the line is not a JSON object with a `u64` id — the
/// caller then treats it as framing corruption.
fn request_id_from_line(text: &str) -> Option<u64> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    value.get("id")?.as_u64()
}

/// Why a raw line failed to decode into an AGP frame, at the granularity needed
/// to diagnose the one real-world cause of a connection close: a VAP viewer
/// connected to the AGP socket instead of the viewer endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RawLineKind {
    /// The line is a VAP viewer message (`docs/viewer.md` §2–§4): a JSON object
    /// with a string `"type"` field, which AGP frames never carry.
    Viewer,
    /// Anything else: an AGP frame, other JSON, or not JSON at all.
    Other,
}

/// Classifies a raw line by shape, without validating it as either protocol.
///
/// VAP messages (`adesk-viewer-proto`) are flat JSON objects discriminated by a
/// string `"type"` field; the VAP client tags are `hello`, `request_frame`,
/// `request_state`, `pointer_move`, `pointer_button`, `scroll`, `key`, `text`,
/// `activate_window`, `set_control`, `bye`, `start_recording`,
/// `stop_recording` and `request_recording` — and even an *unknown* tag keeps
/// the `"type"` string. AGP frames are discriminated by `method`/`event`/`id`
/// (`adesk_proto::Frame::from_value`) and never carry `"type"`, so a string
/// `"type"` on an otherwise unusable line is a viewer's signature. The line is
/// not required to be a *valid* VAP message (a real viewer's first line always
/// is, but the hint must not depend on it being parsed).
fn classify_raw_line(text: &str) -> RawLineKind {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
        return RawLineKind::Other;
    };
    match value.get("type") {
        Some(serde_json::Value::String(_)) => RawLineKind::Viewer,
        _ => RawLineKind::Other,
    }
}

/// Emits the misconnection hint when a closing line is a VAP viewer message.
///
/// The log-only twin of the close: the caller has already decided to close the
/// connection exactly as it would have without the classification, and this
/// only explains *why* a viewer is seeing EOF. It resolves the viewer endpoint
/// socket viewers must use from the runtime's configuration — the one this
/// runtime actually bound (`ServerConfig::viewer_socket_path`), or `None`
/// when the endpoint is disabled, which the message then says instead.
fn hint_if_viewer_line(context: &ServerContext, kind: RawLineKind) {
    warn_viewer_misconnection(kind, context.config.viewer_socket_path().as_deref());
}

/// Logs the VAP-on-the-AGP-socket warning for a `Viewer` line; `viewer_socket`
/// is the viewer endpoint socket the client should have connected to, or `None`
/// when this runtime has no viewer endpoint at all. A non-`Viewer` line logs
/// nothing. Pure log emitter, so tests can capture it.
fn warn_viewer_misconnection(kind: RawLineKind, viewer_socket: Option<&std::path::Path>) {
    if kind != RawLineKind::Viewer {
        return;
    }
    match viewer_socket {
        Some(path) => tracing::warn!(
            viewer_socket = %path.display(),
            "this line is a VAP viewer message, but the client connected to the AGP \
             socket; point the viewer (adesk-viewer / adesk-viewer-gui) at the viewer \
             endpoint socket instead (the \"viewer endpoint bound\" log line names it)"
        ),
        None => tracing::warn!(
            "this line is a VAP viewer message, but the client connected to the AGP \
             socket; this runtime was started with the viewer (VAP) endpoint disabled \
             (--no-viewer), so there is no viewer socket to use"
        ),
    }
}

/// Outbound half of a connection: frames for responses and subscription events.
#[derive(Clone)]
pub struct ConnectionWriter {
    tx: mpsc::Sender<Frame>,
}

impl ConnectionWriter {
    /// Wraps the queue that feeds the connection's writer task.
    pub fn new(tx: mpsc::Sender<Frame>) -> ConnectionWriter {
        ConnectionWriter { tx }
    }

    /// Queues one frame, awaiting capacity (backpressure for responses).
    ///
    /// # Errors
    ///
    /// Returns [`ServerError::ShuttingDown`] when the writer task is gone.
    pub async fn send(&self, frame: Frame) -> Result<()> {
        self.tx
            .send(frame)
            .await
            .map_err(|_| ServerError::ShuttingDown)
    }

    /// Queues one frame without waiting.
    ///
    /// Returns `false` when the queue is full or the connection is gone; used
    /// by the event fan-out, which must never block the pump.
    pub fn try_send(&self, frame: Frame) -> bool {
        self.tx.try_send(frame).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use adesk_core::ErrorCode;
    use adesk_proto::{ErrorPayload, ResponseFrame};
    use std::path::Path;
    use std::sync::{Arc, Mutex};

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
        assert!(
            !writer.try_send(response(2)),
            "full queue must drop, not block"
        );
        assert_eq!(rx.recv().await, Some(response(1)));
    }

    #[tokio::test]
    async fn try_send_reports_failure_when_the_writer_is_gone() {
        let (tx, rx) = mpsc::channel(1);
        let writer = ConnectionWriter::new(tx);
        drop(rx);
        assert!(!writer.try_send(response(1)));
    }

    /// The batched, buffer-reusing `write_loop` emits exactly the same NDJSON
    /// bytes as one `NdjsonCodec::encode_str` + `'\n'` per frame, in queue order.
    #[tokio::test]
    async fn write_loop_emits_one_ndjson_line_per_frame_in_order() {
        let (stream, peer) = UnixStream::pair().expect("unix socket pair");
        let (_read_half, write_half) = stream.into_split();
        let (tx, rx) = mpsc::channel(8);
        let writer = tokio::spawn(write_loop(write_half, rx, NdjsonCodec));

        // Queue a burst so several frames are already waiting when the writer
        // wakes, then close the queue so the task drains and finishes.
        for id in 1..=4u64 {
            tx.send(response(id)).await.unwrap();
        }
        drop(tx);

        let mut reader = BufReader::new(peer);
        let mut lines = Vec::new();
        let mut line = String::new();
        while reader.read_line(&mut line).await.unwrap() > 0 {
            lines.push(std::mem::take(&mut line));
        }
        writer.await.expect("writer task must not panic");

        assert_eq!(lines.len(), 4, "one line per queued frame");
        for (index, raw) in lines.iter().enumerate() {
            let id = index as u64 + 1;
            let expected = NdjsonCodec.encode_str(&response(id)).unwrap();
            assert_eq!(raw, &format!("{expected}\n"), "frame {id} bytes differ");
        }
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

    #[test]
    fn a_vap_typed_line_is_classified_as_a_viewer_message() {
        // Every VAP client tag (`adesk_viewer_proto::ClientMessage::message_type`).
        for tag in [
            "hello",
            "request_frame",
            "request_state",
            "pointer_move",
            "pointer_button",
            "scroll",
            "key",
            "text",
            "activate_window",
            "set_control",
            "bye",
            "start_recording",
            "stop_recording",
            "request_recording",
        ] {
            let line = format!(r#"{{"type":"{tag}"}}"#);
            assert_eq!(classify_raw_line(&line), RawLineKind::Viewer, "tag: {tag}");
        }
        // An unrecognised tag is still VAP-shaped: forward compatibility keeps
        // the `"type"` string (docs/viewer.md §1), so a newer viewer is hinted
        // too.
        assert_eq!(
            classify_raw_line(r#"{"type":"something_newer","field":1}"#),
            RawLineKind::Viewer
        );
        // The real handshake line, built by the VAP codec itself — the exact
        // first line a viewer pointed at the wrong socket sends.
        let hello = adesk_viewer_proto::encode_client(&adesk_viewer_proto::ClientMessage::Hello(
            adesk_viewer_proto::ViewerHello::new(),
        ));
        assert_eq!(classify_raw_line(&hello), RawLineKind::Viewer);
    }

    #[test]
    fn an_agp_or_non_json_line_is_not_a_viewer_message() {
        // Ordinary AGP frames discriminate on `method`/`event`/`id` and never
        // carry `"type"`.
        assert_eq!(
            classify_raw_line(r#"{"id":77,"method":"ping","params":{}}"#),
            RawLineKind::Other
        );
        assert_eq!(
            classify_raw_line(r#"{"event":"surface_commit","seq":1,"ts_ms":0,"data":{}}"#),
            RawLineKind::Other
        );
        assert_eq!(
            classify_raw_line(r#"{"id":5,"result":{"protocol_version":1}}"#),
            RawLineKind::Other
        );
        // Valid JSON that is neither protocol.
        assert_eq!(classify_raw_line("null"), RawLineKind::Other);
        assert_eq!(classify_raw_line("[1, 2, 3]"), RawLineKind::Other);
        assert_eq!(classify_raw_line(r#"{}"#), RawLineKind::Other);
        assert_eq!(
            classify_raw_line(r#"{"typed":true}"#),
            RawLineKind::Other,
            "a `type`-like key is not the VAP discriminator"
        );
        // A non-string `"type"` is not a VAP discriminator either.
        assert_eq!(classify_raw_line(r#"{"type":7}"#), RawLineKind::Other);
        assert_eq!(classify_raw_line(r#"{"type":null}"#), RawLineKind::Other);
        // Not JSON at all.
        assert_eq!(classify_raw_line("this is not json"), RawLineKind::Other);
        assert_eq!(classify_raw_line(""), RawLineKind::Other);
    }

    /// Captures everything the emitter logs into a shared buffer; per-thread
    /// default dispatch keeps concurrent tests from cross-talking.
    struct Captured(Arc<Mutex<Vec<String>>>);

    impl tracing::Subscriber for Captured {
        fn event(&self, event: &tracing::Event<'_>) {
            let mut visitor = MessageVisitor(Vec::new());
            event.record(&mut visitor);
            self.0
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .extend(visitor.0);
        }

        fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
            true
        }
        fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::Id {
            tracing::Id::from_u64(1)
        }
        fn record(&self, _: &tracing::Id, _: &tracing::span::Record<'_>) {}
        fn record_follows_from(&self, _: &tracing::Id, _: &tracing::Id) {}
        fn enter(&self, _: &tracing::Id) {}
        fn exit(&self, _: &tracing::Id) {}
    }

    /// Pulls the `message` field plus every structured field out of each
    /// logged event, one flat string per event (`message … viewer_socket=…`).
    struct MessageVisitor(Vec<String>);

    impl tracing::field::Visit for MessageVisitor {
        fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
            if field.name() == "message" {
                self.0.push(format!("{value:?}"));
            } else {
                if let Some(message) = self.0.last_mut() {
                    message.push_str(&format!(" {}={value:?}", field.name()));
                }
            }
        }
    }

    fn captured_logs(f: impl FnOnce()) -> Vec<String> {
        let logs: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let subscriber = Captured(Arc::clone(&logs));
        let _guard = tracing::subscriber::set_default(subscriber);
        f();
        let result = logs.lock().unwrap_or_else(|e| e.into_inner()).clone();
        result
    }

    #[test]
    fn a_viewer_line_logs_the_misconnection_hint_with_the_viewer_socket() {
        let logs = captured_logs(|| {
            warn_viewer_misconnection(
                RawLineKind::Viewer,
                Some(Path::new("/run/x/adesk-viewer.sock")),
            );
        });
        assert_eq!(logs.len(), 1, "exactly one warning: {logs:?}");
        assert!(
            logs[0].contains("VAP viewer message"),
            "the hint must name the misconnection: {:?}",
            logs[0]
        );
        assert!(
            logs[0].contains("adesk-viewer.sock"),
            "the hint must point at the viewer endpoint socket: {:?}",
            logs[0]
        );
    }

    #[test]
    fn a_viewer_line_without_a_viewer_endpoint_says_the_endpoint_is_disabled() {
        let logs = captured_logs(|| {
            warn_viewer_misconnection(RawLineKind::Viewer, None);
        });
        assert_eq!(logs.len(), 1, "exactly one warning: {logs:?}");
        assert!(
            logs[0].contains("disabled"),
            "a disabled endpoint must be named as such: {:?}",
            logs[0]
        );
    }

    #[test]
    fn a_non_viewer_line_logs_nothing() {
        let logs = captured_logs(|| {
            warn_viewer_misconnection(
                RawLineKind::Other,
                Some(Path::new("/run/x/adesk-viewer.sock")),
            );
        });
        assert!(logs.is_empty(), "no hint for ordinary garbage: {logs:?}");
    }
}
