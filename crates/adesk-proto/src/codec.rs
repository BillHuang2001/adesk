//! Wire codec: NDJSON today, a binary framing later (§1, §7).
//!
//! [`Codec`] is deliberately payload-oriented (bytes, no terminator) so a future
//! binary framing can be added without touching any method definition; the
//! NDJSON-specific string conveniences are [`NdjsonCodec::encode_str`] and
//! [`NdjsonCodec::decode_str`].

use crate::{Frame, ProtoError, Result};

/// A wire codec: one frame ↔ one transport payload.
///
/// For NDJSON a payload is one UTF-8 JSON line; a binary codec would use one
/// packet. Implementations must never include a terminator in [`Codec::encode`]
/// output — the transport adds it.
pub trait Codec: std::fmt::Debug + Send + Sync {
    /// Codec name, for diagnostics.
    fn name(&self) -> &'static str;

    /// Encodes one frame into a transport payload without a terminator.
    ///
    /// # Errors
    ///
    /// Returns a [`ProtoError`] when the frame cannot be encoded.
    fn encode(&self, frame: &Frame) -> Result<Vec<u8>>;

    /// Decodes one transport payload into a frame.
    ///
    /// # Errors
    ///
    /// Returns a [`ProtoError`] when the payload is not a
    /// well-formed frame.
    fn decode(&self, payload: &[u8]) -> Result<Frame>;
}

/// The NDJSON codec (§1): one UTF-8 JSON object per line, no embedded newlines.
#[derive(Debug, Default, Clone, Copy)]
pub struct NdjsonCodec;

impl NdjsonCodec {
    /// Encodes a frame as a JSON string **without** a trailing newline (the
    /// transport adds the `\n`).
    ///
    /// # Errors
    ///
    /// Returns [`ProtoError::Json`] on serialization failure.
    pub fn encode_str(&self, frame: &Frame) -> Result<String> {
        // `serde_json` escapes control characters inside strings, so the object
        // never contains a raw newline (§1: no embedded newlines).
        Ok(serde_json::to_string(frame)?)
    }

    /// Decodes one JSON line.
    ///
    /// # Errors
    ///
    /// Returns a [`ProtoError`] for malformed JSON, unknown
    /// methods/kinds, or frames that match no frame shape.
    pub fn decode_str(&self, line: &str) -> Result<Frame> {
        let value: serde_json::Value = serde_json::from_str(line)?;
        Frame::from_value(value)
    }
}

impl Codec for NdjsonCodec {
    fn name(&self) -> &'static str {
        "ndjson"
    }

    fn encode(&self, frame: &Frame) -> Result<Vec<u8>> {
        Ok(self.encode_str(frame)?.into_bytes())
    }

    fn decode(&self, payload: &[u8]) -> Result<Frame> {
        let line = std::str::from_utf8(payload).map_err(|error| {
            ProtoError::Malformed(format!("payload is not valid UTF-8: {error}"))
        })?;
        self.decode_str(line)
    }
}
