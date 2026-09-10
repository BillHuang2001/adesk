//! The NDJSON text codec (`docs/viewer.md` §1).
//!
//! One UTF-8 JSON object per line, no embedded newlines, **no trailing
//! newline** — the transport adds the `\n`, exactly like
//! `adesk_proto::encode_frame`. Framing (the line cap, the terminator) belongs
//! to the transport, not this crate.

use serde_json::Value;

use crate::{ClientMessage, Result, ServerMessage, ViewerProtoError};

/// Encodes a viewer → server message as one line-ready JSON object.
///
/// No trailing newline is appended. Encoding is infallible: a VAP message is a
/// flat object of scalars, arrays and objects, and the only JSON-unrepresentable
/// value (a non-finite float) is outside the protocol's normalized-coordinate
/// contract.
pub fn encode_client(message: &ClientMessage) -> String {
    message.to_value().to_string()
}

/// Encodes a server → viewer message as one line-ready JSON object.
///
/// No trailing newline is appended; see [`encode_client`] for the infallibility
/// contract.
pub fn encode_server(message: &ServerMessage) -> String {
    message.to_value().to_string()
}

/// Decodes one viewer → server line.
///
/// Only the client `"type"` tags are recognised; any other tag (including a
/// server-only tag) decodes to [`ClientMessage::Unknown`] rather than an error
/// (§1).
///
/// # Errors
///
/// Returns [`ViewerProtoError::Malformed`] when `line` is not valid JSON, is not
/// a JSON object, has no `"type"`, or has a non-string `"type"`, and when a
/// recognised tag does not match its schema.
pub fn decode_client(line: &str) -> Result<ClientMessage> {
    ClientMessage::from_value(parse(line)?)
}

/// Decodes one server → viewer line.
///
/// Only the server `"type"` tags are recognised; any other tag (including a
/// client-only tag) decodes to [`ServerMessage::Unknown`] rather than an error
/// (§1).
///
/// # Errors
///
/// Returns [`ViewerProtoError::Malformed`] when `line` is not valid JSON, is not
/// a JSON object, has no `"type"`, or has a non-string `"type"`, and when a
/// recognised tag does not match its schema.
pub fn decode_server(line: &str) -> Result<ServerMessage> {
    ServerMessage::from_value(parse(line)?)
}

/// Parses a line into a JSON value, mapping any syntax error to
/// [`ViewerProtoError::Malformed`].
fn parse(line: &str) -> Result<Value> {
    serde_json::from_str(line).map_err(|_| ViewerProtoError::Malformed)
}
