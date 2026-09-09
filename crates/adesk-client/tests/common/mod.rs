#![allow(dead_code)] // skeleton phase: items are used once the Manager implements the tests
#![allow(unused_variables)] // skeleton phase: todo!() bodies do not read their parameters yet

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
//!
//! Skeleton phase: every body is `todo!()`. The field list and the signatures
//! are the contract the Manager phase implements.

use std::path::{Path, PathBuf};

use adesk_core::ErrorCode;
use adesk_proto::{ErrorPayload, EventFrame, Frame, NdjsonCodec, RequestFrame, ResponseFrame};
use serde_json::Value;
use tempfile::TempDir;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
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
    reader: Option<OwnedReadHalf>,
    /// Write half of the accepted connection (responses, events, raw lines).
    writer: Option<OwnedWriteHalf>,
    /// NDJSON codec — the same `adesk-proto` codec the runtime uses.
    codec: NdjsonCodec,
}

impl MockServer {
    /// Bind `<tempdir>/adesk.sock` and start listening.
    pub async fn start() -> MockServer {
        todo!("create a TempDir, bind a UnixListener on <dir>/adesk.sock, and return the harness with no connection accepted yet")
    }

    /// Socket path to hand to `Client::connect` / `ConnectOptions::path`.
    pub fn path(&self) -> &Path {
        &self.socket_path
    }

    /// Accept exactly one client connection and split it into read/write halves.
    pub async fn accept(&mut self) {
        todo!("listener.accept() -> UnixStream, then into_split() into reader/writer fields; panics on accept error, which is a test bug")
    }

    /// Read one request frame and return `(id, method, params)`.
    ///
    /// Panics if the client sends anything that is not a well-formed NDJSON
    /// `Frame::Request` (that is a client bug, not a server behaviour to model).
    pub async fn next_request(&mut self) -> (u64, String, Value) {
        todo!("read one newline-terminated line, NdjsonCodec::decode it, and destructure Frame::Request into (id, method, params)")
    }

    /// Answer request `id` with `result` (a `ResponseOutcome::Result` frame).
    pub async fn respond(&mut self, id: u64, result: Value) {
        todo!("write the encoded ResponseFrame with ResponseOutcome::Result for id and flush")
    }

    /// Answer request `id` with an AGP error frame (protocol §6).
    ///
    /// The wire `data` object is omitted; the client drops it anyway.
    pub async fn respond_error(&mut self, id: u64, code: ErrorCode, message: &str) {
        todo!("build ErrorPayload with code + message, wrap it as ResponseOutcome::Error for id, write and flush")
    }

    /// Write `line` verbatim plus a trailing newline (malformed/oversized tests).
    pub async fn send_raw(&mut self, line: &str) {
        todo!("write line bytes followed by b'\\n' and flush; no encoding, no validation")
    }

    /// Push one event frame to the connected client.
    pub async fn emit_event(&mut self, name: &str, seq: u64, ts_ms: u64, data: Value) {
        todo!("build EventFrame with event = name, seq, ts_ms, data, wrap it in Frame::Event, write and flush")
    }

    /// Drop the connection: the client observes EOF (`ClientError::Closed`).
    pub async fn close(self) {
        todo!("drop the reader/writer halves so the client sees EOF; the TempDir removes the socket path on drop")
    }
}

/// Encode one AGP frame as a complete NDJSON line (trailing `\n` included).
///
/// Shared by every sender above so the harness writes exactly the bytes the
/// runtime would (protocol §1). A codec error is a harness bug: panic.
fn encode_frame(codec: &NdjsonCodec, frame: &Frame) -> Vec<u8> {
    todo!("NdjsonCodec::encode(frame); a failure is a test-harness bug and may panic")
}

/// Build the success `ResponseFrame` for `id`.
fn result_response(id: u64, result: Value) -> ResponseFrame {
    todo!("ResponseFrame with ResponseOutcome::Result for the given id and result value")
}

/// Build the error `ResponseFrame` for `id` from an AGP error object.
fn error_response(id: u64, error: ErrorPayload) -> ResponseFrame {
    todo!("ResponseFrame with ResponseOutcome::Error for the given id and ErrorPayload")
}

/// Build the `EventFrame` for one pushed event (protocol §5.6).
fn event_frame(name: &str, seq: u64, ts_ms: u64, data: Value) -> EventFrame {
    todo!("EventFrame with event = name, seq, ts_ms, data")
}

/// Split a decoded `RequestFrame` into `(id, method, params)`.
fn request_parts(request: RequestFrame) -> (u64, String, Value) {
    todo!("destructure RequestFrame into id, method name and params")
}
