//! Frame types: the three NDJSON frame kinds of §1 and their payloads.

use adesk_core::{ErrorCode, RuntimeEvent};
use serde::{Deserialize, Serialize};

use crate::event::{EventKind, EventPayload};
use crate::methods::Method;
use crate::Result;

/// One AGP frame (§1).
///
/// Wire discrimination on decode: a frame with `"event"` is an event, one with
/// `"method"` is a request, one with `"id"` plus exactly one of `"result"` /
/// `"error"` is a response; anything else is [`ProtoError::Malformed`].
#[derive(Debug, Clone, PartialEq)]
pub enum Frame {
    /// Client → server request; exactly one response follows.
    Request(RequestFrame),
    /// Server → client response.
    Response(ResponseFrame),
    /// Server → client unsolicited event (only after `subscribe_events`).
    Event(EventFrame),
}

/// Request frame: `{"id": 1, "method": "...", "params": {...}}` (§1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RequestFrame {
    /// Client-assigned id, unique per connection while in flight.
    pub id: u64,
    /// Typed method; serializes as the sibling `method`/`params` fields.
    #[serde(flatten)]
    pub method: Method,
}

impl RequestFrame {
    /// Builds a request.
    pub fn new(id: u64, method: Method) -> RequestFrame {
        RequestFrame { id, method }
    }
}

/// Response frame: `{"id": 1, "result": {...}}` or `{"id": 1, "error": {...}}` (§1).
#[derive(Debug, Clone, PartialEq)]
pub struct ResponseFrame {
    /// Id of the request this answers.
    pub id: u64,
    /// Exactly one outcome (§6: the server answers every request once).
    pub outcome: ResponseOutcome,
}

/// Either a successful result or an error — never both, never neither (§6).
#[derive(Debug, Clone, PartialEq)]
pub enum ResponseOutcome {
    /// Successful result payload.
    Result(ResultPayload),
    /// Structured error payload.
    Error(ErrorPayload),
}

/// Untyped result payload.
///
/// A codec cannot know which method an `id` belongs to, so responses carry the
/// result untyped; decode it with the method's typed result struct:
/// `response.outcome.result_payload().unwrap().decode::<PingResult>()?`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ResultPayload(pub serde_json::Value);

impl ResultPayload {
    /// Encodes a typed result struct.
    ///
    /// # Errors
    ///
    /// Returns [`ProtoError::Json`] if serialization fails.
    pub fn new<R: Serialize>(result: &R) -> Result<ResultPayload> {
        todo!()
    }

    /// Decodes into the method's typed result struct.
    ///
    /// # Errors
    ///
    /// Returns [`ProtoError::InvalidResult`] when the payload does not match `R`.
    pub fn decode<R: serde::de::DeserializeOwned>(&self) -> Result<R> {
        todo!()
    }

    /// The raw JSON value.
    pub fn as_value(&self) -> &serde_json::Value {
        &self.0
    }
}

/// Wire error object (§6): `{"code": "...", "message": "...", "data": {...}?}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ErrorPayload {
    /// AGP error code; wire name is [`ErrorCode::as_str`].
    pub code: ErrorCode,
    /// Human-readable explanation.
    pub message: String,
    /// Optional machine-readable details (omitted when `None`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

impl ErrorPayload {
    /// Builds an error payload without `data`.
    pub fn new(code: ErrorCode, message: impl Into<String>) -> ErrorPayload {
        ErrorPayload {
            code,
            message: message.into(),
            data: None,
        }
    }

    /// Attaches machine-readable details.
    pub fn with_data(mut self, data: serde_json::Value) -> ErrorPayload {
        self.data = Some(data);
        self
    }
}

impl ResponseFrame {
    /// Builds a success response, encoding `result`.
    ///
    /// # Errors
    ///
    /// Returns [`ProtoError::Json`] if `result` fails to serialize.
    pub fn result<R: Serialize>(id: u64, result: &R) -> Result<ResponseFrame> {
        todo!()
    }

    /// Builds an error response.
    pub fn error(id: u64, error: ErrorPayload) -> ResponseFrame {
        ResponseFrame {
            id,
            outcome: ResponseOutcome::Error(error),
        }
    }
}

impl ResponseOutcome {
    /// The result payload, when this outcome is a success.
    pub fn result_payload(&self) -> Option<&ResultPayload> {
        match self {
            ResponseOutcome::Result(payload) => Some(payload),
            ResponseOutcome::Error(_) => None,
        }
    }

    /// The error payload, when this outcome is an error.
    pub fn error_payload(&self) -> Option<&ErrorPayload> {
        match self {
            ResponseOutcome::Result(_) => None,
            ResponseOutcome::Error(error) => Some(error),
        }
    }

    /// Whether this outcome is an error.
    pub fn is_error(&self) -> bool {
        matches!(self, ResponseOutcome::Error(_))
    }
}

impl Serialize for ResponseFrame {
    /// Emits `{"id": n, "result": ...}` or `{"id": n, "error": ...}`.
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        todo!()
    }
}

impl<'de> Deserialize<'de> for ResponseFrame {
    /// Requires `id` and exactly one of `result`/`error`; both or neither is
    /// [`ProtoError::Malformed`].
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        todo!()
    }
}

/// Event frame: `{"event": "...", "seq": n, "ts_ms": n, "data": {...}}` (§1).
///
/// `seq`/`ts_ms` are hoisted from the core [`RuntimeEvent`]; `data` is the
/// variant fields minus those two.
#[derive(Debug, Clone, PartialEq)]
pub struct EventFrame {
    /// The event kind (the frame's `event` field).
    pub event: EventKind,
    /// Global monotonic sequence assigned by the runtime.
    pub seq: u64,
    /// Milliseconds since runtime start.
    pub ts_ms: u64,
    /// Typed payload (the frame's `data` object).
    pub data: EventPayload,
}

impl EventFrame {
    /// Builds an event frame.
    pub fn new(event: EventKind, seq: u64, ts_ms: u64, data: EventPayload) -> EventFrame {
        EventFrame {
            event,
            seq,
            ts_ms,
            data,
        }
    }

    /// Builds a frame from a core runtime event, hoisting `seq`/`ts_ms` (§1).
    pub fn from_runtime(event: &RuntimeEvent) -> EventFrame {
        todo!()
    }

    /// Rebuilds the core runtime event, stamping `seq`/`ts_ms` back onto it.
    ///
    /// Returns `None` for protocol-only kinds (`quiet`, `inspect_frame`).
    pub fn to_runtime(&self) -> Option<RuntimeEvent> {
        todo!()
    }
}

impl Serialize for EventFrame {
    /// Emits `{"event": ..., "seq": ..., "ts_ms": ..., "data": {...}}`.
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        todo!()
    }
}

impl<'de> Deserialize<'de> for EventFrame {
    /// Reads `event`/`seq`/`ts_ms`/`data` and dispatches `data` by `event`.
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        todo!()
    }
}

impl From<RequestFrame> for Frame {
    fn from(frame: RequestFrame) -> Frame {
        Frame::Request(frame)
    }
}

impl From<ResponseFrame> for Frame {
    fn from(frame: ResponseFrame) -> Frame {
        Frame::Response(frame)
    }
}

impl From<EventFrame> for Frame {
    fn from(frame: EventFrame) -> Frame {
        Frame::Event(frame)
    }
}

impl Serialize for Frame {
    /// Delegates to the variant's wire shape (§1).
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        match self {
            Frame::Request(frame) => frame.serialize(serializer),
            Frame::Response(frame) => frame.serialize(serializer),
            Frame::Event(frame) => frame.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for Frame {
    /// Discriminates by keys: `event` → event, `method` → request,
    /// `id` + exactly one of `result`/`error` → response.
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        todo!()
    }
}
