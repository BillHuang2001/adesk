#![allow(dead_code)] // each test target uses a subset of the harness helpers

//! Shared mock AGP server harness for `adesk-client` integration tests.
//!
//! **Not a test target**: each test file declares `mod common;` and drives one
//! [`MockServer`].
//!
//! What it is:
//!
//! - a real `tokio::net::UnixListener` bound to `<tempdir>/adesk.sock`; the
//!   [`TempDir`] owns the path and removes it when the server drops,
//! - NDJSON framing through `adesk_proto::{NdjsonCodec, Frame, RequestFrame,
//!   ResponseFrame, EventFrame, ErrorPayload}` — exactly the proto surface
//!   required by `crates/adesk-client/CONTEXT.md` → "Dependencies",
//! - exactly one accepted client connection, split into read/write halves,
//! - a scriptable peer: the test reads requests and chooses what to write back
//!   (result frames, error frames, raw lines, event frames, or a close).
//!
//! There is no compositor, display, GPU or network: a test connects an
//! `adesk_client::Client` to [`MockServer::path`] and asserts both the wire
//! bytes and the typed results (`CONTEXT.md` → "Test Strategy").

use std::path::{Path, PathBuf};

use adesk_core::ErrorCode;
use adesk_proto::{
    Codec, ErrorPayload, EventFrame, EventKind, EventPayload, Frame, NdjsonCodec, RequestFrame,
    ResponseFrame, ResponseOutcome, ResultPayload,
};
use serde_json::Value;
use tempfile::TempDir;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::unix::OwnedWriteHalf;
use tokio::net::UnixListener;

/// A scripted AGP peer speaking NDJSON over a real Unix socket.
///
/// One instance serves exactly one client connection: [`MockServer::start`]
/// binds and listens, [`MockServer::accept`] takes the connection, then the
/// test alternates [`MockServer::next_request`] with `respond*` /
/// [`MockServer::emit_event`] / [`MockServer::send_raw`], and finally
/// [`MockServer::close`].
pub struct MockServer {
    /// Owns the temp dir (and therefore the socket path); removed on drop.
    _dir: TempDir,
    /// `<tempdir>/adesk.sock` — hand this to `Client::connect`.
    socket_path: PathBuf,
    /// Bound listener; `accept` takes the first connection.
    listener: UnixListener,
    /// Read half of the accepted connection (inbound request frames).
    ///
    /// Buffered so a `next_request` never swallows bytes of the next frame.
    reader: Option<BufReader<tokio::net::unix::OwnedReadHalf>>,
    /// Write half of the accepted connection (responses, events, raw lines).
    writer: Option<OwnedWriteHalf>,
    /// NDJSON codec — the same `adesk-proto` codec the runtime uses.
    codec: NdjsonCodec,
}

impl MockServer {
    /// Bind `<tempdir>/adesk.sock` and start listening.
    pub async fn start() -> MockServer {
        let dir = tempfile::tempdir().expect("mock server: create temp dir");
        let socket_path = dir.path().join("adesk.sock");
        let listener =
            UnixListener::bind(&socket_path).expect("mock server: bind <tempdir>/adesk.sock");
        MockServer {
            _dir: dir,
            socket_path,
            listener,
            reader: None,
            writer: None,
            codec: NdjsonCodec,
        }
    }

    /// Socket path to hand to `Client::connect` / `ConnectOptions::path`.
    pub fn path(&self) -> &Path {
        &self.socket_path
    }

    /// Accept exactly one client connection and split it into read/write halves.
    pub async fn accept(&mut self) {
        let (stream, _addr) = self
            .listener
            .accept()
            .await
            .expect("mock server: accept client connection");
        let (reader, writer) = stream.into_split();
        self.reader = Some(BufReader::new(reader));
        self.writer = Some(writer);
    }

    /// Read one request frame and return `(id, method, params)`.
    ///
    /// Panics if the client sends anything that is not a well-formed NDJSON
    /// `Frame::Request` (that is a client bug, not a server behaviour to model).
    pub async fn next_request(&mut self) -> (u64, String, Value) {
        let line = self.read_line().await;
        let frame = self
            .codec
            .decode(&line)
            .unwrap_or_else(|error| panic!("mock server: malformed client frame: {error}"));
        match frame {
            Frame::Request(request) => request_parts(request),
            other => panic!("mock server: expected a request frame, got {other:?}"),
        }
    }

    /// Answer request `id` with `result` (a `ResponseOutcome::Result` frame).
    pub async fn respond(&mut self, id: u64, result: Value) {
        let frame = Frame::Response(result_response(id, result));
        self.write_frame(&frame).await;
    }

    /// Answer request `id` with an AGP error frame (protocol §6).
    ///
    /// The wire `data` object is omitted; the client drops it anyway.
    pub async fn respond_error(&mut self, id: u64, code: ErrorCode, message: &str) {
        let frame = Frame::Response(error_response(id, ErrorPayload::new(code, message)));
        self.write_frame(&frame).await;
    }

    /// Write `line` verbatim plus a trailing newline (malformed/oversized tests).
    pub async fn send_raw(&mut self, line: &str) {
        let mut bytes = line.as_bytes().to_vec();
        bytes.push(b'\n');
        self.write_bytes(&bytes).await;
    }

    /// Push one event frame to the connected client.
    pub async fn emit_event(&mut self, name: &str, seq: u64, ts_ms: u64, data: Value) {
        // The typed proto surface can only represent known kinds with matching
        // data (`adesk_proto::EventPayload::from_data`); everything else — the
        // forward-compatibility cases `future_kind` and a bare `inspect_frame`
        // — is written as the equivalent raw NDJSON line.
        let typed = serde_json::from_value::<EventKind>(Value::String(name.to_owned()))
            .ok()
            .and_then(|kind| EventPayload::from_data(kind, data.clone()).ok());
        if typed.is_some() {
            let frame = Frame::Event(event_frame(name, seq, ts_ms, data));
            self.write_frame(&frame).await;
        } else {
            let line = serde_json::json!({
                "event": name,
                "seq": seq,
                "ts_ms": ts_ms,
                "data": data,
            })
            .to_string();
            self.send_raw(&line).await;
        }
    }

    /// Drop the connection: the client observes EOF (`ClientError::Closed`).
    pub async fn close(self) {
        // Dropping both halves closes the socket; the TempDir removes the path.
        let MockServer { reader, writer, .. } = self;
        drop(reader);
        drop(writer);
    }

    /// Read one newline-terminated line, without the terminator.
    async fn read_line(&mut self) -> Vec<u8> {
        let reader = self
            .reader
            .as_mut()
            .expect("mock server: accept() was not called");
        let mut line = Vec::new();
        let read = reader
            .read_until(b'\n', &mut line)
            .await
            .expect("mock server: read a client frame");
        assert!(
            read > 0,
            "mock server: the client closed the connection mid-request"
        );
        assert_eq!(
            line.last(),
            Some(&b'\n'),
            "mock server: frame is not newline-terminated"
        );
        line.pop();
        line
    }

    /// Encode `frame` as one NDJSON line and write it.
    async fn write_frame(&mut self, frame: &Frame) {
        let bytes = encode_frame(&self.codec, frame);
        self.write_bytes(&bytes).await;
    }

    /// Write raw bytes and flush.
    async fn write_bytes(&mut self, bytes: &[u8]) {
        let writer = self
            .writer
            .as_mut()
            .expect("mock server: accept() was not called");
        writer
            .write_all(bytes)
            .await
            .expect("mock server: write to the client");
        writer
            .flush()
            .await
            .expect("mock server: flush to the client");
    }
}

/// Encode one AGP frame as a complete NDJSON line (trailing `\n` included).
///
/// Shared by every sender above so the harness writes exactly the bytes the
/// runtime would (protocol §1). A codec error is a harness bug: panic.
fn encode_frame(codec: &NdjsonCodec, frame: &Frame) -> Vec<u8> {
    let mut bytes = codec.encode(frame).expect("mock server: encode frame");
    bytes.push(b'\n');
    bytes
}

/// Build the success `ResponseFrame` for `id`.
fn result_response(id: u64, result: Value) -> ResponseFrame {
    ResponseFrame {
        id,
        outcome: ResponseOutcome::Result(ResultPayload(result)),
    }
}

/// Build the error `ResponseFrame` for `id` from an AGP error object.
fn error_response(id: u64, error: ErrorPayload) -> ResponseFrame {
    ResponseFrame {
        id,
        outcome: ResponseOutcome::Error(error),
    }
}

/// Build the `EventFrame` for one pushed event (protocol §5.6).
///
/// Panics for a name/data pair the typed proto surface cannot represent; the
/// caller ([`MockServer::emit_event`]) checks first and falls back to raw JSON.
fn event_frame(name: &str, seq: u64, ts_ms: u64, data: Value) -> EventFrame {
    let kind: EventKind = serde_json::from_value(Value::String(name.to_owned()))
        .unwrap_or_else(|_| panic!("mock server: `{name}` is not a known event kind"));
    let payload = EventPayload::from_data(kind, data)
        .unwrap_or_else(|error| panic!("mock server: invalid `{name}` data: {error}"));
    EventFrame::new(kind, seq, ts_ms, payload)
}

/// Split a decoded `RequestFrame` into `(id, method, params)`.
fn request_parts(request: RequestFrame) -> (u64, String, Value) {
    let RequestFrame { id, method } = request;
    let name = method.method_name().to_owned();
    let params = method.params_value().unwrap_or_else(|error| {
        panic!("mock server: params of `{name}` do not serialise: {error}")
    });
    (id, name, params)
}
