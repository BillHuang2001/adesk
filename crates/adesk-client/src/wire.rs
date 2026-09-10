//! AGP wire layer — the **only** module that names `adesk-proto` frame types.
//!
//! `adesk-proto` owns the AGP frame/codec definitions (root `CONTEXT.md`);
//! `adesk-client` is a consumer. To keep the client resilient while the two
//! crates are designed side by side, every `adesk_proto` item the client needs
//! is referenced here (plus the `ImagePayload`/`QuietEvent` re-exports in
//! `lib.rs`), and this module immediately converts proto frames into the
//! crate-internal [`Inbound`]/[`RawEvent`] vocabulary that the rest of the
//! client uses.
//!
//! Consequences:
//!
//! - If `adesk-proto`'s frame API changes, only this file (and the payload
//!   re-exports) needs updating.
//! - The transport never depends on proto types; it works on
//!   [`RawEvent`] and [`Inbound`] only.
//!
//! Required `adesk-proto` surface (see `CONTEXT.md` → "Required adesk-proto
//! surface"):
//! `PROTOCOL_VERSION`, `Frame::{Request, Response, Event}`,
//! `RequestFrame { id, method }` + `RequestFrame::new` and
//! `Method::from_parts(name, params)`,
//! `ResponseFrame { id, outcome }` + `ResponseOutcome::{Result(ResultPayload),
//! Error(ErrorPayload)}` + `ResultPayload::as_value`,
//! `EventFrame { event, seq, ts_ms, data }`, `EventKind` (snake_case serde),
//! `EventPayload::to_data`, `NdjsonCodec` + the `Codec` trait, and the
//! `ImagePayload` / `QuietEvent` payloads.

use adesk_core::ErrorCode;
use adesk_proto::methods::Method;
use adesk_proto::{
    Codec, EventFrame, EventKind, Frame, NdjsonCodec, RequestFrame, ResponseFrame, ResponseOutcome,
};
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

/// The wire name of an event kind (`snake_case`, protocol §5.6).
///
/// [`EventKind`] exposes no `as_str()`, so the name is read back from its serde
/// representation (the same encoding the frame's `event` field uses).
fn event_name(kind: EventKind) -> Result<String> {
    match serde_json::to_value(kind).map_err(|error| ClientError::Protocol {
        message: format!("failed to encode event name: {error}"),
    })? {
        Value::String(name) => Ok(name),
        other => Err(ClientError::Protocol {
            message: format!("event name is not a string: {other}"),
        }),
    }
}

/// Encode one request frame as a complete NDJSON line (trailing `\n` included).
///
/// `params` must already be the method's params object; the caller owns the
/// request id. Encoding failure is a client-side bug or an oversized frame and
/// surfaces as [`ClientError::Protocol`].
pub(crate) fn encode_request(id: u64, method: &str, params: Value) -> Result<Vec<u8>> {
    let typed = Method::from_parts(method, params).map_err(|error| ClientError::Protocol {
        message: format!("failed to encode request {id} ({method}): {error}"),
    })?;
    let frame = Frame::Request(RequestFrame::new(id, typed));
    // The codec payload has no terminator; the NDJSON line adds it.
    let mut line = NdjsonCodec
        .encode(&frame)
        .map_err(|error| ClientError::Protocol {
            message: format!("failed to encode request {id} ({method}): {error}"),
        })?;
    line.push(b'\n');
    Ok(line)
}

/// Decode one inbound NDJSON line (without the trailing `\n`).
///
/// A request frame arriving from the server is a protocol violation: AGP is
/// full-duplex but only the client sends requests.
///
/// The strict `adesk-proto` codec rejects an event frame whose `event` name it
/// does not know ([`adesk_proto::ProtoError::UnknownEventKind`]), which would
/// make the forward-compatibility escape hatch [`AgpEvent::Other`](crate::AgpEvent::Other)
/// unreachable (protocol §7: additive event kinds must not break a client). When
/// the strict decode fails, the line is therefore retried as a bare JSON object:
/// a string `event` plus numeric `seq`/`ts_ms` (optional `data`) becomes a
/// [`RawEvent`] and everything else keeps the original [`ClientError::Protocol`].
pub(crate) fn decode_line(line: &[u8]) -> Result<Inbound> {
    let error = match NdjsonCodec.decode(line) {
        Ok(frame) => return from_frame(frame),
        Err(error) => error,
    };
    match lenient_event(line) {
        Some(event) => Ok(Inbound::Event(event)),
        None => Err(ClientError::Protocol {
            message: format!("malformed inbound frame: {error}"),
        }),
    }
}

/// Convert a strictly decoded frame into the crate-internal inbound vocabulary.
fn from_frame(frame: Frame) -> Result<Inbound> {
    match frame {
        Frame::Response(ResponseFrame { id, outcome }) => match outcome {
            ResponseOutcome::Result(payload) => Ok(Inbound::Response {
                id,
                result: Ok(payload.as_value().clone()),
            }),
            ResponseOutcome::Error(error) => Ok(Inbound::Response {
                id,
                result: Err(ServerError {
                    code: error.code,
                    message: error.message,
                }),
            }),
        },
        Frame::Event(EventFrame {
            event,
            seq,
            ts_ms,
            data,
        }) => {
            let name = event_name(event)?;
            let data = data.to_data().map_err(|error| ClientError::Protocol {
                message: format!("malformed event payload: {error}"),
            })?;
            Ok(Inbound::Event(RawEvent {
                name,
                seq,
                ts_ms,
                data,
            }))
        }
        Frame::Request(_) => Err(ClientError::Protocol {
            message: "server sent a request frame".to_owned(),
        }),
    }
}

/// Parse an event frame the strict codec refused, as raw JSON (protocol §7).
///
/// Requires an object with a string `event` and numeric `seq`/`ts_ms`; `data`
/// is optional and defaults to an empty object. Anything else is not an event
/// frame and keeps the strict error.
fn lenient_event(line: &[u8]) -> Option<RawEvent> {
    let Value::Object(object) = serde_json::from_slice::<Value>(line).ok()? else {
        return None;
    };
    let name = match object.get("event") {
        Some(Value::String(name)) => name.clone(),
        _ => return None,
    };
    let seq = object.get("seq").and_then(Value::as_u64)?;
    let ts_ms = object.get("ts_ms").and_then(Value::as_u64)?;
    let data = match object.get("data") {
        None | Some(Value::Null) => Value::Object(serde_json::Map::new()),
        Some(data) => data.clone(),
    };
    Some(RawEvent {
        name,
        seq,
        ts_ms,
        data,
    })
}
