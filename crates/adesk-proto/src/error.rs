//! Crate-local error type and its mapping to AGP error codes.

use adesk_core::ErrorCode;
use thiserror::Error;

/// Errors produced while encoding or decoding AGP frames.
///
/// Every variant maps to exactly one AGP error code (§6) through
/// [`ProtoError::error_code`], so the server can answer a malformed request
/// without inventing codes.
#[derive(Debug, Error)]
pub enum ProtoError {
    /// The payload is not a well-formed frame of any kind (§1).
    #[error("malformed frame: {0}")]
    Malformed(String),
    /// The `method` name is unknown; the server answers `unknown_method` (§1).
    #[error("unknown method: {0}")]
    UnknownMethod(String),
    /// The `params` object does not match the method's schema.
    #[error("invalid params for `{method}`: {message}")]
    InvalidParams {
        /// Wire name of the method whose params failed to decode.
        method: String,
        /// Underlying deserialization failure.
        message: String,
    },
    /// The `data` object does not match the event kind's schema.
    #[error("invalid event data for `{kind}`: {message}")]
    InvalidEventData {
        /// Wire name of the event kind whose `data` failed to decode.
        kind: String,
        /// Underlying deserialization failure.
        message: String,
    },
    /// The frame's `event` name is not a known AGP event kind (§5.6).
    #[error("unknown event kind: {0}")]
    UnknownEventKind(String),
    /// A typed result payload could not be decoded into the expected struct.
    #[error("invalid result payload: {0}")]
    InvalidResult(String),
    /// The peer speaks a different protocol version (§5.1, §7).
    #[error("protocol version mismatch: this build speaks v{expected}, peer sent v{got}")]
    VersionMismatch {
        /// Version this build implements ([`PROTOCOL_VERSION`](crate::PROTOCOL_VERSION)).
        expected: u32,
        /// Version reported by the peer.
        got: u32,
    },
    /// The payload is not valid JSON.
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    /// A base64 image payload could not be decoded (§4).
    #[error("base64 error: {0}")]
    Base64(#[from] base64::DecodeError),
}

/// Crate-local result type (`docs/core-api.md` convention).
pub type Result<T> = std::result::Result<T, ProtoError>;

impl ProtoError {
    /// AGP error code (§6) this failure maps to.
    pub fn error_code(&self) -> ErrorCode {
        match self {
            ProtoError::UnknownMethod(_) => ErrorCode::UnknownMethod,
            ProtoError::VersionMismatch { .. } => ErrorCode::ProtocolVersionMismatch,
            ProtoError::Malformed(_)
            | ProtoError::InvalidParams { .. }
            | ProtoError::InvalidEventData { .. }
            | ProtoError::UnknownEventKind(_)
            | ProtoError::InvalidResult(_)
            | ProtoError::Json(_)
            | ProtoError::Base64(_) => ErrorCode::InvalidRequest,
        }
    }
}

impl From<ProtoError> for adesk_core::Error {
    fn from(err: ProtoError) -> adesk_core::Error {
        adesk_core::Error::new(err.error_code(), err.to_string())
    }
}
