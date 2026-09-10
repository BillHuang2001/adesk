//! Frame types: the three NDJSON frame kinds of §1 and their payloads.

use adesk_core::{ErrorCode, RuntimeEvent};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::event::{EventKind, EventPayload};
use crate::methods::Method;
use crate::{ProtoError, Result};

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
    /// Server → client unsolicited event (only after `subscribe_events` or
    /// `inspect_subscribe`).
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
/// result untyped. Pattern-match [`ResponseOutcome`] to reach the success
/// payload, then decode it with the method's typed result struct through
/// [`ResultPayload::decode`] (or `into_decode` to consume the payload) — e.g.
/// `let ResponseOutcome::Result(payload) = &response.outcome else { unreachable!() };`
/// followed by `let result: PingResult = payload.decode()?;`.
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
        Ok(ResultPayload(serde_json::to_value(result)?))
    }

    /// Decodes into the method's typed result struct.
    ///
    /// # Errors
    ///
    /// Returns [`ProtoError::InvalidResult`] when the payload does not match `R`.
    pub fn decode<R: serde::de::DeserializeOwned>(&self) -> Result<R> {
        serde_json::from_value(self.0.clone())
            .map_err(|error| ProtoError::InvalidResult(error.to_string()))
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
        Ok(ResponseFrame {
            id,
            outcome: ResponseOutcome::Result(ResultPayload::new(result)?),
        })
    }

    /// Builds an error response.
    pub fn error(id: u64, error: ErrorPayload) -> ResponseFrame {
        ResponseFrame {
            id,
            outcome: ResponseOutcome::Error(error),
        }
    }
}

impl Serialize for ResponseFrame {
    /// Emits `{"id": n, "result": ...}` or `{"id": n, "error": ...}`.
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;

        let mut map = serializer.serialize_map(Some(2))?;
        map.serialize_entry("id", &self.id)?;
        match &self.outcome {
            ResponseOutcome::Result(payload) => map.serialize_entry("result", payload)?,
            ResponseOutcome::Error(error) => map.serialize_entry("error", error)?,
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for ResponseFrame {
    /// Requires `id` and exactly one of `result`/`error`; both or neither is
    /// [`ProtoError::Malformed`].
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        response_from_value(value).map_err(serde::de::Error::custom)
    }
}

/// Reads a response frame from an already-parsed JSON value.
///
/// `id` is mandatory; exactly one of `result`/`error` must be present (§6).
fn response_from_value(value: Value) -> Result<ResponseFrame> {
    let Value::Object(object) = value else {
        return Err(ProtoError::Malformed(
            "response frame must be a JSON object".to_owned(),
        ));
    };
    let id = object
        .get("id")
        .and_then(Value::as_u64)
        .ok_or_else(|| ProtoError::Malformed("response frame requires an `id` u64".to_owned()))?;
    let outcome = match (object.get("result"), object.get("error")) {
        (Some(_), Some(_)) => {
            return Err(ProtoError::Malformed(
                "response frame carries both `result` and `error`".to_owned(),
            ))
        }
        (None, None) => {
            return Err(ProtoError::Malformed(
                "response frame carries neither `result` nor `error`".to_owned(),
            ))
        }
        (Some(result), None) => ResponseOutcome::Result(ResultPayload(result.clone())),
        (None, Some(error)) => {
            let error: ErrorPayload = serde_json::from_value(error.clone()).map_err(|error| {
                ProtoError::Malformed(format!("invalid `error` payload: {error}"))
            })?;
            ResponseOutcome::Error(error)
        }
    };
    Ok(ResponseFrame { id, outcome })
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
        let data = EventPayload::from_runtime(event);
        EventFrame {
            event: data.kind(),
            seq: event.seq(),
            ts_ms: event.ts_ms(),
            data,
        }
    }

    /// Rebuilds the core runtime event, stamping `seq`/`ts_ms` back onto it.
    ///
    /// Returns `None` for protocol-only kinds (`quiet`, `inspect_frame`).
    pub fn to_runtime(&self) -> Option<RuntimeEvent> {
        self.data.to_runtime(self.seq, self.ts_ms)
    }
}

impl Serialize for EventFrame {
    /// Emits `{"event": ..., "seq": ..., "ts_ms": ..., "data": {...}}`.
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        use serde::ser::{Error as _, SerializeMap};

        let data = self.data.to_data().map_err(S::Error::custom)?;
        let mut map = serializer.serialize_map(Some(4))?;
        map.serialize_entry("event", &self.event)?;
        map.serialize_entry("seq", &self.seq)?;
        map.serialize_entry("ts_ms", &self.ts_ms)?;
        map.serialize_entry("data", &data)?;
        map.end()
    }
}

impl<'de> Deserialize<'de> for EventFrame {
    /// Reads `event`/`seq`/`ts_ms`/`data` and dispatches `data` by `event`.
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        event_from_value(value).map_err(serde::de::Error::custom)
    }
}

/// Reads an event frame from an already-parsed JSON value (§1, §5.6).
fn event_from_value(value: Value) -> Result<EventFrame> {
    let Value::Object(object) = value else {
        return Err(ProtoError::Malformed(
            "event frame must be a JSON object".to_owned(),
        ));
    };
    let name = match object.get("event") {
        Some(Value::String(name)) => name.clone(),
        Some(other) => {
            return Err(ProtoError::Malformed(format!(
                "event frame's `event` must be a string, got {other}"
            )))
        }
        None => {
            return Err(ProtoError::Malformed(
                "event frame requires an `event` field".to_owned(),
            ))
        }
    };
    let event: EventKind = serde_json::from_value(Value::String(name.clone()))
        .map_err(|_| ProtoError::UnknownEventKind(name))?;
    let seq = object
        .get("seq")
        .and_then(Value::as_u64)
        .ok_or_else(|| ProtoError::Malformed("event frame requires a `seq` u64".to_owned()))?;
    let ts_ms = object
        .get("ts_ms")
        .and_then(Value::as_u64)
        .ok_or_else(|| ProtoError::Malformed("event frame requires a `ts_ms` u64".to_owned()))?;
    let data = match object.get("data") {
        None | Some(Value::Null) => Value::Object(Map::new()),
        Some(data) => data.clone(),
    };
    let data = EventPayload::from_data(event, data)?;
    Ok(EventFrame::new(event, seq, ts_ms, data))
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
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        match self {
            Frame::Request(frame) => frame.serialize(serializer),
            Frame::Response(frame) => frame.serialize(serializer),
            Frame::Event(frame) => frame.serialize(serializer),
        }
    }
}

impl Frame {
    /// Builds a frame from an already-parsed JSON value.
    ///
    /// This is the single decode entry point for the crate: serde's
    /// `Deserialize` cannot carry [`ProtoError`] payloads, so the codec parses
    /// the line itself and discriminates here (§1).
    ///
    /// # Errors
    ///
    /// Returns [`ProtoError::Malformed`] when the value matches no frame shape,
    /// [`ProtoError::UnknownMethod`] / [`ProtoError::InvalidParams`] for request
    /// frames, [`ProtoError::UnknownEventKind`] / [`ProtoError::InvalidEventData`]
    /// for event frames.
    pub(crate) fn from_value(value: Value) -> Result<Frame> {
        let Value::Object(object) = value else {
            return Err(ProtoError::Malformed(
                "frame must be a JSON object".to_owned(),
            ));
        };
        if object.contains_key("event") {
            return Ok(Frame::Event(event_from_value(Value::Object(object))?));
        }
        if object.contains_key("method") {
            return Ok(Frame::Request(request_from_value(Value::Object(object))?));
        }
        if object.contains_key("id") {
            return Ok(Frame::Response(response_from_value(Value::Object(object))?));
        }
        Err(ProtoError::Malformed(
            "frame has none of `event`, `method` or `id`".to_owned(),
        ))
    }
}

/// Reads a request frame from an already-parsed JSON value (§1).
fn request_from_value(value: Value) -> Result<RequestFrame> {
    let Value::Object(object) = value else {
        return Err(ProtoError::Malformed(
            "request frame must be a JSON object".to_owned(),
        ));
    };
    let id = object
        .get("id")
        .and_then(Value::as_u64)
        .ok_or_else(|| ProtoError::Malformed("request frame requires an `id` u64".to_owned()))?;
    let name = match object.get("method") {
        Some(Value::String(name)) => name.clone(),
        Some(other) => {
            return Err(ProtoError::Malformed(format!(
                "request frame's `method` must be a string, got {other}"
            )))
        }
        None => {
            return Err(ProtoError::Malformed(
                "request frame requires a `method` field".to_owned(),
            ))
        }
    };
    // Absent or `null` params mean "no parameters" (§1 examples, §5.1 `ping`).
    let params = match object.get("params") {
        None | Some(Value::Null) => Value::Object(Map::new()),
        Some(params) => params.clone(),
    };
    Ok(RequestFrame::new(id, Method::from_parts(&name, params)?))
}

impl<'de> Deserialize<'de> for Frame {
    /// Discriminates by keys: `event` → event, `method` → request,
    /// `id` + exactly one of `result`/`error` → response.
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        Frame::from_value(value).map_err(serde::de::Error::custom)
    }
}
