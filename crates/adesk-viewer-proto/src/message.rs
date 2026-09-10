//! The two VAP message enums and their `"type"`-tag dispatch (`docs/viewer.md` §2–§4).
//!
//! Both enums use the same convention: the wire form is a flat JSON object with a
//! `"type"` discriminator plus the message's fields. An unrecognised `"type"` is
//! decoded into the `Unknown` variant rather than being an error, so a peer stays
//! forward-compatible (§1, §7); see [`crate::codec`] for the text entry points.

use adesk_core::{ActionId, Button, ButtonState, ErrorCode};
use adesk_proto::KeySpec;
use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};

use crate::types::{ControlOwner, DesktopState, KeyAction, ServerHello, ViewerFrame, ViewerHello};
use crate::{Result, ViewerProtoError};

/// A message from a Viewer to the ADesk runtime (`docs/viewer.md` §2, §4).
#[derive(Debug, Clone, PartialEq)]
pub enum ClientMessage {
    /// Handshake (§2).
    Hello(ViewerHello),
    /// Render and push one `frame` now (headless pull). `id` is echoed.
    RequestFrame {
        /// Optional client id, echoed by the matching `input_ack`/`error`.
        id: Option<u64>,
    },
    /// Push the current `state`. `id` is echoed.
    RequestState {
        /// Optional client id, echoed by the matching `input_ack`/`error`.
        id: Option<u64>,
    },
    /// Move the pointer to a normalized output position.
    PointerMove {
        /// Horizontal output fraction.
        x: f64,
        /// Vertical output fraction.
        y: f64,
    },
    /// Press or release a pointer button, optionally moving first.
    PointerButton {
        /// Which button.
        button: Button,
        /// Pressed or released.
        state: ButtonState,
        /// Optional horizontal output fraction to move to first.
        x: Option<f64>,
        /// Optional vertical output fraction to move to first.
        y: Option<f64>,
    },
    /// Scroll at an optional normalized position.
    Scroll {
        /// Horizontal scroll delta.
        dx: f64,
        /// Vertical scroll delta.
        dy: f64,
        /// Optional horizontal output fraction to scroll at.
        x: Option<f64>,
        /// Optional vertical output fraction to scroll at.
        y: Option<f64>,
    },
    /// Press, release or tap a key or chord.
    Key {
        /// The key or chord (an AGP `KeySpec`).
        keys: KeySpec,
        /// What to do with it.
        action: KeyAction,
    },
    /// Type UTF-8 text.
    Text {
        /// The text to type.
        text: String,
    },
    /// Announce who owns input (`"ai"` | `"human"`).
    SetControl {
        /// The announced input owner.
        owner: ControlOwner,
    },
    /// The viewer is leaving.
    Bye {
        /// Optional human-readable reason.
        reason: Option<String>,
    },
    /// A message whose `"type"` this build does not recognise (forward
    /// compatibility, §1). The original JSON object is preserved verbatim.
    Unknown {
        /// The unrecognised `"type"` value.
        message_type: String,
        /// The full original JSON object.
        value: Value,
    },
}

/// A message from the ADesk runtime to a Viewer (`docs/viewer.md` §2, §3).
#[derive(Debug, Clone, PartialEq)]
pub enum ServerMessage {
    /// Handshake acknowledgement + display metadata (§2).
    Hello(ServerHello),
    /// One rendered desktop frame (§3).
    Frame(ViewerFrame),
    /// Desktop metadata (§3).
    State(DesktopState),
    /// Who owns input (`"ai"` | `"human"`).
    Control {
        /// The current input owner.
        owner: ControlOwner,
    },
    /// An input message with client id `id` was applied as AGP `action_id`.
    InputAck {
        /// The client id being acknowledged, when the input carried one.
        id: Option<u64>,
        /// The AGP action id the runtime recorded.
        action_id: ActionId,
    },
    /// A message could not be applied; the connection stays open (§6).
    Error {
        /// AGP error code.
        code: ErrorCode,
        /// Human-readable explanation.
        message: String,
        /// The client id being answered, when known.
        id: Option<u64>,
    },
    /// The server is ending the connection.
    Bye {
        /// Why the connection is ending.
        reason: String,
    },
    /// A message whose `"type"` this build does not recognise (forward
    /// compatibility, §1). The original JSON object is preserved verbatim.
    Unknown {
        /// The unrecognised `"type"` value.
        message_type: String,
        /// The full original JSON object.
        value: Value,
    },
}

impl ClientMessage {
    /// The wire `"type"` discriminator of this message.
    ///
    /// For [`ClientMessage::Unknown`] this is the stored `message_type`.
    pub fn message_type(&self) -> &str {
        match self {
            ClientMessage::Hello(_) => "hello",
            ClientMessage::RequestFrame { .. } => "request_frame",
            ClientMessage::RequestState { .. } => "request_state",
            ClientMessage::PointerMove { .. } => "pointer_move",
            ClientMessage::PointerButton { .. } => "pointer_button",
            ClientMessage::Scroll { .. } => "scroll",
            ClientMessage::Key { .. } => "key",
            ClientMessage::Text { .. } => "text",
            ClientMessage::SetControl { .. } => "set_control",
            ClientMessage::Bye { .. } => "bye",
            ClientMessage::Unknown { message_type, .. } => message_type,
        }
    }

    /// Decodes a client message from its JSON object form, mapping an unknown
    /// `"type"` to [`ClientMessage::Unknown`].
    ///
    /// # Errors
    ///
    /// Returns [`ViewerProtoError::Malformed`] when `value` is not a JSON object
    /// or has no string `"type"`, and when a known `"type"` does not match its
    /// schema.
    pub(crate) fn from_value(value: Value) -> Result<ClientMessage> {
        let tag = type_tag(&value)?;
        match tag {
            "hello" => Ok(ClientMessage::Hello(decode(value)?)),
            "request_frame" => {
                let fields: IdFields = decode(value)?;
                Ok(ClientMessage::RequestFrame { id: fields.id })
            }
            "request_state" => {
                let fields: IdFields = decode(value)?;
                Ok(ClientMessage::RequestState { id: fields.id })
            }
            "pointer_move" => {
                let fields: PointerMoveFields = decode(value)?;
                Ok(ClientMessage::PointerMove {
                    x: fields.x,
                    y: fields.y,
                })
            }
            "pointer_button" => {
                let fields: PointerButtonFields = decode(value)?;
                Ok(ClientMessage::PointerButton {
                    button: fields.button,
                    state: fields.state,
                    x: fields.x,
                    y: fields.y,
                })
            }
            "scroll" => {
                let fields: ScrollFields = decode(value)?;
                Ok(ClientMessage::Scroll {
                    dx: fields.dx,
                    dy: fields.dy,
                    x: fields.x,
                    y: fields.y,
                })
            }
            "key" => {
                let fields: KeyFields = decode(value)?;
                Ok(ClientMessage::Key {
                    keys: fields.keys,
                    action: fields.action,
                })
            }
            "text" => {
                let fields: TextFields = decode(value)?;
                Ok(ClientMessage::Text { text: fields.text })
            }
            "set_control" => {
                let fields: SetControlFields = decode(value)?;
                Ok(ClientMessage::SetControl {
                    owner: fields.owner,
                })
            }
            "bye" => {
                let fields: ByeFields = decode(value)?;
                Ok(ClientMessage::Bye {
                    reason: fields.reason,
                })
            }
            other => Ok(ClientMessage::Unknown {
                message_type: other.to_owned(),
                value,
            }),
        }
    }

    /// Builds the flat JSON object form of this message.
    pub(crate) fn to_value(&self) -> Value {
        match self {
            ClientMessage::Hello(payload) => tagged("hello", payload),
            ClientMessage::RequestFrame { id } => {
                let mut map = tagged_empty("request_frame");
                insert_optional_id(&mut map, *id);
                Value::Object(map)
            }
            ClientMessage::RequestState { id } => {
                let mut map = tagged_empty("request_state");
                insert_optional_id(&mut map, *id);
                Value::Object(map)
            }
            ClientMessage::PointerMove { x, y } => {
                let mut map = tagged_empty("pointer_move");
                map.insert("x".to_owned(), Value::from(*x));
                map.insert("y".to_owned(), Value::from(*y));
                Value::Object(map)
            }
            ClientMessage::PointerButton {
                button,
                state,
                x,
                y,
            } => {
                let mut map = tagged_empty("pointer_button");
                map.insert("button".to_owned(), field(button));
                map.insert("state".to_owned(), field(state));
                insert_optional_f64(&mut map, "x", *x);
                insert_optional_f64(&mut map, "y", *y);
                Value::Object(map)
            }
            ClientMessage::Scroll { dx, dy, x, y } => {
                let mut map = tagged_empty("scroll");
                map.insert("dx".to_owned(), Value::from(*dx));
                map.insert("dy".to_owned(), Value::from(*dy));
                insert_optional_f64(&mut map, "x", *x);
                insert_optional_f64(&mut map, "y", *y);
                Value::Object(map)
            }
            ClientMessage::Key { keys, action } => {
                let mut map = tagged_empty("key");
                map.insert("keys".to_owned(), field(keys));
                // `docs/viewer.md` §4 names this wire field `state`.
                map.insert("state".to_owned(), field(action));
                Value::Object(map)
            }
            ClientMessage::Text { text } => {
                let mut map = tagged_empty("text");
                map.insert("text".to_owned(), Value::String(text.clone()));
                Value::Object(map)
            }
            ClientMessage::SetControl { owner } => {
                let mut map = tagged_empty("set_control");
                map.insert("owner".to_owned(), field(owner));
                Value::Object(map)
            }
            ClientMessage::Bye { reason } => {
                let mut map = tagged_empty("bye");
                if let Some(reason) = reason {
                    map.insert("reason".to_owned(), Value::String(reason.clone()));
                }
                Value::Object(map)
            }
            ClientMessage::Unknown { value, .. } => value.clone(),
        }
    }
}

impl ServerMessage {
    /// The wire `"type"` discriminator of this message.
    ///
    /// For [`ServerMessage::Unknown`] this is the stored `message_type`.
    pub fn message_type(&self) -> &str {
        match self {
            ServerMessage::Hello(_) => "hello",
            ServerMessage::Frame(_) => "frame",
            ServerMessage::State(_) => "state",
            ServerMessage::Control { .. } => "control",
            ServerMessage::InputAck { .. } => "input_ack",
            ServerMessage::Error { .. } => "error",
            ServerMessage::Bye { .. } => "bye",
            ServerMessage::Unknown { message_type, .. } => message_type,
        }
    }

    /// Decodes a server message from its JSON object form, mapping an unknown
    /// `"type"` to [`ServerMessage::Unknown`].
    ///
    /// # Errors
    ///
    /// Returns [`ViewerProtoError::Malformed`] when `value` is not a JSON object
    /// or has no string `"type"`, and when a known `"type"` does not match its
    /// schema.
    pub(crate) fn from_value(value: Value) -> Result<ServerMessage> {
        let tag = type_tag(&value)?;
        match tag {
            "hello" => Ok(ServerMessage::Hello(decode(value)?)),
            "frame" => Ok(ServerMessage::Frame(decode(value)?)),
            "state" => Ok(ServerMessage::State(decode(value)?)),
            "control" => {
                let fields: ControlFields = decode(value)?;
                Ok(ServerMessage::Control {
                    owner: fields.owner,
                })
            }
            "input_ack" => {
                let fields: InputAckFields = decode(value)?;
                Ok(ServerMessage::InputAck {
                    id: fields.id,
                    action_id: fields.action_id,
                })
            }
            "error" => {
                let fields: ErrorFields = decode(value)?;
                Ok(ServerMessage::Error {
                    code: fields.code,
                    message: fields.message,
                    id: fields.id,
                })
            }
            "bye" => {
                let fields: ServerByeFields = decode(value)?;
                Ok(ServerMessage::Bye {
                    reason: fields.reason,
                })
            }
            other => Ok(ServerMessage::Unknown {
                message_type: other.to_owned(),
                value,
            }),
        }
    }

    /// Builds the flat JSON object form of this message.
    pub(crate) fn to_value(&self) -> Value {
        match self {
            ServerMessage::Hello(payload) => tagged("hello", payload),
            ServerMessage::Frame(payload) => tagged("frame", payload),
            ServerMessage::State(payload) => tagged("state", payload),
            ServerMessage::Control { owner } => {
                let mut map = tagged_empty("control");
                map.insert("owner".to_owned(), field(owner));
                Value::Object(map)
            }
            ServerMessage::InputAck { id, action_id } => {
                let mut map = tagged_empty("input_ack");
                insert_optional_id(&mut map, *id);
                map.insert("action_id".to_owned(), field(action_id));
                Value::Object(map)
            }
            ServerMessage::Error { code, message, id } => {
                let mut map = tagged_empty("error");
                map.insert("code".to_owned(), field(code));
                map.insert("message".to_owned(), Value::String(message.clone()));
                insert_optional_id(&mut map, *id);
                Value::Object(map)
            }
            ServerMessage::Bye { reason } => {
                let mut map = tagged_empty("bye");
                map.insert("reason".to_owned(), Value::String(reason.clone()));
                Value::Object(map)
            }
            ServerMessage::Unknown { value, .. } => value.clone(),
        }
    }
}

impl Serialize for ClientMessage {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        self.to_value().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ClientMessage {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        ClientMessage::from_value(value).map_err(D::Error::custom)
    }
}

impl Serialize for ServerMessage {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        self.to_value().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ServerMessage {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        ServerMessage::from_value(value).map_err(D::Error::custom)
    }
}

/// Reads the string `"type"` discriminator of a message object.
fn type_tag(value: &Value) -> Result<&str> {
    value
        .as_object()
        .and_then(|object| object.get("type"))
        .and_then(Value::as_str)
        .ok_or(ViewerProtoError::Malformed)
}

/// Deserializes a known message payload, mapping every failure to
/// [`ViewerProtoError::Malformed`].
fn decode<T: serde::de::DeserializeOwned>(value: Value) -> Result<T> {
    serde_json::from_value(value).map_err(|_| ViewerProtoError::Malformed)
}

/// Serializes a VAP field to a JSON value; the vocabulary types used on the
/// wire are plain enums/strings, so this never fails.
fn field<T: Serialize + ?Sized>(value: &T) -> Value {
    serde_json::to_value(value).expect("a VAP field always serialises to JSON")
}

/// Builds `{ "type": <tag> }`.
fn tagged_empty(tag: &str) -> Map<String, Value> {
    let mut map = Map::new();
    map.insert("type".to_owned(), Value::String(tag.to_owned()));
    map
}

/// Builds `{ "type": <tag>, <payload fields> }`.
fn tagged<T: Serialize>(tag: &str, payload: &T) -> Value {
    let mut map = tagged_empty(tag);
    if let Value::Object(fields) = field(payload) {
        map.extend(fields);
    }
    Value::Object(map)
}

/// Inserts `id` when present (the `id?` optional wire field).
fn insert_optional_id(map: &mut Map<String, Value>, id: Option<u64>) {
    if let Some(id) = id {
        map.insert("id".to_owned(), Value::from(id));
    }
}

/// Inserts a normalized coordinate when present (an `x?`/`y?` optional field).
fn insert_optional_f64(map: &mut Map<String, Value>, key: &str, value: Option<f64>) {
    if let Some(value) = value {
        map.insert(key.to_owned(), Value::from(value));
    }
}

/// Deserializes the `request_frame`/`request_state` `id?` field.
#[derive(Deserialize)]
struct IdFields {
    #[serde(default)]
    id: Option<u64>,
}

/// Deserializes `pointer_move` fields.
#[derive(Deserialize)]
struct PointerMoveFields {
    x: f64,
    y: f64,
}

/// Deserializes `pointer_button` fields.
#[derive(Deserialize)]
struct PointerButtonFields {
    button: Button,
    state: ButtonState,
    #[serde(default)]
    x: Option<f64>,
    #[serde(default)]
    y: Option<f64>,
}

/// Deserializes `scroll` fields.
#[derive(Deserialize)]
struct ScrollFields {
    dx: f64,
    dy: f64,
    #[serde(default)]
    x: Option<f64>,
    #[serde(default)]
    y: Option<f64>,
}

/// Deserializes `key` fields (the wire field is `state`, §4).
#[derive(Deserialize)]
struct KeyFields {
    keys: KeySpec,
    #[serde(rename = "state")]
    action: KeyAction,
}

/// Deserializes the `text` field.
#[derive(Deserialize)]
struct TextFields {
    text: String,
}

/// Deserializes the `set_control` field.
#[derive(Deserialize)]
struct SetControlFields {
    owner: ControlOwner,
}

/// Deserializes the client `bye` `reason?` field.
#[derive(Deserialize)]
struct ByeFields {
    #[serde(default)]
    reason: Option<String>,
}

/// Deserializes the `control` field.
#[derive(Deserialize)]
struct ControlFields {
    owner: ControlOwner,
}

/// Deserializes `input_ack` fields.
#[derive(Deserialize)]
struct InputAckFields {
    #[serde(default)]
    id: Option<u64>,
    action_id: ActionId,
}

/// Deserializes `error` fields.
#[derive(Deserialize)]
struct ErrorFields {
    code: ErrorCode,
    message: String,
    #[serde(default)]
    id: Option<u64>,
}

/// Deserializes the server `bye` `reason` field.
#[derive(Deserialize)]
struct ServerByeFields {
    reason: String,
}
