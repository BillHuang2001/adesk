//! AGP wire layer — the **only** module that names `adesk-proto` frame types.
//!
//! `adesk-proto` owns the AGP frame/codec definitions (root `CONTEXT.md`);
//! `adesk-client` is a consumer. To keep the client resilient while the two
//! crates are designed side by side, every `adesk_proto` item the client needs
//! is referenced here (plus the `ImagePayload` re-export in `lib.rs`), and this
//! module immediately converts proto frames into the crate-internal
//! [`Inbound`]/[`RawEvent`] vocabulary that the rest of the client uses.
//!
//! Consequences:
//!
//! - If `adesk-proto`'s frame API changes, only this file (and the
//!   `ImagePayload` re-export) needs updating.
//! - The transport never depends on proto types; it works on
//!   [`RawEvent`] and [`Inbound`] only.
//!
//! Required `adesk-proto` surface (see `CONTEXT.md` → "Required adesk-proto
//! surface"):
//! `PROTOCOL_VERSION`, `Codec::{new, encode, decode}`, `Frame`,
//! `Request { id, method, params }`, `Response::{Result{id,result},
//! Error{id,error}}`, `EventFrame { event, seq, ts_ms, data }`, and
//! `ImagePayload`.

use adesk_core::ErrorCode;
use adesk_proto::{Codec, EventFrame, Frame, Request, Response};
use serde_json::Value;

use crate::{ClientError, Result};

/// The AGP version this client implements (`docs/protocol.md` §5.1).
///
/// Sourced from `adesk-proto` so the client and the wire definitions can never
/// disagree about the version they speak.
pub(crate) const PROTOCOL_VERSION: u32 = adesk_proto::PROTOCOL_VERSION;

/// One inbound event frame, split into envelope and payload.
///
/// The payload stays as [`Value`] here; `events.rs` maps it to the typed
/// [`AgpEvent`](crate::AgpEvent) vocabulary (and to
/// [`RuntimeEvent`](adesk_core::RuntimeEvent)).
#[derive(Debug, Clone)]
pub(crate) struct RawEvent {
    /// Wire event name, e.g. `surface_commit` (protocol §5.6).
    pub(crate) name: String,
    /// Global monotonic event sequence.
    pub(crate) seq: u64,
    /// Monotonic milliseconds since runtime start.
    pub(crate) ts_ms: u64,
    /// Event-specific payload object.
    pub(crate) data: Value,
}

/// One inbound frame after direction checking.
#[derive(Debug)]
pub(crate) enum Inbound {
    /// A response to a request this client sent, matched by `id`.
    Response {
        /// Request id echoed by the server.
        id: u64,
        /// `Ok(result)` for a result frame, `Err(..)` for an error frame.
        result: Result<Value, ServerError>,
    },
    /// An unsolicited event frame.
    Event(RawEvent),
}

/// The AGP error object of a failed request (protocol §6).
///
/// The wire `data` object is intentionally not carried: it is advisory and the
/// typed error variants cover the actionable cases. If a consumer needs it,
/// add a field here rather than re-parsing the frame.
#[derive(Debug, Clone)]
pub(crate) struct ServerError {
    /// AGP error code.
    pub(crate) code: ErrorCode,
    /// Human-readable server message.
    pub(crate) message: String,
}

/// Encode one request frame as a complete NDJSON line (trailing `\n` included).
///
/// `params` must already be the method's params object; the caller owns the
/// request id. Encoding failure is a client-side bug or an oversized frame and
/// surfaces as [`ClientError::Protocol`].
pub(crate) fn encode_request(id: u64, method: &str, params: Value) -> Result<Vec<u8>> {
    let frame = Frame::Request(Request { id, method: method.to_owned(), params });
    Codec::new().encode(&frame).map_err(|error| ClientError::Protocol {
        message: format!("failed to encode request {id} ({method}): {error}"),
    })
}

/// Decode one inbound NDJSON line (without the trailing `\n`).
///
/// A request frame arriving from the server is a protocol violation: AGP is
/// full-duplex but only the client sends requests.
pub(crate) fn decode_line(line: &[u8]) -> Result<Inbound> {
    let frame = Codec::new().decode(line).map_err(|error| ClientError::Protocol {
        message: format!("malformed inbound frame: {error}"),
    })?;
    match frame {
        Frame::Response(Response::Result { id, result }) => {
            Ok(Inbound::Response { id, result: Ok(result) })
        }
        Frame::Response(Response::Error { id, error }) => Ok(Inbound::Response {
            id,
            result: Err(ServerError { code: error.code, message: error.message }),
        }),
        Frame::Event(EventFrame { event, seq, ts_ms, data }) => {
            Ok(Inbound::Event(RawEvent { name: event, seq, ts_ms, data }))
        }
        Frame::Request(_) => Err(ClientError::Protocol {
            message: "server sent a request frame".to_owned(),
        }),
    }
}
